//! The ledger: `ledger.db` in the state directory, SQLite in WAL mode so
//! the CLI and the daemon can use it at once. Changes run in immediate
//! transactions: a state is read and changed under one write lock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use yonder_model::{Reminder, ReminderState, Task, TaskState};

use crate::redact::redact;
use crate::when::{Rule, format_utc};

/// The schema, one step per ledger version; `PRAGMA user_version` says how
/// many are applied. Only ever append.
const MIGRATIONS: &[&str] = &[include_str!("schema.sql")];

const TASK_COLS: &str = "id, title, state, source, created_at, updated_at";
const REMINDER_COLS: &str =
    "id, content, state, next_fire_at, every_spec, snooze_until, check_cmd, check_hash, task_id, created_at, updated_at";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no {0} {1}")]
    NotFound(&'static str, String),
    #[error("{id} is {state}, not {want}")]
    WrongState { id: String, state: String, want: &'static str },
    #[error("{0} is empty")]
    Empty(&'static str),
    #[error("a check needs a repeat rule")]
    CheckNeedsEvery,
    #[error("no state directory: set YONDER_STATE_DIR or HOME")]
    NoStateDir,
    #[error("state directory {0}: {1}")]
    Dir(PathBuf, std::io::Error),
    #[error("the ledger is version {0}, newer than this yonder knows")]
    TooNew(i64),
    #[error("ledger: {0}")]
    Sql(#[from] rusqlite::Error),
}

impl StoreError {
    /// A word that names the error for scripts; stays the same across versions.
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::NotFound(..) => "not_found",
            StoreError::WrongState { .. } => "wrong_state",
            StoreError::Empty(_) => "empty",
            StoreError::CheckNeedsEvery => "check_needs_every",
            StoreError::NoStateDir | StoreError::Dir(..) => "state_dir",
            StoreError::TooNew(_) => "ledger_too_new",
            StoreError::Sql(_) => "ledger",
        }
    }
}

type Result<T> = std::result::Result<T, StoreError>;

/// `$YONDER_STATE_DIR`, else `~/.local/state/yonder`.
pub fn state_dir() -> Result<PathBuf> {
    state_dir_from(std::env::var_os("YONDER_STATE_DIR"), std::env::var_os("HOME")).ok_or(StoreError::NoStateDir)
}

pub fn state_dir_from(dir: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> Option<PathBuf> {
    match (dir.filter(|d| !d.is_empty()), home.filter(|h| !h.is_empty())) {
        (Some(dir), _) => Some(dir.into()),
        (None, Some(home)) => Some(Path::new(&home).join(".local/state/yonder")),
        (None, None) => None,
    }
}

/// A reminder to add; times are Unix seconds.
#[derive(Clone, Debug)]
pub struct NewReminder {
    pub content: String,
    pub next_fire_at: i64,
    pub every: Option<Rule>,
    /// The check command and the hash of its first output.
    pub check: Option<(String, String)>,
    pub task_id: Option<String>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// The ledger in the default state directory.
    pub fn open_default() -> Result<Store> {
        Store::open(&state_dir()?)
    }

    /// The ledger in `dir`, made (only its owner can read it) if need be.
    pub fn open(dir: &Path) -> Result<Store> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| StoreError::Dir(dir.into(), e))?;
        let mut conn = Connection::open(dir.join("ledger.db"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    pub fn add_task(&mut self, title: &str, source: Option<&str>, now: i64) -> Result<Task> {
        let title = redact(title.trim());
        if title.is_empty() {
            return Err(StoreError::Empty("the title"));
        }
        let now = format_utc(now);
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = next_id(&tx, "tasks", "T")?;
        tx.execute(
            "INSERT INTO tasks (id, title, state, source, created_at, updated_at) VALUES (?1, ?2, 'open', ?3, ?4, ?4)",
            params![id, title, source.filter(|s| !s.is_empty()), now],
        )?;
        let task = get_task(&tx, &id)?;
        tx.commit()?;
        Ok(task)
    }

    pub fn task(&self, id: &str) -> Result<Task> {
        get_task(&self.conn, id)
    }

    /// Open tasks, or all of them; oldest first.
    pub fn tasks(&self, all: bool) -> Result<Vec<Task>> {
        let filter = if all { "" } else { "WHERE state = 'open'" };
        let mut stmt = self.conn.prepare(&format!("SELECT {TASK_COLS} FROM tasks {filter} ORDER BY created_at, id"))?;
        let tasks = stmt.query_map([], task_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(tasks)
    }

    /// An open task made `done` or `cancelled`.
    pub fn close_task(&mut self, id: &str, to: TaskState, now: i64) -> Result<Task> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = get_task(&tx, id)?;
        if task.state != TaskState::Open {
            return Err(StoreError::WrongState { id: task.id, state: task.state.to_string(), want: "open" });
        }
        tx.execute(
            "UPDATE tasks SET state = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, to.as_str(), format_utc(now)],
        )?;
        let task = get_task(&tx, id)?;
        tx.commit()?;
        Ok(task)
    }

    pub fn add_reminder(&mut self, new: &NewReminder, now: i64) -> Result<Reminder> {
        let content = redact(new.content.trim());
        if content.is_empty() {
            return Err(StoreError::Empty("the reminder"));
        }
        if new.check.is_some() && new.every.is_none() {
            return Err(StoreError::CheckNeedsEvery);
        }
        let now = format_utc(now);
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(task_id) = &new.task_id {
            get_task(&tx, task_id)?;
        }
        let id = next_id(&tx, "reminders", "R")?;
        let (cmd, hash) = new.check.clone().unzip();
        tx.execute(
            "INSERT INTO reminders (id, content, state, next_fire_at, every_spec, check_cmd, check_hash, task_id, created_at, updated_at)
             VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                id,
                content,
                format_utc(new.next_fire_at),
                new.every.map(|r| r.spec()),
                cmd,
                hash,
                new.task_id,
                now
            ],
        )?;
        let reminder = get_reminder(&tx, &id)?;
        tx.commit()?;
        Ok(reminder)
    }

    pub fn reminder(&self, id: &str) -> Result<Reminder> {
        get_reminder(&self.conn, id)
    }

    /// Active reminders, or all of them; soonest first.
    pub fn reminders(&self, all: bool) -> Result<Vec<Reminder>> {
        let filter = if all { "" } else { "WHERE state = 'active'" };
        self.query_reminders(filter, [])
    }

    /// Active reminders due at `now`, their snooze over. Changes nothing.
    pub fn due(&self, now: i64) -> Result<Vec<Reminder>> {
        self.query_reminders(
            "WHERE state = 'active' AND next_fire_at <= ?1 AND (snooze_until IS NULL OR snooze_until <= ?1)",
            [format_utc(now)],
        )
    }

    /// An active reminder kept quiet until `until`.
    pub fn snooze(&mut self, id: &str, until: i64, now: i64) -> Result<Reminder> {
        self.change_active(id, "snooze_until = ?2", format_utc(until), now)
    }

    pub fn cancel_reminder(&mut self, id: &str, now: i64) -> Result<Reminder> {
        self.change_active(id, "state = ?2", ReminderState::Cancelled.as_str().into(), now)
    }

    fn change_active(&mut self, id: &str, set: &str, value: String, now: i64) -> Result<Reminder> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let r = get_reminder(&tx, id)?;
        if r.state != ReminderState::Active {
            return Err(StoreError::WrongState { id: r.id, state: r.state.to_string(), want: "active" });
        }
        tx.execute(
            &format!("UPDATE reminders SET {set}, updated_at = ?3 WHERE id = ?1"),
            params![id, value, format_utc(now)],
        )?;
        let r = get_reminder(&tx, id)?;
        tx.commit()?;
        Ok(r)
    }

    fn query_reminders(&self, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Reminder>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {REMINDER_COLS} FROM reminders {filter}
             ORDER BY max(next_fire_at, coalesce(snooze_until, next_fire_at)), id"
        ))?;
        let reminders = stmt.query_map(args, reminder_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(reminders)
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version = |c: &Connection| c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0));
    let latest = MIGRATIONS.len() as i64;
    if version(conn)? == latest {
        return Ok(());
    }
    loop {
        // Read again under the write lock: another process may be migrating.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let v = version(&tx)?;
        if v > latest {
            return Err(StoreError::TooNew(v));
        }
        if v == latest {
            return Ok(());
        }
        tx.execute_batch(MIGRATIONS[v as usize])?;
        tx.pragma_update(None, "user_version", v + 1)?;
        tx.commit()?;
    }
}

