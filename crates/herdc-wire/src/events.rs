//! The terminals an event subscription describes, kept current from its
//! JSON lines (herdr-core `schema/README.md`, Events).
//!
//! Adapted from herdr-app `src/hc.rs` (`EventLines`), Apache-2.0.

use serde_json::{json, Value};
use std::collections::HashMap;

/// What a push changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pushed {
    /// A document changed, or events were missed: read documents again.
    pub docs: bool,
    /// Machines (`machine` on doc events) whose documents changed; `None` is this one.
    pub doc_machines: Vec<Option<String>>,
    pub terminals: bool,
}

#[derive(Debug, Default)]
pub struct Terminals {
    pending: Vec<u8>,
    /// The last event applied: older ones were in the snapshot.
    seq: u64,
    /// Every terminal the stream describes, in its order.
    terminals: Vec<Value>,
    /// Each terminal's highest `herdr.seen` among the label changes since the
    /// last `take_seen`: `terminals` keeps only the last, which may hide a
    /// write of ours under a later, lower one (herdr-app docs/labels.md).
    seen: HashMap<String, u64>,
    /// A fresh snapshot replaced the terminals since the last `take_seen`.
    resynced: bool,
}

impl Terminals {
    /// From `events.subscribe`'s reply: its `seq` and `terminals`.
    pub fn new(snapshot: &Value) -> Self {
        let terminals = snapshot["terminals"].as_array().cloned().unwrap_or_default();
        Terminals { seq: snapshot["seq"].as_u64().unwrap_or(0), terminals, resynced: true, ..Default::default() }
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Every record, live or finished, in the core's order.
    pub fn all(&self) -> &[Value] {
        &self.terminals
    }

    /// The live terminals, as `terminal.list` puts them.
    pub fn live(&self) -> Vec<Value> {
        self.terminals.iter().filter(|t| t["state"] == "live").cloned().collect()
    }

    pub fn get(&self, address: &str) -> Option<&Value> {
        self.terminals.iter().find(|t| t["address"] == address)
    }

    /// The `herdr.seen` values that went by, and whether a snapshot replaced everything.
    pub fn take_seen(&mut self) -> (HashMap<String, u64>, bool) {
        (std::mem::take(&mut self.seen), std::mem::take(&mut self.resynced))
    }

    fn note_seen(&mut self, address: &Value, labels: &Value) {
        let (Some(address), Some(seen)) =
            (address.as_str(), labels["herdr.seen"].as_str().and_then(|s| s.parse::<u64>().ok()))
        else {
            return;
        };
        let high = self.seen.entry(address.to_owned()).or_default();
        *high = (*high).max(seen);
    }

    /// Applies stream bytes; lines may be split anywhere.
    pub fn push(&mut self, bytes: &[u8]) -> Pushed {
        self.pending.extend_from_slice(bytes);
        let mut pushed = Pushed::default();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Value = serde_json::from_slice(&self.pending[..end]).unwrap_or_default();
            self.pending.drain(..=end);
            self.apply_line(&line, &mut pushed);
        }
        pushed
    }

    fn apply_line(&mut self, line: &Value, pushed: &mut Pushed) {
        let p = &line["params"];
        match line["method"].as_str() {
            Some("missed") => {
                self.terminals = p["terminals"].as_array().cloned().unwrap_or_default();
                self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                self.resynced = true;
                pushed.docs = true;
                pushed.terminals = true;
            }
            Some("event") if p["seq"].as_u64().is_none_or(|s| s > self.seq) => {
                self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                if p["type"] == "doc" {
                    pushed.docs = true;
                    let machine = p["machine"].as_str().map(str::to_owned);
                    if !pushed.doc_machines.contains(&machine) {
                        pushed.doc_machines.push(machine);
                    }
                } else {
                    pushed.terminals |= self.apply(p);
                }
            }
            _ => {}
        }
    }

