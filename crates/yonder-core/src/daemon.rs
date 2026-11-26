//! yonderd's rounds: fire what is due, run checks (two at a time, beside
//! the round, never holding it up), and send what waits to the lead agent
//! in one message of at most `BATCH_BYTES`. Once `stop` is set nothing new
//! starts, and no step waits long, so yonderd ends within two seconds.
//! Also the lock that keeps yonderd to one instance.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use crate::check_exec::{self, CheckError, CheckOutput};
use crate::firing::{BATCH_BYTES, CheckJob, Fired, Outcome, SEPARATOR, Step, Trigger};
use crate::hc::{Lead, SendError};
use crate::store::{Store, StoreError};
use crate::when::format_utc;

/// Checks running at once.
pub const MAX_CHECKS: usize = 2;
/// The longest yonderd waits for another writer of the ledger.
pub const LEDGER_WAIT: Duration = Duration::from_secs(1);

/// Somewhere to tell the owner on this computer.
pub trait Notifier {
    fn notify(&mut self, title: &str, text: &str);
}

/// macOS notifications through osascript; nothing elsewhere or when off.
pub struct LocalNotifier {
    pub enabled: bool,
    pub stop: &'static AtomicBool,
}

impl Notifier for LocalNotifier {
    fn notify(&mut self, title: &str, text: &str) {
        if !self.enabled || !cfg!(target_os = "macos") {
            return;
        }
        // Text as arguments, never inside the script.
        let mut command = std::process::Command::new("/usr/bin/osascript");
        command
            .args(["-e", "on run argv", "-e", "display notification (item 2 of argv) with title (item 1 of argv)", "-e", "end run"])
            .args([title, text])
            .stderr(std::process::Stdio::null());
        let _ = check_exec::capture(command, std::time::Duration::from_secs(5), 4096, self.stop, false);
    }
}

type Done = (CheckJob, Result<CheckOutput, CheckError>);

pub struct Daemon<L: Lead, N: Notifier> {
    pub store: Store,
    lead: L,
    notifier: N,
    log: Box<dyn FnMut(&str)>,
    stop: &'static AtomicBool,
    /// Reminders whose check is running.
    running: HashSet<String>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    /// Why the last delivery could not go, logged once until it changes.
    trouble: Option<String>,
    /// The most bytes in one message.
    batch_bytes: usize,
}

