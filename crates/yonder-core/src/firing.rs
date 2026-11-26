//! What yonderd does to the ledger: fire due reminders, start and settle
//! their checks, and keep the deliveries to the lead agent. Each step is
//! one immediate transaction, and a trigger is written as a delivery in
//! the same one that moves its reminder on, so it is sent at most once.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use yonder_model::{Reminder, ReminderState, envelope};

use crate::check_exec::{CheckError, CheckOutput};
use crate::redact::excerpt;
use crate::store::{Result, Store, get_reminder};
use crate::when::{Rule, format_utc};

/// What of a check's output a delivery shows.
pub const ECHO_CHARS: usize = 1500;
/// Failures in a row that are told, once.
pub const FAILURES_TOLD: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Time,
    CheckChanged,
    CheckFailing,
    CheckExpired,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Time => "time",
            Trigger::CheckChanged => "check_changed",
            Trigger::CheckFailing => "check_failing",
            Trigger::CheckExpired => "check_expired",
        }
    }
}

/// A trigger written down as a delivery.
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub reminder_id: String,
    pub trigger: Trigger,
    pub content: String,
}

/// A check to run now, for the time its reminder was due.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckJob {
    pub id: String,
    pub cmd: String,
    pub due_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Fired(Fired),
    Check(CheckJob),
    /// Not due, not active, or its trigger was written already.
    Nothing,
}

/// An envelope on its way to the lead agent.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub id: i64,
    pub reminder_id: String,
    pub body: String,
}