    /// One event's change to the terminal it names.
    fn apply(&mut self, p: &Value) -> bool {
        let (kind, address, data) = (p["type"].as_str().unwrap_or_default(), &p["terminal"], &p["data"]);
        if matches!(kind, "created" | "restarted" | "labels") {
            self.note_seen(address, &data["labels"]);
        }
        let at = self.terminals.iter().position(|t| &t["address"] == address);
        let whole = match kind {
            "created" | "restarted" => Some(data),
            "labels" => data.get("record"),
            _ => None,
        };
        if let Some(record) = whole {
            match at {
                Some(i) => self.terminals[i] = record.clone(),
                None => self.terminals.push(record.clone()),
            }
            return true;
        }
        let Some(i) = at else { return false };
        if kind == "removed" {
            self.terminals.remove(i);
            return true;
        }
        let t = &mut self.terminals[i];
        let mut set = |keys: &[&str]| keys.iter().for_each(|k| t[*k] = data[*k].clone());
        match kind {
            "finished" | "lost" => {
                set(&["reason", "exit_code", "signal", "finished_ms"]);
                t["state"] = if kind == "lost" { "live" } else { "finished" }.into();
                if kind == "lost" {
                    t["pid"] = Value::Null;
                }
                if let Some(r) = t.as_object_mut() {
                    r.remove("foreground");
                }
            }
            "status" => set(&["status", "records", "reported"]),
            "turn" => set(&["turn"]),
            "title" => set(&["title"]),
            "labels" => set(&["labels"]),
            "keep" => set(&["keep"]),
            "size" => set(&["cols", "rows"]),
            "foreground" => t["foreground"] = data.clone(),
            _ => return false,
        }
        true
    }
}

/// Builds an event line, for tests of code that consumes them.
pub fn event_line(seq: u64, kind: &str, terminal: &str, data: Value) -> Vec<u8> {
    format!("{}\n", json!({"jsonrpc":"2.0","method":"event","params":{"seq":seq,"type":kind,"terminal":terminal,"data":data}}))
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(address: &str) -> Value {
        json!({"address": address, "state": "live", "labels": {}})
    }

    #[test]
    fn events_keep_the_terminals_as_the_core_has_them() {
        let mut t = Terminals::new(&json!({"seq": 1, "terminals": [rec("t1")]}));
        assert!(t.push(&event_line(2, "created", "t2", rec("t2"))).terminals);
        assert!(t.push(&event_line(3, "status", "t2", json!({"status":{"state":"working"},"records":{},"reported":true}))).terminals);
        assert_eq!(t.get("t2").unwrap()["status"]["state"], "working");
        // Old events (already in the snapshot) are ignored.
        assert!(!t.push(&event_line(3, "removed", "t2", json!({}))).terminals);
        assert!(t.push(&event_line(4, "finished", "t1", json!({"reason":"exited","exit_code":0}))).terminals);
        assert_eq!(t.live().len(), 1);
        assert!(t.push(&event_line(5, "removed", "t1", json!({}))).terminals);
        assert_eq!(t.all().len(), 1);
    }

    #[test]
    fn lines_split_anywhere_and_missed_resyncs() {
        let mut t = Terminals::new(&json!({"seq": 0, "terminals": []}));
        let line = event_line(1, "created", "t1", rec("t1"));
        let (a, b) = line.split_at(10);
        assert_eq!(t.push(a), Pushed::default());
        assert!(t.push(b).terminals);
        let missed = format!("{}\n", json!({"method":"missed","params":{"seq":9,"terminals":[rec("t5")]}}));
        let pushed = t.push(missed.as_bytes());
        assert!(pushed.docs && pushed.terminals);
        assert_eq!(t.seq(), 9);
        assert!(t.get("t1").is_none() && t.get("t5").is_some());
    }

    #[test]
    fn doc_events_name_their_machine() {
        let mut t = Terminals::new(&json!({"seq": 0, "terminals": []}));
        let local = format!("{}\n", json!({"method":"event","params":{"seq":1,"type":"doc","data":{"name":"herdr-app","version":2}}}));
        let remote = format!("{}\n", json!({"method":"event","params":{"seq":2,"type":"doc","machine":"vps","data":{"name":"herdr-app","version":7}}}));
        let pushed = t.push(format!("{local}{remote}").as_bytes());
        assert!(pushed.docs);
        assert_eq!(pushed.doc_machines, [None, Some("vps".into())]);
    }

    #[test]
    fn a_seen_write_stays_known_under_a_later_lower_one() {
        let mut t = Terminals::new(&json!({"seq": 0, "terminals": [rec("t1")]}));
        for (seq, seen) in [(1, "4"), (2, "2")] {
            t.push(&event_line(seq, "labels", "t1", json!({"labels": {"herdr.seen": seen}})));
        }
        assert_eq!(t.get("t1").unwrap()["labels"]["herdr.seen"], "2");
        let (seen, resynced) = t.take_seen();
        assert_eq!(seen["t1"], 4);
        assert!(resynced);
        assert!(!t.take_seen().1);
    }
}