/// The next `T-0001`-style id of `table`.
fn next_id(conn: &Connection, table: &str, prefix: &str) -> Result<String> {
    let last: i64 = conn.query_row(
        &format!("SELECT coalesce(max(CAST(substr(id, 3) AS INTEGER)), 0) FROM {table}"),
        [],
        |r| r.get(0),
    )?;
    Ok(format!("{prefix}-{:04}", last + 1))
}

fn get_task(conn: &Connection, id: &str) -> Result<Task> {
    conn.query_row(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), [id], task_row)
        .optional()?
        .ok_or_else(|| StoreError::NotFound("task", id.into()))
}

fn get_reminder(conn: &Connection, id: &str) -> Result<Reminder> {
    conn.query_row(&format!("SELECT {REMINDER_COLS} FROM reminders WHERE id = ?1"), [id], reminder_row)
        .optional()?
        .ok_or_else(|| StoreError::NotFound("reminder", id.into()))
}

fn word<T>(row: &Row, i: usize, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let s: String = row.get(i)?;
    parse(&s).ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(i, Type::Text, format!("unknown state {s:?}").into()))
}

fn task_row(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        title: r.get(1)?,
        state: word(r, 2, TaskState::parse)?,
        source: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

fn reminder_row(r: &Row) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: r.get(0)?,
        content: r.get(1)?,
        state: word(r, 2, ReminderState::parse)?,
        next_fire_at: r.get(3)?,
        every: r.get(4)?,
        snooze_until: r.get(5)?,
        check_cmd: r.get(6)?,
        check_hash: r.get(7)?,
        task_id: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_795_668_322; // 2026-11-26T04:45:22Z

    fn store(name: &str) -> (Store, PathBuf) {
        let dir = crate::test_dir(name);
        (Store::open(&dir).unwrap(), dir)
    }

    fn once(content: &str, at: i64) -> NewReminder {
        NewReminder { content: content.into(), next_fire_at: at, every: None, check: None, task_id: None }
    }

    fn count(s: &Store, table: &str) -> i64 {
        s.conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn opens_in_wal_mode() {
        let (s, dir) = store("opens_in_wal_mode");
        let pragma = |name: &str| s.conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, rusqlite::types::Value>(0)).unwrap();
        use rusqlite::types::Value::{Integer, Text};
        assert_eq!(pragma("journal_mode"), Text("wal".into()));
        assert_eq!(pragma("foreign_keys"), Integer(1));
        assert_eq!(pragma("synchronous"), Integer(1)); // NORMAL
        assert_eq!(pragma("user_version"), Integer(MIGRATIONS.len() as i64));
        drop(s);
        // Opening again keeps what is there.
        let mut s = Store::open(&dir).unwrap();
        s.add_task("again", None, T0).unwrap();
        drop(s);
        assert_eq!(Store::open(&dir).unwrap().tasks(true).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_a_newer_ledger() {
        let (s, dir) = store("refuses_a_newer_ledger");
        s.conn.pragma_update(None, "user_version", 99).unwrap();
        drop(s);
        assert!(matches!(Store::open(&dir), Err(StoreError::TooNew(99))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tasks_open_and_close() {
        let (mut s, dir) = store("tasks_open_and_close");
        let a = s.add_task("deploy nova on gpu-box", Some("hc:nova"), T0).unwrap();
        let b = s.add_task("  back up homelab ", None, T0 + 1).unwrap();
        assert_eq!((a.id.as_str(), b.id.as_str()), ("T-0001", "T-0002"));
        assert_eq!(a.state, TaskState::Open);
        assert_eq!(a.source.as_deref(), Some("hc:nova"));
        assert_eq!(b.title, "back up homelab");
        assert_eq!(a.created_at, "2026-11-26T04:45:22Z");

        let done = s.close_task("T-0001", TaskState::Done, T0 + 60).unwrap();
        assert_eq!(done.state, TaskState::Done);
        assert_eq!(done.updated_at, "2026-11-26T04:46:22Z");
        let open: Vec<_> = s.tasks(false).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(open, ["T-0002"]);
        let all: Vec<_> = s.tasks(true).unwrap().into_iter().map(|t| (t.id, t.state)).collect();
        assert_eq!(all, [("T-0001".into(), TaskState::Done), ("T-0002".into(), TaskState::Open)]);

        // A closed task stays as it was.
        let again = s.close_task("T-0001", TaskState::Cancelled, T0 + 120);
        assert!(matches!(again, Err(StoreError::WrongState { .. })), "{again:?}");
        assert_eq!(s.task("T-0001").unwrap(), done);
        assert!(matches!(s.close_task("T-0009", TaskState::Done, T0), Err(StoreError::NotFound("task", _))));
        assert!(matches!(s.add_task("  ", None, T0), Err(StoreError::Empty(_))));
        assert_eq!(count(&s, "tasks"), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn redacts_what_it_stores() {
        let (mut s, dir) = store("redacts_what_it_stores");
        let t = s.add_task("rotate sk-1234567890abcdefghijklmno", None, T0).unwrap();
        assert_eq!(t.title, "rotate [REDACTED]");
        let r = s.add_reminder(&once("HF_TOKEN=hf_abcdefgh expires", T0 + 60), T0).unwrap();
        assert_eq!(r.content, "HF_TOKEN=[REDACTED] expires");
        let raw: String = s.conn.query_row("SELECT title FROM tasks", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("sk-"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn due_snooze_and_cancel() {
        let (mut s, dir) = store("due_snooze_and_cancel");
        let r = s.add_reminder(&once("drink water", T0 + 120), T0).unwrap();
        assert_eq!((r.id.as_str(), r.state), ("R-0001", ReminderState::Active));
        assert_eq!(r.next_fire_at, "2026-11-26T04:47:22Z");
        let ids = |s: &Store, t| s.due(t).unwrap().into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert!(ids(&s, T0).is_empty());
        assert_eq!(ids(&s, T0 + 120), ["R-0001"]);
        assert_eq!(ids(&s, T0 + 180), ["R-0001"]);
        // Looking does not change it.
        assert_eq!(s.reminder("R-0001").unwrap(), r);

        let snoozed = s.snooze("R-0001", T0 + 3600, T0 + 10).unwrap();
        assert_eq!(snoozed.due_at(), "2026-11-26T05:45:22Z");
        assert!(ids(&s, T0 + 180).is_empty());
        assert_eq!(ids(&s, T0 + 3600), ["R-0001"]);

        s.cancel_reminder("R-0001", T0 + 20).unwrap();
        assert!(ids(&s, T0 + 7200).is_empty());
        assert!(s.reminders(false).unwrap().is_empty());
        assert_eq!(s.reminders(true).unwrap()[0].state, ReminderState::Cancelled);
        let again = s.snooze("R-0001", T0 + 9000, T0 + 30);
        assert!(matches!(again, Err(StoreError::WrongState { .. })), "{again:?}");
        assert_eq!(s.reminder("R-0001").unwrap().snooze_until.as_deref(), Some("2026-11-26T05:45:22Z"));
        assert!(matches!(s.cancel_reminder("R-0002", T0), Err(StoreError::NotFound("reminder", _))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reminders_with_rules_checks_and_tasks() {
        let (mut s, dir) = store("reminders_with_rules_checks_and_tasks");
        let task = s.add_task("watch the trainer run", None, T0).unwrap();
        let rule = Rule::parse("5m").unwrap();
        let new = NewReminder {
            content: "trainer log".into(),
            next_fire_at: rule.next_after(T0).unwrap(),
            every: Some(rule),
            check: Some(("tail -1 /tmp/example/log".into(), "ab".repeat(32))),
            task_id: Some(task.id.clone()),
        };
        let r = s.add_reminder(&new, T0).unwrap();
        assert_eq!(r.every.as_deref(), Some("5m"));
        assert_eq!(r.check_cmd.as_deref(), Some("tail -1 /tmp/example/log"));
        assert_eq!(r.task_id.as_deref(), Some("T-0001"));

        // A check without a rule, or a task that is not there: nothing written.
        let no_rule = NewReminder { every: None, ..new.clone() };
        assert!(matches!(s.add_reminder(&no_rule, T0), Err(StoreError::CheckNeedsEvery)));
        let no_task = NewReminder { task_id: Some("T-0042".into()), ..new.clone() };
        assert!(matches!(s.add_reminder(&no_task, T0), Err(StoreError::NotFound("task", _))));
        assert_eq!(count(&s, "reminders"), 1);

        // Soonest first.
        s.add_reminder(&once("sooner", T0 + 60), T0).unwrap();
        let order: Vec<_> = s.reminders(false).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(order, ["R-0002", "R-0001"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finds_the_state_dir() {
        let some = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(state_dir_from(some("/tmp/example/s"), some("/tmp/example")), Some("/tmp/example/s".into()));
        assert_eq!(state_dir_from(some(""), some("/tmp/example")), Some("/tmp/example/.local/state/yonder".into()));
        assert_eq!(state_dir_from(None, None), None);
    }
}
