//! Each machine's `herdr-app` document: latest-state CAS writes, one request
//! per machine at a time and no queue of operations.
//!
//! Adapted from herdr-app `src/mirror_sync.rs`, Apache-2.0.

use herdc_model::mirror::Document;
use herdc_wire::{Client, Error};
use serde_json::json;
use std::time::{Duration, Instant};

pub const NAME: &str = "herdr-app";

#[derive(Default, PartialEq)]
enum ReadState {
    #[default]
    Unread,
    Available,
    Unavailable,
    Blocked,
}

#[derive(Default)]
pub struct Slot {
    document: Option<Document>,
    version: u64,
    read: ReadState,
    connected: bool,
    connected_since: Option<u64>,
    pending: bool,
    dirty: bool,
    error: Option<String>,
    notified_error: Option<String>,
}

pub struct Applied {
    pub replace: Option<Document>,
    pub first: bool,
    pub conflict: bool,
    pub notice: Option<String>,
}

impl Slot {
    pub fn loaded(&self) -> bool { self.document.is_some() }
    pub fn available(&self) -> bool { self.read == ReadState::Available }
    pub fn error(&self) -> Option<String> { self.error.clone() }
    pub fn invalidate(&mut self) { self.dirty = true; }

    pub fn layout(&self) -> &'static str {
        if self.available() { "ready" }
        else if self.connected && self.read == ReadState::Unread { "loading" }
        else { "unavailable" }
    }

    fn ready(&self) -> bool {
        self.connected && !self.pending && (self.read != ReadState::Blocked || self.dirty)
    }

    pub fn restart(&mut self, local: bool) {
        self.read = ReadState::Unread;
        self.connected = local;
        self.pending = false;
    }

    pub fn lost(&mut self) {
        self.read = ReadState::Unavailable;
        self.pending = false;
        self.connected = false;
    }

    pub fn connection(&mut self, connected: bool, since: Option<u64>, reason: Option<String>) {
        if connected && (!self.connected || self.connected_since != since) {
            self.dirty = true;
            self.read = ReadState::Unread;
            self.error = None;
        } else if !connected {
            self.read = ReadState::Unavailable;
            self.error = reason;
        }
        self.connected = connected;
        self.connected_since = since;
    }

    pub fn prepare(&mut self, machine: &str, projection: Document, refresh: bool) -> Option<Work> {
        self.dirty |= refresh;
        if !self.ready() { return None; }
        let write = (self.available() && self.document.as_ref() != Some(&projection)).then_some(projection);
        if write.is_none() && self.available() && !self.dirty { return None; }
        self.pending = true;
        self.dirty = false;
        Some(Work { machine: machine.into(), version: self.version, write })
    }

    pub fn apply(&mut self, outcome: Outcome, projection: &Document) -> Applied {
        self.pending = false;
        let mut applied = Applied { replace: None, first: self.document.is_none(), conflict: false, notice: None };
        let (document, version, replace) = match outcome {
            Outcome::Read(d, v) => {
                let replace = !self.available() || self.version != v || self.document.is_none();
                let unsent = self.document.as_ref().is_some_and(|old| old != projection);
                applied.conflict = replace && unsent && *projection != d;
                (d, v, replace)
            }
            Outcome::Written(d, v) => (d, v, false),
            Outcome::Conflict(d, v) => { applied.conflict = true; (d, v, true) }
            Outcome::Unavailable(e) => {
                self.failed(e, false, &mut applied);
                return applied;
            }
            Outcome::Undecodable(e) | Outcome::Blocked(e) => {
                self.failed(e, true, &mut applied);
                return applied;
            }
        };
        self.version = version;
        self.read = if self.connected { ReadState::Available } else { ReadState::Unavailable };
        if self.connected {
            self.error = None;
            self.notified_error = None;
        }
        self.document = Some(document.clone());
        if replace { applied.replace = Some(document); }
        applied
    }

    fn failed(&mut self, error: String, blocked: bool, applied: &mut Applied) {
        self.read = if blocked { ReadState::Blocked } else { ReadState::Unavailable };
        if self.connected || self.error.is_none() { self.error = Some(error.clone()); }
        if self.connected && blocked && self.notified_error.as_ref() != Some(&error) {
            self.notified_error = Some(error.clone());
            applied.notice = Some(error);
        }
    }
}

pub struct Work {
    pub machine: String,
    pub version: u64,
    pub write: Option<Document>,
}

#[derive(Debug)]
pub enum Outcome {
    Read(Document, u64),
    Written(Document, u64),
    Conflict(Document, u64),
    Undecodable(String),
    Blocked(String),
    Unavailable(String),
}

fn rpc_error(error: Error) -> Outcome {
    let reason = error.to_string();
    match error.kind() {
        Some("needs_update" | "needs_login") => Outcome::Blocked(reason),
        _ => Outcome::Unavailable(reason),
    }
}

async fn read(core: &Client, machine: &str, timeout: Duration) -> Result<(Document, u64), Outcome> {
    let on = (!machine.is_empty()).then_some(machine);
    let result = core.call_timeout("doc.get", json!({"name": NAME, "on": on}), timeout).await.map_err(rpc_error)?;
    let document = if result["value"].is_null() {
        Document::default()
    } else {
        serde_json::from_value(result["value"].clone())
            .map_err(|e| Outcome::Undecodable(format!("Cannot read this machine's layout: {e}")))?
    };
    let version = result["version"].as_u64().ok_or_else(|| Outcome::Undecodable("Layout document has no version".into()))?;
    Ok((document, version))
}