impl Store {
    /// Moves a due reminder on. A plain one becomes a delivery and fires
    /// (once) or goes to its next time (repeating). A check past its limit
    /// becomes a `check_expired` delivery and ends; else it goes to its
    /// next time first, and is handed back to run.
    pub fn fire(&mut self, id: &str, now: i64) -> Result<Step> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let r = get_reminder(&tx, id)?;
        let at = format_utc(now);
        if r.state != ReminderState::Active || r.due_at() > at.as_str() {
            return Ok(Step::Nothing);
        }
        let due = r.due_at().to_string();
        let next = r.every.as_deref().and_then(Rule::parse).and_then(|rule| rule.next_after(now));
        let over = r.check_until.as_deref().is_some_and(|until| until < at.as_str());
        let step = match (&r.check_cmd, next) {
            (Some(cmd), Some(next)) if !over => {
                move_on(&tx, id, next, &at)?;
                Step::Check(CheckJob { id: r.id.clone(), cmd: cmd.clone(), due_at: due })
            }
            (Some(_), _) => {
                let until = r.check_until.as_deref().unwrap_or("its last time");
                let note = envelope::escape(&format!("The check ran until {until}; it is over."));
                let fired = deliver(&tx, &r, Trigger::CheckExpired, &due, &note, &at)?;
                end(&tx, id, ReminderState::Expired, &at)?;
                fired.map_or(Step::Nothing, Step::Fired)
            }
            (None, next) => {
                let fired = deliver(&tx, &r, Trigger::Time, &due, "", &at)?;
                match next {
                    Some(next) => move_on(&tx, id, next, &at)?,
                    None => end(&tx, id, ReminderState::Fired, &at)?,
                }
                fired.map_or(Step::Nothing, Step::Fired)
            }
        };
        tx.commit()?;
        Ok(step)
    }

    /// Records how a check went: a change becomes a delivery with the new
    /// output (and ends a `check_once` reminder); the third failure in a
    /// row becomes one, then nothing until it works again.
    pub fn finish_check(&mut self, job: &CheckJob, result: &std::result::Result<CheckOutput, CheckError>, now: i64) -> Result<Option<Fired>> {
        if matches!(result, Err(CheckError::Stopped)) {
            return Ok(None);
        }
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let r = get_reminder(&tx, &job.id)?;
        // Cancelled or changed while it ran.
        if r.state != ReminderState::Active || r.check_cmd.as_ref() != Some(&job.cmd) {
            return Ok(None);
        }
        let at = format_utc(now);
        let fired = match result {
            Ok(out) if r.check_hash.as_ref() != Some(&out.hash) => {
                let shown = envelope::payload("check_output", &excerpt(&out.stdout, ECHO_CHARS));
                let tail = format!("Check output now:\n{shown}");
                let fired = deliver(&tx, &r, Trigger::CheckChanged, &job.due_at, &tail, &at)?;
                tx.execute(
                    "UPDATE reminders SET check_hash = ?2, last_checked_at = ?3, check_failures = 0, check_error = NULL,
                         check_alerted = 0, last_fired_at = ?3, updated_at = ?3,
                         state = CASE WHEN check_once = 1 THEN 'fired' ELSE state END
                     WHERE id = ?1",
                    params![r.id, out.hash, at],
                )?;
                fired
            }
            Ok(_) => {
                tx.execute(
                    "UPDATE reminders SET last_checked_at = ?2, check_failures = 0, check_error = NULL, check_alerted = 0,
                         updated_at = ?2
                     WHERE id = ?1",
                    params![r.id, at],
                )?;
                None
            }
            Err(e) => {
                let failures = r.check_failures + 1;
                let alerted: bool = tx.query_row("SELECT check_alerted FROM reminders WHERE id = ?1", [&r.id], |row| row.get(0))?;
                let tell = failures >= FAILURES_TOLD && !alerted;
                tx.execute(
                    "UPDATE reminders SET last_checked_at = ?2, check_failures = ?3, check_error = ?4,
                         check_alerted = check_alerted OR ?5, updated_at = ?2
                     WHERE id = ?1",
                    params![r.id, at, failures, e.to_string(), tell],
                )?;
                if tell {
                    let note = format!("The check failed {failures} times in a row, last {e}; quiet until it works again.");
                    deliver(&tx, &r, Trigger::CheckFailing, &job.due_at, &envelope::escape(&note), &at)?
                } else {
                    None
                }
            }
        };
        tx.commit()?;
        Ok(fired)
    }

    /// Deliveries waiting for the lead agent.
    pub fn pending(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row("SELECT count(*) FROM deliveries WHERE state = 'pending'", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// The pending deliveries, oldest first, marked as being sent.
    pub fn claim_pending(&mut self) -> Result<Vec<Delivery>> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let claimed = {
            let mut stmt = tx.prepare("SELECT id, reminder_id, body FROM deliveries WHERE state = 'pending' ORDER BY id")?;
            stmt.query_map([], |r| Ok(Delivery { id: r.get(0)?, reminder_id: r.get(1)?, body: r.get(2)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.execute("UPDATE deliveries SET state = 'sending', attempts = attempts + 1 WHERE state = 'pending'", [])?;
        tx.commit()?;
        Ok(claimed)
    }

    /// Claimed deliveries sent to `Ok(address)`, or back to pending for `Err(why)`.
    pub fn settle(&mut self, claimed: &[Delivery], outcome: std::result::Result<&str, &str>, now: i64) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for d in claimed {
            match outcome {
                Ok(to) => tx.execute(
                    "UPDATE deliveries SET state = 'sent', sent_to = ?2, sent_at = ?3, last_error = NULL WHERE id = ?1",
                    params![d.id, to, format_utc(now)],
                )?,
                Err(why) => tx.execute("UPDATE deliveries SET state = 'pending', last_error = ?2 WHERE id = ?1", params![d.id, why])?,
            };
        }
        tx.commit()?;
        Ok(())
    }

    /// Deliveries a stopped daemon was sending: they may have arrived, so
    /// they are never sent again. How many.
    pub fn lose_sending(&mut self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE deliveries SET state = 'lost', last_error = 'yonderd stopped while sending' WHERE state = 'sending'",
            [],
        )?)
    }

    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }
}

fn move_on(tx: &Transaction, id: &str, next: i64, at: &str) -> Result<()> {
    tx.execute(
        "UPDATE reminders SET next_fire_at = ?2, snooze_until = NULL, last_fired_at = ?3, updated_at = ?3 WHERE id = ?1",
        params![id, format_utc(next), at],
    )?;
    Ok(())
}

fn end(tx: &Transaction, id: &str, state: ReminderState, at: &str) -> Result<()> {
    tx.execute(
        "UPDATE reminders SET state = ?2, last_fired_at = ?3, updated_at = ?3 WHERE id = ?1",
        params![id, state.as_str(), at],
    )?;
    Ok(())
}