impl<L: Lead, N: Notifier> Daemon<L, N> {
    pub fn new(mut store: Store, lead: L, notifier: N, stop: &'static AtomicBool, mut log: Box<dyn FnMut(&str)>) -> Result<Self, StoreError> {
        store.set_busy_timeout(LEDGER_WAIT)?;
        let lost = store.lose_sending()?;
        if lost > 0 {
            log(&format!("{lost} deliveries were being sent when yonderd stopped; not sending them again"));
        }
        let (done_tx, done_rx) = channel();
        let batch_bytes = BATCH_BYTES;
        Ok(Daemon { store, lead, notifier, log, stop, running: HashSet::new(), done_tx, done_rx, trouble: None, batch_bytes })
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// One round at `now`. With `wait`, the checks it starts finish in it
    /// too (`yonderd --once`); else they report in later rounds.
    pub fn tick(&mut self, now: i64, wait: bool) -> Result<(), StoreError> {
        self.settle_checks(now, false)?;
        loop {
            let deferred = self.fire_due(now)?;
            if !wait || self.stopping() || (self.running.is_empty() && !deferred) {
                break;
            }
            self.settle_checks(now, true)?;
        }
        if self.stopping() {
            return Ok(());
        }
        self.deliver(now)?;
        self.store.set_meta("last_tick_at", &format_utc(now))
    }

    /// Once `stop` is set: waits for the running checks, which end at once,
    /// keeping nothing of them. Their reminders moved on already; a change
    /// they saw is seen again, against the old baseline, next time.
    pub fn finish(&mut self) {
        while !self.running.is_empty() {
            match self.done_rx.recv() {
                Ok((job, _)) => self.running.remove(&job.id),
                Err(_) => break,
            };
        }
    }

    /// Fires what is due; whether a check had to wait for a free slot.
    fn fire_due(&mut self, now: i64) -> Result<bool, StoreError> {
        let mut deferred = false;
        for r in self.store.due(now)? {
            if self.stopping() {
                break;
            }
            if self.running.contains(&r.id) {
                continue;
            }
            if r.check_cmd.is_some() && self.running.len() >= MAX_CHECKS {
                deferred = true;
                continue;
            }
            match self.store.fire(&r.id, now)? {
                Step::Fired(fired) => self.tell_locally(&fired),
                Step::Check(job) => self.start(job),
                Step::Nothing => {}
            }
        }
        Ok(deferred)
    }

    fn start(&mut self, job: CheckJob) {
        self.running.insert(job.id.clone());
        let (done, stop) = (self.done_tx.clone(), self.stop);
        std::thread::spawn(move || {
            let run = || check_exec::run_until(&job.cmd, check_exec::TIMEOUT, check_exec::MAX_OUTPUT, stop);
            let result = std::panic::catch_unwind(run)
                .unwrap_or_else(|_| Err(CheckError::Spawn(std::io::Error::other("the check runner panicked"))));
            let _ = done.send((job, result));
        });
    }

    /// Records finished checks: one, waiting for it, with `block`; else
    /// those done by now.
    fn settle_checks(&mut self, now: i64, block: bool) -> Result<(), StoreError> {
        loop {
            let done = if block { self.done_rx.recv().ok() } else { self.done_rx.try_recv().ok() };
            let Some((job, result)) = done else { return Ok(()) };
            self.running.remove(&job.id);
            if self.stopping() {
                continue;
            }
            match &result {
                Ok(out) => (self.log)(&format!("{} check ok, output hash {}", job.id, &out.hash[..12])),
                Err(e) => (self.log)(&format!("{} check failed: {e}", job.id)),
            }
            if let Some(fired) = self.store.finish_check(&job, &result, now)? {
                self.tell_locally(&fired);
            }
            if block {
                return Ok(());
            }
        }
    }

    fn tell_locally(&mut self, f: &Fired) {
        (self.log)(&format!("{} fired: {}", f.reminder_id, f.trigger.as_str()));
        let what = match f.trigger {
            Trigger::Time => "",
            Trigger::CheckChanged => "Changed: ",
            Trigger::CheckFailing => "Check failing: ",
            Trigger::CheckExpired => "Check over: ",
        };
        self.notifier.notify(&format!("Yonder {}", f.reminder_id), &format!("{what}{}", f.content));
    }

    /// What is pending, oldest first, in one `hc send` of at most
    /// `batch_bytes`; the rest goes in later rounds.
    fn deliver(&mut self, now: i64) -> Result<(), StoreError> {
        let waiting = self.store.pending()?;
        if waiting == 0 {
            return Ok(());
        }
        let to = match self.lead.find() {
            Ok(Some(to)) => to,
            Ok(None) => {
                self.trouble(format!("no live lead agent terminal; {waiting} waiting"));
                return Ok(());
            }
            Err(e) => {
                self.trouble(format!("{e}; {waiting} waiting"));
                return Ok(());
            }
        };
        if self.stopping() {
            return Ok(());
        }
        let (claimed, too_big) = self.store.claim_pending(self.batch_bytes)?;
        for d in &too_big {
            (self.log)(&format!("lost {}: {} bytes, more than a message holds; not sending it", d.reminder_id, d.body.len()));
        }
        if claimed.is_empty() {
            return Ok(());
        }
        let text = claimed.iter().map(|d| d.body.as_str()).collect::<Vec<_>>().join(SEPARATOR);
        let ids = claimed.iter().map(|d| d.reminder_id.as_str()).collect::<Vec<_>>().join(", ");
        match self.lead.send(&to, &text) {
            Ok(()) => {
                self.store.settle(&claimed, Outcome::Sent(&to), now)?;
                self.trouble = None;
                (self.log)(&format!("sent {ids} to {to}"));
            }
            Err(SendError::NotSent(e)) => {
                self.store.settle(&claimed, Outcome::Retry(&e), now)?;
                self.trouble(format!("{e}; {} waiting", claimed.len()));
            }
            Err(SendError::Unknown(e)) => {
                self.store.settle(&claimed, Outcome::Lost(&e), now)?;
                (self.log)(&format!("lost {ids}: {e}; they may have arrived, so they are not sent again"));
            }
        }
        Ok(())
    }

    fn trouble(&mut self, why: String) {
        if self.trouble.as_ref() != Some(&why) {
            (self.log)(&format!("not delivered: {why}"));
            self.trouble = Some(why);
        }
    }
}

/// The lock file that makes yonderd one of a kind; it holds its pid.
pub fn lock_path(state_dir: &Path) -> PathBuf {
    state_dir.join("yonderd.lock")
}

/// Held for as long as this yonderd runs.
pub struct InstanceLock(#[allow(dead_code)] File);

impl InstanceLock {
    /// The lock, or None if another yonderd holds it.
    pub fn acquire(state_dir: &Path) -> std::io::Result<Option<InstanceLock>> {
        let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lock_path(state_dir))?;
        if !flock(&file, libc::LOCK_EX)? {
            return Ok(None);
        }
        file.set_len(0)?;
        file.rewind()?;
        write!(file, "{}", std::process::id())?;
        Ok(Some(InstanceLock(file)))
    }
}

/// The pid of the running yonderd, if one runs.
pub fn running_pid(state_dir: &Path) -> std::io::Result<Option<u32>> {
    let mut file = match File::open(lock_path(state_dir)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // Free to share: nobody holds it.
    if flock(&file, libc::LOCK_SH)? {
        return Ok(None);
    }
    let mut pid = String::new();
    file.read_to_string(&mut pid)?;
    Ok(Some(pid.trim().parse().unwrap_or(0)))
}

/// Takes a lock without waiting; false if someone else has it.
fn flock(file: &File, how: libc::c_int) -> std::io::Result<bool> {
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(file.as_raw_fd(), how | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    if e.kind() == std::io::ErrorKind::WouldBlock { Ok(false) } else { Err(e) }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    static NEVER: AtomicBool = AtomicBool::new(false);
    use crate::NewReminder;
    use crate::when::Rule;

    const T0: i64 = 1_795_668_322;

    #[derive(Clone, Copy, Default, PartialEq)]
    enum Send {
        #[default]
        Works,
        /// hc refused: nothing pasted.
        Refused,
        /// Pasted, then cut short: hc timed out.
        PastedThenTimedOut,
    }

    #[derive(Clone, Default)]
    struct FakeLead {
        to: Rc<RefCell<Option<String>>>,
        /// What reached the terminal.
        sent: Rc<RefCell<Vec<(String, String)>>>,
        how: Rc<RefCell<Send>>,
    }

    impl Lead for FakeLead {
        fn find(&mut self) -> Result<Option<String>, String> {
            Ok(self.to.borrow().clone())
        }
        fn send(&mut self, to: &str, text: &str) -> Result<(), SendError> {
            let how = *self.how.borrow();
            if how == Send::Refused {
                return Err(SendError::NotSent("hc send: exit:3: busy".into()));
            }
            self.sent.borrow_mut().push((to.into(), text.into()));
            match how {
                Send::PastedThenTimedOut => Err(SendError::Unknown("hc send: timeout".into())),
                _ => Ok(()),
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeNotifier(Rc<RefCell<Vec<String>>>);

    impl Notifier for FakeNotifier {
        fn notify(&mut self, title: &str, text: &str) {
            self.0.borrow_mut().push(format!("{title}: {text}"));
        }
    }

    fn daemon(name: &str) -> (Daemon<FakeLead, FakeNotifier>, FakeLead, FakeNotifier, PathBuf) {
        let dir = crate::test_dir(name);
        let (lead, notes) = (FakeLead::default(), FakeNotifier::default());
        let d = Daemon::new(Store::open(&dir).unwrap(), lead.clone(), notes.clone(), &NEVER, Box::new(|_| {})).unwrap();
        (d, lead, notes, dir)
    }

    fn remind(d: &mut Daemon<FakeLead, FakeNotifier>, content: &str, every: Option<&str>, check: Option<(&str, &str)>) {
        let new = NewReminder {
            content: content.into(),
            next_fire_at: T0 + 60,
            every: every.and_then(Rule::parse),
            check: check.map(|(c, h)| (c.to_string(), h.to_string())),
            check_once: false,
            check_until: None,
            task_id: None,
        };
        d.store.add_reminder(&new, T0).unwrap();
    }

    #[test]
    fn sends_what_is_due_in_one_message() {
        let (mut d, lead, notes, dir) = daemon("sends_what_is_due_in_one_message");
        *lead.to.borrow_mut() = Some("t9".into());
        remind(&mut d, "drink water", None, None);
        remind(&mut d, "stand-up", Some("5m"), None);
        d.tick(T0, true).unwrap();
        assert!(lead.sent.borrow().is_empty());
        d.tick(T0 + 60, true).unwrap();
        let sent = lead.sent.borrow().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "t9");
        let segments: Vec<_> = sent[0].1.split("\n\n").collect();
        assert!(segments[0].starts_with("[yonder kind=remind id=R-0001 trigger=time due=2026-11-26T04:46:22Z]\ndrink water"));
        assert!(segments[1].starts_with("[yonder kind=remind id=R-0002 trigger=time"));
        assert_eq!(*notes.0.borrow(), ["Yonder R-0001: drink water", "Yonder R-0002: stand-up"]);
        d.tick(T0 + 120, true).unwrap();
        assert_eq!(lead.sent.borrow().len(), 1);
        assert_eq!(d.store.meta("last_tick_at").unwrap().as_deref(), Some("2026-11-26T04:47:22Z"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn waits_for_the_lead_and_sends_once() {
        let (mut d, lead, notes, dir) = daemon("waits_for_the_lead_and_sends_once");
        remind(&mut d, "drink water", None, None);
        d.tick(T0 + 60, true).unwrap();
        // Told here even with no lead agent.
        assert_eq!(notes.0.borrow().len(), 1);
        assert_eq!(d.store.pending().unwrap(), 1);
        *lead.to.borrow_mut() = Some("t9".into());
        *lead.how.borrow_mut() = Send::Refused;
        d.tick(T0 + 75, true).unwrap();
        assert_eq!(d.store.pending().unwrap(), 1);
        *lead.how.borrow_mut() = Send::Works;
        d.tick(T0 + 90, true).unwrap();
        d.tick(T0 + 105, true).unwrap();
        assert_eq!(lead.sent.borrow().len(), 1);
        assert_eq!(d.store.pending().unwrap(), 0);
        assert_eq!(notes.0.borrow().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_may_have_arrived_is_not_sent_again() {
        let (mut d, lead, _, dir) = daemon("what_may_have_arrived_is_not_sent_again");
        *lead.to.borrow_mut() = Some("t9".into());
        *lead.how.borrow_mut() = Send::PastedThenTimedOut;
        remind(&mut d, "drink water", None, None);
        d.tick(T0 + 60, true).unwrap();
        assert_eq!(lead.sent.borrow().len(), 1);
        assert_eq!((d.store.pending().unwrap(), d.store.lost().unwrap()), (0, 1));
        *lead.how.borrow_mut() = Send::Works;
        for t in [75, 90, 105] {
            d.tick(T0 + t, true).unwrap();
        }
        assert_eq!(lead.sent.borrow().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_backlog_goes_in_rounds_each_once() {
        let (mut d, lead, _, dir) = daemon("a_backlog_goes_in_rounds_each_once");
        d.batch_bytes = 460;
        for i in 1..=7 {
            // R-0003 is too big for any message.
            let content = if i == 3 { "y".repeat(500) } else { format!("{i}{}", "x".repeat(150)) };
            remind(&mut d, &content, None, None);
        }
        d.tick(T0 + 60, true).unwrap();
        assert_eq!(d.store.pending().unwrap(), 7);
        let mut rounds = 1;
        while d.store.pending().unwrap() > 0 {
            let before = lead.sent.borrow().len();
            *lead.to.borrow_mut() = Some("t9".into());
            d.tick(T0 + 60 + 15 * rounds, true).unwrap();
            assert_eq!(lead.sent.borrow().len(), before + 1, "one send a round");
            rounds += 1;
            assert!(rounds < 10);
        }
        // Nobody was there the first round.
        let sent = lead.sent.borrow();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|(_, t)| t.len() <= 460), "{sent:?}");
        let all: String = sent.iter().map(|(_, t)| t.as_str()).collect();
        for i in [1, 2, 4, 5, 6, 7] {
            assert_eq!(all.matches(&format!("id=R-000{i} ")).count(), 1, "R-000{i}");
        }
        assert_eq!(all.matches("id=R-0003 ").count(), 0);
        assert_eq!(d.store.lost().unwrap(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_stopped_daemon_starts_nothing() {
        static STOPPED: AtomicBool = AtomicBool::new(true);
        let dir = crate::test_dir("a_stopped_daemon_starts_nothing");
        let lead = FakeLead::default();
        *lead.to.borrow_mut() = Some("t9".into());
        let mut d = Daemon::new(Store::open(&dir).unwrap(), lead.clone(), FakeNotifier::default(), &STOPPED, Box::new(|_| {})).unwrap();
        remind(&mut d, "drink water", None, None);
        remind(&mut d, "port up", Some("1m"), Some(("true", "x")));
        d.tick(T0 + 60, true).unwrap();
        assert!(lead.sent.borrow().is_empty() && d.running.is_empty());
        assert_eq!(d.store.due(T0 + 60).unwrap().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn waits_at_most_a_second_for_the_ledger() {
        let (mut d, _, _, dir) = daemon("waits_at_most_a_second_for_the_ledger");
        remind(&mut d, "drink water", None, None);
        let other = rusqlite::Connection::open(dir.join("ledger.db")).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(d.tick(T0 + 60, true), Err(StoreError::Sql(_))));
        assert!(start.elapsed() < Duration::from_millis(1800), "{:?}", start.elapsed());
        other.execute_batch("ROLLBACK").unwrap();
        d.tick(T0 + 60, true).unwrap();
        assert_eq!(d.store.pending().unwrap(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn runs_checks_two_at_a_time() {
        let (mut d, lead, _, dir) = daemon("runs_checks_two_at_a_time");
        *lead.to.borrow_mut() = Some("t9".into());
        for i in 0..3 {
            remind(&mut d, &format!("check {i}"), Some("1m"), Some(("sleep 0.3; echo new", "old")));
        }
        remind(&mut d, "plain", None, None);
        // Not waiting: the plain one goes at once, two checks start.
        d.tick(T0 + 60, false).unwrap();
        assert_eq!(d.running.len(), MAX_CHECKS);
        assert_eq!(lead.sent.borrow().len(), 1);
        assert!(lead.sent.borrow()[0].1.contains("plain"));
        // Waiting: the third runs once a slot is free, all three report.
        d.tick(T0 + 60, true).unwrap();
        assert!(d.running.is_empty());
        let sent = lead.sent.borrow().iter().map(|(_, t)| t.matches("trigger=check_changed").count()).sum::<usize>();
        assert_eq!(sent, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_instance_at_a_time() {
        let dir = crate::test_dir("one_instance_at_a_time");
        assert_eq!(running_pid(&dir).unwrap(), None);
        let lock = InstanceLock::acquire(&dir).unwrap().unwrap();
        assert!(InstanceLock::acquire(&dir).unwrap().is_none());
        assert_eq!(running_pid(&dir).unwrap(), Some(std::process::id()));
        drop(lock);
        assert_eq!(running_pid(&dir).unwrap(), None);
        assert!(InstanceLock::acquire(&dir).unwrap().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