pub async fn round(core: Client, work: Work) -> (String, Outcome) {
    let timeout = Duration::from_secs(10);
    let on = (!work.machine.is_empty()).then_some(work.machine.as_str());
    let deadline = Instant::now() + timeout;
    let result = match work.write {
        Some(document) => {
            let params = json!({"name": NAME, "on": on, "version": work.version, "value": document});
            match core.call_timeout("doc.set", params, timeout).await {
                Ok(v) => match v["version"].as_u64() {
                    Some(version) => Outcome::Written(document, version),
                    None => Outcome::Undecodable("Layout write returned no version".into()),
                },
                Err(e) if e.kind() == Some("conflict") => {
                    match read(&core, &work.machine, deadline.saturating_duration_since(Instant::now())).await {
                        Ok((d, v)) => Outcome::Conflict(d, v),
                        Err(e) => e,
                    }
                }
                Err(e) => rpc_error(e),
            }
        }
        None => match read(&core, &work.machine, timeout).await {
            Ok((d, v)) => Outcome::Read(d, v),
            Err(e) => e,
        },
    };
    (work.machine, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_during_a_request_is_read_after_completion() {
        let mut slot = Slot::default();
        slot.connection(true, Some(1), None);
        let doc = Document::default();
        assert!(slot.prepare("", doc.clone(), false).is_some());
        slot.invalidate();
        assert!(slot.prepare("", doc.clone(), false).is_none());
        slot.apply(Outcome::Read(doc.clone(), 1), &doc);
        assert!(slot.prepare("", doc.clone(), false).is_some());
        slot.apply(Outcome::Read(doc.clone(), 1), &doc);
        assert!(slot.prepare("", doc.clone(), false).is_none());
        slot.lost();
        assert!(!slot.available());
        assert!(slot.prepare("", doc.clone(), true).is_none());
        slot.restart(true);
        assert_eq!(slot.layout(), "loading");
        assert!(slot.prepare("", doc, false).is_some());
    }

    #[test]
    fn conflict_replaces_projection_and_retains_the_remote_version() {
        let mut slot = Slot::default();
        slot.connection(true, None, None);
        let local: Document = serde_json::from_value(json!({"spaces":[{"id":"local"}]})).unwrap();
        let remote = Document::default();
        slot.apply(Outcome::Read(remote.clone(), 1), &remote);
        let write = slot.prepare("", local.clone(), false).unwrap();
        assert_eq!(write.version, 1);
        assert_eq!(write.write, Some(local.clone()));
        let applied = slot.apply(Outcome::Conflict(remote.clone(), 2), &local);
        assert!(applied.conflict);
        assert_eq!(applied.replace, Some(remote.clone()));
        assert!(slot.prepare("", remote, false).is_none());
        assert_eq!(slot.prepare("", local, false).unwrap().version, 2);
    }

    #[test]
    fn layout_loading_ends_on_read_failure_and_restarts_on_reconnect() {
        let mut slot = Slot::default();
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(1), None);
        assert_eq!(slot.layout(), "loading");
        slot.apply(Outcome::Unavailable("offline".into()), &Document::default());
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(1), None);
        assert_eq!(slot.layout(), "unavailable");
        slot.apply(Outcome::Read(Document::default(), 1), &Document::default());
        assert_eq!(slot.layout(), "ready");
        slot.connection(false, Some(2), Some("offline".into()));
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(3), None);
        assert_eq!(slot.layout(), "loading");
    }

    #[test]
    fn blocked_reads_wait_for_an_event_or_reconnect() {
        let mut slot = Slot { read: ReadState::Blocked, connected: true, ..Default::default() };
        assert!(!slot.ready());
        slot.dirty = true;
        assert!(slot.ready());
        slot.pending = true;
        assert!(!slot.ready());
        slot.pending = false;
        slot.dirty = false;
        assert!(!slot.ready());
        slot.read = ReadState::Unavailable;
        assert!(slot.ready());
    }

    #[test]
    fn offline_slots_do_not_read_even_when_dirty() {
        let mut slot = Slot::default();
        slot.connection(false, Some(1), Some("machine is connecting".into()));
        slot.dirty = true;
        assert!(!slot.ready());
        assert_eq!(slot.error.as_deref(), Some("machine is connecting"));
        slot.connection(true, Some(2), None);
        assert!(slot.ready());
        assert!(slot.error.is_none());
    }

    #[test]
    fn login_and_update_errors_keep_the_real_reason_and_hint() {
        for kind in ["needs_login", "needs_update"] {
            let error = Error::Rpc {
                method: "doc.get".into(),
                kind: Some(kind.into()),
                message: "cannot use machine".into(),
                hint: Some("run hc machine login".into()),
            };
            let Outcome::Blocked(reason) = rpc_error(error) else { panic!("must not retry this error on a timer") };
            assert!(reason.contains("cannot use machine"));
            assert!(reason.contains("run hc machine login"));
        }
    }
}
