//! Each machine's `herdr-app` document: latest-state CAS writes, one request
//! per machine at a time and no queue of operations.
//!
//! Adapted from herdr-app `src/mirror_sync.rs`, Apache-2.0.

use herdc_model::mirror::Document;
use herdc_wire::{Client, Error};
use serde_json::json;
use std::time::{Duration, Instant};

pub const NAME: &str = "herdr-app";

#[derive(Default)]
pub struct Slot {
    pub document: Option<Document>,
    pub version: u64,
    pub available: bool,
    pub read_attempted: bool,
    pub connected: bool,
    connected_since: Option<u64>,
    pub pending: bool,
    pub dirty: bool,
    pub blocked: bool,
    pub error: Option<String>,
    pub notified_error: Option<String>,
}

impl Slot {
    pub fn layout(&self) -> &'static str {
        if self.available {
            "ready"
        } else if self.connected && !self.read_attempted {
            "loading"
        } else {
            "unavailable"
        }
    }

    pub fn ready(&self) -> bool {
        self.connected && !self.pending && (!self.blocked || self.dirty)
    }

    pub fn connection(&mut self, connected: bool, since: Option<u64>, reason: Option<String>) {
        if connected && (!self.connected || self.connected_since != since) {
            self.blocked = false;
            self.dirty = true;
            self.available = false;
            self.read_attempted = false;
            self.error = None;
        } else if !connected {
            self.available = false;
            self.error = reason;
        }
        self.connected = connected;
        self.connected_since = since;
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
    fn layout_loading_ends_on_read_failure_and_restarts_on_reconnect() {
        let mut slot = Slot::default();
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(1), None);
        assert_eq!(slot.layout(), "loading");
        slot.read_attempted = true;
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(1), None);
        assert_eq!(slot.layout(), "unavailable");
        slot.available = true;
        assert_eq!(slot.layout(), "ready");
        slot.connection(false, Some(2), Some("offline".into()));
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(3), None);
        assert_eq!(slot.layout(), "loading");
    }

    #[test]
    fn blocked_reads_wait_for_an_event_or_reconnect() {
        let mut slot = Slot { blocked: true, connected: true, ..Default::default() };
        assert!(!slot.ready());
        slot.dirty = true;
        assert!(slot.ready());
        slot.pending = true;
        assert!(!slot.ready());
        slot.pending = false;
        slot.dirty = false;
        assert!(!slot.ready());
        slot.blocked = false;
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
