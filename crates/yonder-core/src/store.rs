//! The ledger: `ledger.db` in Yonder's home, SQLite in WAL mode so
//! the CLI and the daemon can use it at once. Changes run in immediate
//! transactions: a state is read and changed under one write lock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use yonder_model::{BotRun, BotRunState, Reminder, ReminderState, Task, TaskState};

use crate::redact::redact;
use crate::when::{Rule, format_utc};

/// The schema, one step per ledger version; `PRAGMA user_version` says how
/// many are applied. Only ever append.
const MIGRATIONS: &[&str] = &[include_str!("schema.sql"), include_str!("schema2.sql"), include_str!("schema3.sql"), include_str!("schema4.sql"), include_str!("schema5.sql"), include_str!("schema6.sql"), SCHEMA7];

/// Version 7: when a handoff was claimed for sending, so one stuck in
/// `sending` (its sender's pid reused by another process) ends `lost`.
/// Here, not in a file of its own.
const SCHEMA7: &str = "ALTER TABLE handoffs ADD COLUMN claimed_at TEXT;";

const TASK_COLS: &str = "id, title, state, source, created_at, updated_at";
pub(crate) const REMINDER_COLS: &str = "id, content, state, next_fire_at, every_spec, snooze_until, check_cmd, check_hash, \
     check_once, check_until, last_checked_at, check_failures, check_error, last_fired_at, task_id, created_at, updated_at";

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
    #[error("{0} is {1} bytes, more than {MAX_CONTENT}")]
    TooLong(&'static str, usize),
    #[error("{0}")]
    Home(#[from] crate::home::HomeError),
    #[error("Yonder home {0}: {1}")]
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
            StoreError::TooLong(..) => "bad_input",
            StoreError::Home(crate::home::HomeError::Relative(_)) => "bad_home",
            StoreError::Home(_) | StoreError::Dir(..) => "home",
            StoreError::TooNew(_) => "ledger_too_new",
            StoreError::Sql(_) => "ledger",
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, StoreError>;

/// The longest reminder, in bytes: its envelope must fit one message.
pub const MAX_CONTENT: usize = 4096;

/// Yonder's home (`home::dir`), or why there is none.
pub fn home_dir() -> Result<PathBuf> {
    Ok(crate::home::dir()?)
}

/// A reminder to add; times are Unix seconds.
#[derive(Clone, Debug)]
pub struct NewReminder {
    pub content: String,
    pub next_fire_at: i64,
    pub every: Option<Rule>,
    /// The check command and the hash of its first output.
    pub check: Option<(String, String)>,
    /// The check ends after its first change.
    pub check_once: bool,
    /// The check stops after this; None: never.
    pub check_until: Option<i64>,
    pub task_id: Option<String>,
}

pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// The ledger in Yonder's home.
    pub fn open_default() -> Result<Store> {
        Store::open(&home_dir()?)
    }

    /// The ledger in `dir`, made (only its owner can read it) if need be.
    pub fn open(dir: &Path) -> Result<Store> {
        crate::home::ensure(dir).map_err(|e| StoreError::Dir(dir.into(), e))?;
        let mut conn = Connection::open(dir.join("ledger.db"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    /// The ledger in `dir` if there is one, only to read: nothing is made
    /// or migrated. One older than this yonder reads as if it had nothing
    /// newer in it (see `pushed`).
    pub fn open_read_only(dir: &Path) -> Result<Option<Store>> {
        let path = dir.join("ledger.db");
        if !path.is_file() {
            return Ok(None);
        }
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let store = Store { conn };
        let v = store.version()?;
        if v > MIGRATIONS.len() as i64 {
            return Err(StoreError::TooNew(v));
        }
        Ok(Some(store))
    }

    fn version(&self) -> Result<i64> {
        Ok(self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// The SHA-256 hex of the AGENTS.md last pushed to `bot` at `target`.
    pub fn pushed(&self, bot: &str, target: &str) -> Result<Option<String>> {
        if self.version()? < 3 {
            return Ok(None);
        }
        let hash = self
            .conn
            .query_row("SELECT agents_sha256 FROM bot_pushes WHERE bot = ?1 AND target = ?2", [bot, target], |r| r.get(0))
            .optional()?;
        Ok(hash)
    }

    /// Records that an AGENTS.md of `hash` went to `bot` at `target`.
    pub fn set_pushed(&self, bot: &str, target: &str, hash: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO bot_pushes (bot, target, agents_sha256, pushed_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (bot, target) DO UPDATE SET agents_sha256 = excluded.agents_sha256, pushed_at = excluded.pushed_at",
            params![bot, target, hash, format_utc(now)],
        )?;
        Ok(())
    }

    /// `bot`'s current run, if it ever had one.
    pub fn bot_run(&self, bot: &str) -> Result<Option<BotRun>> {
        Ok(self.conn.query_row(&format!("SELECT {RUN_COLS} FROM bot_runs WHERE bot = ?1"), [bot], run_row).optional()?)
    }

    /// A new run of `bot` at `address`, `starting`: only over none, or one
    /// that is final. False if a run that is not final is there.
    pub fn begin_run(&self, bot: &str, address: &str, session: Option<&str>, now: i64) -> Result<bool> {
        let changed = self.conn.execute(
            "INSERT INTO bot_runs (bot, address, state, session, started_at, updated_at, reason)
             VALUES (?1, ?2, 'starting', ?3, ?4, ?4, NULL)
             ON CONFLICT (bot) DO UPDATE SET address = excluded.address, state = 'starting', session = excluded.session,
                 started_at = excluded.started_at, updated_at = excluded.updated_at, reason = NULL
             WHERE bot_runs.state IN ('failed', 'gone')",
            params![bot, address, session, format_utc(now)],
        )?;
        Ok(changed == 1)
    }

    /// Moves `bot`'s run at `address` from `from` to `to`, only if it is
    /// still there and in `from`: a late word about an older run, or one
    /// already moved, changes nothing (false). Only `starting` to `ready`
    /// or `failed`, and `ready` to `gone`; `starting` to `gone` too, when
    /// the owner ends it (`bot pause`, `bot reset`).
    pub fn move_run(&self, bot: &str, address: &str, from: BotRunState, to: BotRunState, reason: Option<&str>, now: i64) -> Result<bool> {
        use BotRunState::*;
        if !matches!((from, to), (Starting, Ready) | (Starting, Failed) | (Ready, Gone) | (Starting, Gone)) {
            return Err(StoreError::WrongState { id: bot.into(), state: from.to_string(), want: "starting, or ready to go" });
        }
        let changed = self.conn.execute(
            "UPDATE bot_runs SET state = ?4, reason = ?5, updated_at = ?6 WHERE bot = ?1 AND address = ?2 AND state = ?3",
            params![bot, address, from.as_str(), to.as_str(), reason, format_utc(now)],
        )?;
        Ok(changed == 1)
    }

    /// The session `bot` resumes, if one is kept.
    pub fn session(&self, bot: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT session FROM bot_sessions WHERE bot = ?1", [bot], |r| r.get(0)).optional()?)
    }

    /// Keeps `session` for `bot`, or forgets it (None).
    pub fn set_session(&self, bot: &str, session: Option<&str>, now: i64) -> Result<()> {
        match session {
            Some(id) => self.conn.execute(
                "INSERT INTO bot_sessions (bot, session, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (bot) DO UPDATE SET session = excluded.session, updated_at = excluded.updated_at",
                params![bot, id, format_utc(now)],
            )?,
            None => self.conn.execute("DELETE FROM bot_sessions WHERE bot = ?1", [bot])?,
        };
        Ok(())
    }

    /// When the owner paused `bot`, if it is paused.
    pub fn paused(&self, bot: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT paused_at FROM bot_pauses WHERE bot = ?1", [bot], |r| r.get(0)).optional()?)
    }

    /// Pauses `bot`; false if it was paused already (kept as it was).
    pub fn set_paused(&self, bot: &str, now: i64) -> Result<bool> {
        let added = self.conn.execute("INSERT INTO bot_pauses (bot, paused_at) VALUES (?1, ?2) ON CONFLICT (bot) DO NOTHING", params![bot, format_utc(now)])?;
        Ok(added == 1)
    }

    /// No longer paused; false if it was not.
    pub fn clear_paused(&self, bot: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM bot_pauses WHERE bot = ?1", [bot])? == 1)
    }

    /// When `bot`'s AGENTS.md was last pushed anywhere, if ever.
    pub fn last_push(&self, bot: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT max(pushed_at) FROM bot_pushes WHERE bot = ?1", [bot], |r| r.get(0))?)
    }

    /// Forgets all the ledger keeps of `bot`: its run, session, pause and
    /// pushes, in one transaction. Its handoffs stay, as a record; those
    /// still waiting are `lost` (never sent to another bot of its name).
    pub fn forget_bot(&mut self, bot: &str) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for table in ["bot_runs", "bot_sessions", "bot_pauses", "bot_pushes"] {
            tx.execute(&format!("DELETE FROM {table} WHERE bot = ?1"), [bot])?;
        }
        tx.execute("UPDATE handoffs SET state = 'lost', reason = 'the bot was removed before it was delivered' WHERE bot = ?1 AND state = 'waiting'", [bot])?;
        tx.commit()?;
        Ok(())
    }

    /// How long to wait for another writer before giving up.
    pub fn set_busy_timeout(&self, wait: Duration) -> Result<()> {
        Ok(self.conn.busy_timeout(wait)?)
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
        if content.len() > MAX_CONTENT {
            return Err(StoreError::TooLong("the reminder", content.len()));
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
        let until = new.check.as_ref().and(new.check_until).map(format_utc);
        tx.execute(
            "INSERT INTO reminders (id, content, state, next_fire_at, every_spec, check_cmd, check_hash, check_once, check_until,
                                    task_id, created_at, updated_at)
             VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                id,
                content,
                format_utc(new.next_fire_at),
                new.every.map(|r| r.spec()),
                cmd,
                hash,
                new.check.is_some() && new.check_once,
                until,
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
pub(crate) fn next_id(conn: &Connection, table: &str, prefix: &str) -> Result<String> {
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

pub(crate) fn get_reminder(conn: &Connection, id: &str) -> Result<Reminder> {
    conn.query_row(&format!("SELECT {REMINDER_COLS} FROM reminders WHERE id = ?1"), [id], reminder_row)
        .optional()?
        .ok_or_else(|| StoreError::NotFound("reminder", id.into()))
}

fn word<T>(row: &Row, i: usize, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let s: String = row.get(i)?;
    parse(&s).ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(i, Type::Text, format!("unknown state {s:?}").into()))
}

const RUN_COLS: &str = "bot, address, state, session, started_at, updated_at, reason";

fn run_row(r: &Row) -> rusqlite::Result<BotRun> {
    Ok(BotRun {
        bot: r.get(0)?,
        address: r.get(1)?,
        state: word(r, 2, BotRunState::parse)?,
        session: r.get(3)?,
        started_at: r.get(4)?,
        updated_at: r.get(5)?,
        reason: r.get(6)?,
    })
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
        check_once: r.get(8)?,
        check_until: r.get(9)?,
        last_checked_at: r.get(10)?,
        check_failures: r.get(11)?,
        check_error: r.get(12)?,
        last_fired_at: r.get(13)?,
        task_id: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
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
        NewReminder {
            content: content.into(),
            next_fire_at: at,
            every: None,
            check: None,
            check_once: false,
            check_until: None,
            task_id: None,
        }
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
    fn remembers_pushes_and_reads_without_writing() {
        let dir = crate::test_dir("store-pushes");
        assert!(Store::open_read_only(&dir).unwrap().is_none());
        assert!(!dir.join("ledger.db").exists(), "a read made the ledger");
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.pushed("ops", "local:/tmp/example/ops").unwrap(), None);
        s.set_pushed("ops", "local:/tmp/example/ops", "aa", 0).unwrap();
        s.set_pushed("ops", "local:/tmp/example/ops", "bb", 1).unwrap();
        s.set_pushed("ops", "gpu-box:~/ops", "cc", 1).unwrap();
        assert_eq!(s.pushed("ops", "local:/tmp/example/ops").unwrap().as_deref(), Some("bb"));
        drop(s);
        let r = Store::open_read_only(&dir).unwrap().unwrap();
        assert_eq!(r.pushed("ops", "gpu-box:~/ops").unwrap().as_deref(), Some("cc"));
        assert!(r.set_pushed("ops", "gpu-box:~/ops", "dd", 2).is_err(), "a read-only ledger took a write");
        // An older ledger is not migrated by a read: it has no pushes yet.
        r.conn.close().unwrap();
        let conn = Connection::open(dir.join("ledger.db")).unwrap();
        conn.execute_batch("DROP TABLE bot_pushes; PRAGMA user_version = 2;").unwrap();
        drop(conn);
        let r = Store::open_read_only(&dir).unwrap().unwrap();
        assert_eq!(r.pushed("ops", "gpu-box:~/ops").unwrap(), None);
        assert_eq!(r.version().unwrap(), 2);
        drop(r);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn runs_move_only_from_where_they_are() {
        use BotRunState::*;
        let (s, dir) = store("store-runs");
        assert_eq!(s.bot_run("ops").unwrap(), None);
        assert!(s.begin_run("ops", "t70", Some("s-1"), 0).unwrap());
        // Not over a run that is not final.
        assert!(!s.begin_run("ops", "t71", None, 1).unwrap());
        let run = s.bot_run("ops").unwrap().unwrap();
        assert_eq!((run.address.as_str(), run.state, run.session.as_deref()), ("t70", Starting, Some("s-1")));
        // A late word about another terminal, or from the wrong state: nothing.
        assert!(!s.move_run("ops", "t71", Starting, Failed, Some("start_timeout"), 2).unwrap());
        assert!(!s.move_run("ops", "t70", Ready, Gone, None, 2).unwrap());
        assert!(s.move_run("ops", "t70", Starting, Ready, None, 2).unwrap());
        // Once ready, a late timeout cannot fail it.
        assert!(!s.move_run("ops", "t70", Starting, Failed, Some("start_timeout"), 3).unwrap());
        assert!(matches!(s.move_run("ops", "t70", Ready, Starting, None, 3), Err(StoreError::WrongState { .. })));
        assert!(matches!(s.move_run("ops", "t70", Gone, Ready, None, 3), Err(StoreError::WrongState { .. })));
        assert!(s.move_run("ops", "t70", Ready, Gone, Some("finished"), 4).unwrap());
        assert_eq!(s.bot_run("ops").unwrap().unwrap().reason.as_deref(), Some("finished"));
        // Final: a new start replaces it.
        assert!(s.begin_run("ops", "t72", None, 5).unwrap());
        let run = s.bot_run("ops").unwrap().unwrap();
        assert_eq!((run.address.as_str(), run.state, run.reason, run.started_at.as_str()), ("t72", Starting, None, "1970-01-01T00:00:05Z"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pauses_are_the_owners_and_bots_can_be_forgotten() {
        let (mut s, dir) = store("store-pauses");
        assert_eq!(s.paused("ops").unwrap(), None);
        assert!(s.set_paused("ops", 0).unwrap());
        assert!(!s.set_paused("ops", 5).unwrap());
        assert_eq!(s.paused("ops").unwrap().as_deref(), Some("1970-01-01T00:00:00Z"));
        // A run's ups and downs leave it alone.
        assert!(s.begin_run("ops", "t70", None, 1).unwrap());
        assert!(s.move_run("ops", "t70", BotRunState::Starting, BotRunState::Failed, None, 2).unwrap());
        assert!(s.paused("ops").unwrap().is_some());
        assert!(s.clear_paused("ops").unwrap());
        assert!(!s.clear_paused("ops").unwrap());
        // Everything about one bot goes, nothing about another.
        s.set_session("ops", Some("s-1"), 3).unwrap();
        s.set_paused("ops", 3).unwrap();
        s.set_pushed("ops", "local:/tmp/example", "aa", 3).unwrap();
        s.set_session("trainer", Some("s-2"), 3).unwrap();
        assert_eq!(s.last_push("ops").unwrap().as_deref(), Some("1970-01-01T00:00:03Z"));
        s.forget_bot("ops").unwrap();
        assert_eq!((s.bot_run("ops").unwrap(), s.session("ops").unwrap(), s.paused("ops").unwrap(), s.last_push("ops").unwrap()), (None, None, None, None));
        assert_eq!(s.session("trainer").unwrap().as_deref(), Some("s-2"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_sessions_apart_from_runs() {
        let (s, dir) = store("store-sessions");
        assert_eq!(s.session("trainer").unwrap(), None);
        s.set_session("trainer", Some("s-1"), 0).unwrap();
        s.set_session("trainer", Some("s-2"), 1).unwrap();
        assert!(s.begin_run("trainer", "gpu-box/t71", Some("s-2"), 1).unwrap());
        assert!(s.move_run("trainer", "gpu-box/t71", BotRunState::Starting, BotRunState::Failed, None, 2).unwrap());
        assert_eq!(s.session("trainer").unwrap().as_deref(), Some("s-2"));
        s.set_session("trainer", None, 3).unwrap();
        assert_eq!(s.session("trainer").unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_a_version_1_ledger() {
        let dir = crate::test_dir("migrates_a_version_1_ledger");
        let conn = Connection::open(dir.join("ledger.db")).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO tasks VALUES ('T-0001', 'back up homelab', 'open', NULL, 't', 't');
             INSERT INTO reminders VALUES ('R-0001', 'uptime', 'active', '2026-11-26T05:00:00Z', '5m', NULL,
                                           'uptime', 'ab', 'T-0001', 't', 't');",
        )
        .unwrap();
        drop(conn);
        let s = Store::open(&dir).unwrap();
        let r = s.reminder("R-0001").unwrap();
        assert_eq!((r.check_cmd.as_deref(), r.check_hash.as_deref(), r.task_id.as_deref()), (Some("uptime"), Some("ab"), Some("T-0001")));
        assert_eq!((r.check_until, r.check_failures, r.check_once), (None, 0, false));
        let version: i64 = s.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        // Up to the latest: a version 1 ledger goes through every step after it.
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert_eq!(s.pushed("ops", "local:/tmp/example").unwrap(), None);
        let fk: i64 = s.conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 0);
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
        let long = once(&"x".repeat(MAX_CONTENT + 1), T0);
        assert!(matches!(s.add_reminder(&long, T0), Err(StoreError::TooLong(_, n)) if n == MAX_CONTENT + 1));
        assert_eq!(count(&s, "reminders"), 0);
        s.add_reminder(&once(&"x".repeat(MAX_CONTENT), T0), T0).unwrap();
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
            check_once: true,
            check_until: Some(T0 + 7 * 86400),
            task_id: Some(task.id.clone()),
        };
        let r = s.add_reminder(&new, T0).unwrap();
        assert_eq!(r.every.as_deref(), Some("5m"));
        assert!(r.check_once);
        assert_eq!(r.check_until.as_deref(), Some("2026-12-03T04:45:22Z"));
        assert_eq!((r.check_failures, r.last_checked_at.as_deref()), (0, None));
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
}