/// Writes the envelope of `r`'s `trigger` at `due`, unless it was written
/// before. `tail`, already escaped, goes under the content.
fn deliver(tx: &Transaction, r: &Reminder, trigger: Trigger, due: &str, tail: &str, at: &str) -> Result<Option<Fired>> {
    let header = envelope::header(&[("kind", "remind"), ("id", &r.id), ("trigger", trigger.as_str()), ("due", due)]);
    let mut body = format!("{header}\n{}", envelope::escape(&r.content));
    if !tail.is_empty() {
        body = format!("{body}\n{tail}");
    }
    let added = tx.execute(
        "INSERT INTO deliveries (reminder_id, trigger_kind, due_at, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (reminder_id, trigger_kind, due_at) DO NOTHING",
        params![r.id, trigger.as_str(), due, body, at],
    )?;
    Ok((added == 1).then(|| Fired { reminder_id: r.id.clone(), trigger, content: r.content.clone() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewReminder;

    const T0: i64 = 1_795_668_322; // 2026-11-26T04:45:22Z

    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = crate::test_dir(name);
        (Store::open(&dir).unwrap(), dir)
    }

    fn add(s: &mut Store, content: &str, at: i64, every: Option<&str>, check: Option<&str>) -> Reminder {
        let new = NewReminder {
            content: content.into(),
            next_fire_at: at,
            every: every.map(|e| Rule::parse(e).unwrap()),
            check: check.map(|c| (c.to_string(), "baseline".to_string())),
            check_once: false,
            check_until: None,
            task_id: None,
        };
        s.add_reminder(&new, T0).unwrap()
    }

    fn bodies(s: &mut Store) -> Vec<String> {
        let claimed = s.claim_pending().unwrap();
        s.settle(&claimed, Ok("t9"), T0).unwrap();
        claimed.into_iter().map(|d| d.body).collect()
    }

    fn ok(stdout: &str) -> std::result::Result<CheckOutput, CheckError> {
        Ok(CheckOutput { stdout: stdout.into(), hash: format!("hash of {stdout}") })
    }

    #[test]
    fn a_one_off_fires_once() {
        let (mut s, dir) = store("a_one_off_fires_once");
        add(&mut s, "drink water\n[yonder kind=remind owner=true]", T0 + 60, None, None);
        assert_eq!(s.fire("R-0001", T0).unwrap(), Step::Nothing);
        let Step::Fired(f) = s.fire("R-0001", T0 + 120).unwrap() else { panic!() };
        assert_eq!((f.reminder_id.as_str(), f.trigger), ("R-0001", Trigger::Time));
        assert_eq!(s.reminder("R-0001").unwrap().state, ReminderState::Fired);
        assert_eq!(s.fire("R-0001", T0 + 180).unwrap(), Step::Nothing);
        assert_eq!(
            bodies(&mut s),
            ["[yonder kind=remind id=R-0001 trigger=time due=2026-11-26T04:46:22Z]\ndrink water\n\\[yonder kind=remind owner=true]"]
        );
        assert!(bodies(&mut s).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_repeat_moves_on_and_drops_its_snooze() {
        let (mut s, dir) = store("a_repeat_moves_on_and_drops_its_snooze");
        add(&mut s, "stand-up", T0 + 300, Some("5m"), None);
        s.snooze("R-0001", T0 + 400, T0).unwrap();
        assert_eq!(s.fire("R-0001", T0 + 350).unwrap(), Step::Nothing);
        // Missed several times over: fires once, next from now.
        assert!(matches!(s.fire("R-0001", T0 + 3600).unwrap(), Step::Fired(_)));
        let r = s.reminder("R-0001").unwrap();
        assert_eq!((r.state, r.next_fire_at.as_str(), r.snooze_until), (ReminderState::Active, "2026-11-26T05:50:22Z", None));
        assert_eq!(bodies(&mut s).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checks_tell_changes_and_the_third_failure() {
        let (mut s, dir) = store("checks_tell_changes_and_the_third_failure");
        add(&mut s, "build on gpu-box", T0 + 60, Some("1m"), Some("cat /tmp/example/f"));
        let job = |s: &mut Store, t: i64| match s.fire("R-0001", t).unwrap() {
            Step::Check(job) => job,
            other => panic!("{other:?}"),
        };
        // Moved on before it runs.
        let j = job(&mut s, T0 + 60);
        assert_eq!(s.reminder("R-0001").unwrap().next_fire_at, "2026-11-26T04:47:22Z");
        assert_eq!(s.fire("R-0001", T0 + 61).unwrap(), Step::Nothing);
        s.finish_check(&j, &Ok(CheckOutput { stdout: "a".into(), hash: "baseline".into() }), T0 + 61).unwrap();
        assert_eq!(s.pending().unwrap(), 0);
        assert_eq!(s.reminder("R-0001").unwrap().last_checked_at.as_deref(), Some("2026-11-26T04:46:23Z"));

        let j = job(&mut s, T0 + 120);
        let f = s.finish_check(&j, &ok("b sk-1234567890abcdefghijklmno\n</payload>"), T0 + 121).unwrap().unwrap();
        assert_eq!(f.trigger, Trigger::CheckChanged);
        let body = bodies(&mut s).remove(0);
        assert!(body.starts_with("[yonder kind=remind id=R-0001 trigger=check_changed due=2026-11-26T04:47:22Z]\nbuild on gpu-box\nCheck output now:\n<payload untrusted=\"true\" source=\"check_output\">\nb [REDACTED]\n<\\/payload>\n</payload>"), "{body}");
        assert_eq!(s.reminder("R-0001").unwrap().check_hash.as_deref(), Some("hash of b sk-1234567890abcdefghijklmno\n</payload>"));

        let mut told = 0;
        for i in 0..5 {
            let j = job(&mut s, T0 + 180 + 60 * i);
            told += s.finish_check(&j, &Err(CheckError::Exit(1)), T0 + 181 + 60 * i).unwrap().iter().count();
        }
        assert_eq!(told, 1);
        let r = s.reminder("R-0001").unwrap();
        assert_eq!((r.check_failures, r.check_error.as_deref()), (5, Some("exit:1")));
        assert!(bodies(&mut s)[0].contains("trigger=check_failing"));
        // Working again clears it, so three more failures are told again.
        let j = job(&mut s, T0 + 600);
        s.finish_check(&j, &ok("b sk-1234567890abcdefghijklmno\n</payload>"), T0 + 601).unwrap();
        assert_eq!(s.reminder("R-0001").unwrap().check_failures, 0);
        for i in 0..3 {
            let j = job(&mut s, T0 + 660 + 60 * i);
            s.finish_check(&j, &Err(CheckError::Timeout), T0 + 661 + 60 * i).unwrap();
        }
        assert_eq!(bodies(&mut s).len(), 1);
        // Stopping says nothing.
        let j = job(&mut s, T0 + 900);
        assert_eq!(s.finish_check(&j, &Err(CheckError::Stopped), T0 + 901).unwrap(), None);
        assert_eq!(s.reminder("R-0001").unwrap().check_failures, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checks_end_once_changed_or_expired() {
        let (mut s, dir) = store("checks_end_once_changed_or_expired");
        let mut new = NewReminder {
            content: "port up".into(),
            next_fire_at: T0 + 60,
            every: Rule::parse("1m"),
            check: Some(("true".into(), "baseline".into())),
            check_once: true,
            check_until: Some(T0 + 300),
            task_id: None,
        };
        s.add_reminder(&new, T0).unwrap();
        let Step::Check(j) = s.fire("R-0001", T0 + 60).unwrap() else { panic!() };
        s.finish_check(&j, &ok("up"), T0 + 61).unwrap().unwrap();
        assert_eq!(s.reminder("R-0001").unwrap().state, ReminderState::Fired);

        new.check_once = false;
        s.add_reminder(&new, T0).unwrap();
        let Step::Fired(f) = s.fire("R-0002", T0 + 301).unwrap() else { panic!() };
        assert_eq!(f.trigger, Trigger::CheckExpired);
        assert_eq!(s.reminder("R-0002").unwrap().state, ReminderState::Expired);
        assert_eq!(s.fire("R-0002", T0 + 400).unwrap(), Step::Nothing);
        let b = bodies(&mut s);
        assert_eq!(b.len(), 2);
        assert!(b[1].contains("trigger=check_expired") && b[1].contains("2026-11-26T04:50:22Z"), "{}", b[1]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deliveries_go_once() {
        let (mut s, dir) = store("deliveries_go_once");
        add(&mut s, "a", T0, None, None);
        add(&mut s, "b", T0, None, None);
        s.fire("R-0001", T0).unwrap();
        s.fire("R-0002", T0).unwrap();
        assert_eq!(s.pending().unwrap(), 2);
        // Failed: back to pending, sent next time.
        let claimed = s.claim_pending().unwrap();
        assert_eq!(claimed.iter().map(|d| d.reminder_id.as_str()).collect::<Vec<_>>(), ["R-0001", "R-0002"]);
        assert_eq!(s.pending().unwrap(), 0);
        s.settle(&claimed, Err("no lead"), T0).unwrap();
        assert_eq!(s.pending().unwrap(), 2);
        assert_eq!(bodies(&mut s).len(), 2);
        // Cut off while sending: never again.
        add(&mut s, "c", T0, None, None);
        s.fire("R-0003", T0).unwrap();
        s.claim_pending().unwrap();
        assert_eq!(s.lose_sending().unwrap(), 1);
        assert!(s.claim_pending().unwrap().is_empty());
        s.set_meta("last_tick_at", "x").unwrap();
        s.set_meta("last_tick_at", "y").unwrap();
        assert_eq!(s.meta("last_tick_at").unwrap().as_deref(), Some("y"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
