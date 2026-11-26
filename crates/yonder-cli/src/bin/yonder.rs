//! `yonder`: a task list and reminders, kept in the ledger in
//! `$YONDER_STATE_DIR` (default `~/.local/state/yonder`), and a look at
//! yonderd, which fires the reminders. On failure it
//! exits non-zero with one line on stderr: a stable code, what went wrong
//! and what to do next, such as `check_failed: exit:3; ...`.

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use yonder_core::config::Config;
use yonder_core::firing::ECHO_CHARS;
use yonder_core::hc::Hc;
use yonder_core::redact::excerpt;
use yonder_core::when::{self, Rule};
use yonder_core::{NewReminder, Store, StoreError, check_exec, daemon, service, store};
use yonder_model::{Reminder, Task, TaskState};

/// How long a check runs if --for does not say, and the most it may say.
const CHECK_FOR: &str = "7d";
const CHECK_FOR_MAX: i64 = 90 * 86400;

#[derive(Parser)]
#[command(name = "yonder", version, about = "A task list and reminders, kept in Yonder's ledger")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// The task list.
    #[command(subcommand)]
    Task(TaskCmd),
    /// Reminders: once, repeating, or watching a command's output.
    #[command(subcommand)]
    Remind(RemindCmd),
    /// yonderd, which fires the reminders.
    #[command(subcommand)]
    Ctl(CtlCmd),
}

#[derive(Subcommand)]
enum CtlCmd {
    /// Whether yonderd runs, its last round, and what waits to be delivered.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Print a launchd plist that runs yonderd; installing it is up to you.
    ServicePlist,
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Add an open task.
    Add {
        title: String,
        /// Where it came from, such as hc:nova.
        #[arg(long)]
        source: Option<String>,
    },
    /// List open tasks.
    Ls {
        /// Done and cancelled ones too.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Mark an open task done.
    Done { id: String },
    /// Cancel an open task.
    Cancel { id: String },
}

#[derive(Subcommand)]
enum RemindCmd {
    /// Set a reminder.
    Set(SetArgs),
    /// List active reminders, soonest first.
    Ls {
        /// Fired and cancelled ones too.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Keep a reminder quiet for a while from now.
    Snooze {
        id: String,
        /// How long: 30m, 2h, 1d, 1h30m.
        #[arg(long, value_name = "DURATION")]
        by: String,
    },
    /// Cancel a reminder.
    Cancel { id: String },
    /// List the reminders due now; changes nothing.
    Due {
        /// Instead of now: YYYY-MM-DDTHH:MM:SSZ.
        #[arg(long, value_name = "UTC")]
        now: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct SetArgs {
    content: String,
    #[command(flatten)]
    when: When,
    /// A shell command run on each repeat; the reminder is about its
    /// output changing. Needs --every; it must succeed now.
    #[arg(long)]
    check: Option<String>,
    /// How long to keep checking: a duration up to 90d, or forever.
    #[arg(long = "for", value_name = "DURATION", requires = "check", default_value = CHECK_FOR)]
    check_for: String,
    /// Stop after the first change.
    #[arg(long, requires = "check")]
    once: bool,
    /// The task it is about.
    #[arg(long)]
    task: Option<String>,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct When {
    /// After a while: 30m, 2h, 1d, 1h30m.
    #[arg(long = "in", value_name = "DURATION")]
    after: Option<String>,
    /// At a local time: "YYYY-MM-DD HH:MM".
    #[arg(long, value_name = "TIME")]
    at: Option<String>,
    /// Repeating: 5m, 2h, 1d, daily@09:00, weekly:1@09:00 (1 is Monday).
    #[arg(long, value_name = "RULE")]
    every: Option<String>,
}

/// What `yonder` prints on failure: `code: message; hint`.
struct Fail {
    code: &'static str,
    msg: String,
    hint: &'static str,
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Fail {
        let hint = match &e {
            StoreError::NotFound(..) => "see yonder task ls --all or yonder remind ls --all",
            StoreError::WrongState { .. } => "only open tasks and active reminders change",
            StoreError::Empty(_) => "give it some text",
            StoreError::CheckNeedsEvery => "add --every",
            StoreError::TooLong(..) => "say it in fewer words; details can go in a file it names",
            StoreError::NoStateDir | StoreError::Dir(..) => "set YONDER_STATE_DIR to a writable directory",
            StoreError::TooNew(_) => "use a newer yonder",
            StoreError::Sql(_) => "try again; if it stays, look at ledger.db in the state directory",
        };
        Fail { code: e.code(), msg: e.to_string(), hint }
    }
}

const DURATION_HINT: &str = "use 30m, 2h, 1d or 1h30m";

fn bad_duration(s: &str) -> Fail {
    Fail { code: "bad_time", msg: format!("{s:?} is not a duration"), hint: DURATION_HINT }
}

/// `now` and the duration `d` later, no later than the year 9999.
fn later(now: i64, d: &str) -> Result<i64, Fail> {
    let secs = when::parse_duration(d).ok_or_else(|| bad_duration(d))?;
    when::add(now, secs).ok_or(Fail {
        code: "bad_time",
        msg: format!("{d:?} from now is past the year 9999"),
        hint: "give a shorter duration",
    })
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if !e.use_stderr() => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("usage: {}; see yonder --help", one_line(&e.render().to_string()));
            return ExitCode::from(2);
        }
    };
    match run(cli.cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            eprintln!("{}: {}; {}", f.code, one_line(&f.msg), f.hint);
            ExitCode::FAILURE
        }
    }
}

/// A clap error, or any message, on one line: up to its usage section.
fn one_line(text: &str) -> String {
    let lines = text.lines().take_while(|l| !l.starts_with("Usage:") && !l.starts_with("For more"));
    let joined = lines.map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    joined.strip_prefix("error: ").unwrap_or(&joined).to_string()
}

fn run(cmd: Cmd) -> Result<(), Fail> {
    if let Cmd::Ctl(CtlCmd::ServicePlist) = cmd {
        return service_plist();
    }
    let mut store = Store::open_default()?;
    let now = when::now();
    match cmd {
        Cmd::Task(TaskCmd::Add { title, source }) => {
            let t = store.add_task(&title, source.as_deref(), now)?;
            println!("{} added: {}", t.id, t.title);
        }
        Cmd::Task(TaskCmd::Ls { all, json }) => {
            let tasks = store.tasks(all)?;
            if json {
                print_json(&tasks);
            } else if tasks.is_empty() {
                println!("no {}tasks", if all { "" } else { "open " });
            } else {
                tasks.iter().for_each(print_task);
            }
        }
        Cmd::Task(TaskCmd::Done { id }) => print_task(&store.close_task(&id, TaskState::Done, now)?),
        Cmd::Task(TaskCmd::Cancel { id }) => print_task(&store.close_task(&id, TaskState::Cancelled, now)?),
        Cmd::Remind(RemindCmd::Set(args)) => set_reminder(&mut store, args, now)?,
        Cmd::Ctl(CtlCmd::Status { json }) => status(&store, json)?,
        Cmd::Ctl(CtlCmd::ServicePlist) => unreachable!("handled before the ledger opens"),
        Cmd::Remind(RemindCmd::Ls { all, json }) => {
            let reminders = store.reminders(all)?;
            if json {
                print_json(&reminders);
            } else if reminders.is_empty() {
                println!("no {}reminders", if all { "" } else { "active " });
            } else {
                reminders.iter().for_each(print_reminder);
            }
        }
        Cmd::Remind(RemindCmd::Snooze { id, by }) => {
            print_reminder(&store.snooze(&id, later(now, &by)?, now)?);
        }
        Cmd::Remind(RemindCmd::Cancel { id }) => print_reminder(&store.cancel_reminder(&id, now)?),
        Cmd::Remind(RemindCmd::Due { now: at, json }) => {
            let at = match at {
                None => now,
                Some(s) => when::parse_utc(&s).ok_or(Fail {
                    code: "bad_time",
                    msg: format!("{s:?} is not a UTC time"),
                    hint: "use YYYY-MM-DDTHH:MM:SSZ",
                })?,
            };
            let due = store.due(at)?;
            if json {
                print_json(&due);
            } else {
                due.iter().for_each(print_reminder);
            }
        }
    }
    Ok(())
}

fn set_reminder(store: &mut Store, args: SetArgs, now: i64) -> Result<(), Fail> {
    let SetArgs { content, when, check, check_for, once, task: task_id } = args;
    let (next_fire_at, every) = match (when.after, when.at, when.every) {
        (Some(d), _, _) => (later(now, &d)?, None),
        (_, Some(at), _) => {
            let t = when::parse_local(&at).ok_or(Fail {
                code: "bad_time",
                msg: format!("{at:?} is not a local time"),
                hint: "use \"YYYY-MM-DD HH:MM\"",
            })?;
            if t <= now {
                let msg = format!("{} is in the past", when::format_local(t));
                return Err(Fail { code: "bad_time", msg, hint: "give a later --at" });
            }
            (t, None)
        }
        (_, _, Some(spec)) => {
            let rule = Rule::parse(&spec).ok_or(Fail {
                code: "bad_rule",
                msg: format!("{spec:?} is not a repeat rule"),
                hint: "use 5m, 2h, 1d, daily@09:00 or weekly:1@09:00 (1 is Monday)",
            })?;
            let next = rule.next_after(now).ok_or(Fail {
                code: "bad_rule",
                msg: format!("{spec:?} next goes off past the year 9999"),
                hint: "give a shorter repeat",
            })?;
            (next, Some(rule))
        }
        (None, None, None) => unreachable!("clap requires one of --in, --at, --every"),
    };
    if check.is_some() && every.is_none() {
        return Err(StoreError::CheckNeedsEvery.into());
    }
    if let Some(id) = &task_id {
        store.task(id)?;
    }
    let check_until = match check_for.as_str() {
        "forever" => None,
        d if when::parse_duration(d).is_some_and(|s| s > CHECK_FOR_MAX) => {
            let msg = format!("--for {d} is longer than 90d");
            return Err(Fail { code: "bad_time", msg, hint: "give 90d or less, or forever" });
        }
        d => Some(later(now, d)?),
    };
    let mut baseline = None;
    let check = match check {
        None => None,
        Some(cmd) => {
            let out = check_exec::run(&cmd).map_err(|e| Fail {
                code: "check_failed",
                msg: e.to_string(),
                hint: "fix the command so it succeeds and prints what to watch, then set it again",
            })?;
            baseline = Some(out.stdout);
            Some((cmd, out.hash))
        }
    };
    let new = NewReminder { content, next_fire_at, every, check, check_once: once, check_until, task_id };
    let r = store.add_reminder(&new, now)?;
    println!("{} set: {}", r.id, r.content);
    println!("next: {} local ({}), {}", when::format_local(next_fire_at), r.next_fire_at, rule_of(&r));
    if let Some(out) = baseline {
        let until = r.check_until.as_deref().map_or("forever".into(), local);
        let after = if r.check_once { ", or its first change" } else { "" };
        println!("checking until {until}{after}");
        println!("check baseline:\n{}", excerpt(&out, ECHO_CHARS));
    }
    Ok(())
}

fn status(store: &Store, json: bool) -> Result<(), Fail> {
    let dir = store::state_dir()?;
    let pid = daemon::running_pid(&dir).map_err(|e| Fail {
        code: "state_dir",
        msg: format!("reading {}: {e}", daemon::lock_path(&dir).display()),
        hint: "check the state directory's permissions",
    })?;
    let last = store.meta("last_tick_at")?;
    let (pending, lost) = (store.pending()?, store.lost()?);
    if json {
        let v = serde_json::json!({"running": pid.is_some(), "pid": pid, "last_tick_at": last, "pending": pending, "lost": lost});
        println!("{}", serde_json::to_string_pretty(&v).expect("plain data"));
        return Ok(());
    }
    match pid {
        Some(pid) => println!("yonderd: running, pid {pid}"),
        None => println!("yonderd: not running"),
    }
    match &last {
        Some(t) => println!("last round: {} local ({t})", local(t)),
        None => println!("last round: never"),
    }
    println!("pending deliveries: {pending}");
    println!("lost deliveries: {lost} (may or may not have arrived; never sent again)");
    Ok(())
}

fn service_plist() -> Result<(), Fail> {
    let config = Config::load_default().map_err(|e| Fail { code: "config", msg: e.to_string(), hint: "fix the config file" })?;
    let dir = store::state_dir()?;
    let exe = std::env::current_exe().and_then(|p| p.canonicalize());
    let program = exe.map(|p| p.with_file_name("yonderd")).map_err(|e| Fail {
        code: "no_program",
        msg: format!("where yonder runs from: {e}"),
        hint: "run yonder from where yonderd sits beside it",
    })?;
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let hc = Hc::bin(&config.hc.bin, &home);
    let mut env = vec![("PATH".to_string(), service::path_with(service::find_program(&hc).as_deref()))];
    env.push(("HOME".into(), home.display().to_string()));
    for key in ["YONDER_STATE_DIR", "YONDER_CONFIG", "YONDER_HC"] {
        if let Some(v) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            env.push((key.into(), v.to_string_lossy().into_owned()));
        }
    }
    let log = dir.join("logs/yonderd.out");
    print!("{}", service::plist(&config.service.label, &program, &env, &log));
    Ok(())
}

/// A ledger time as local `YYYY-MM-DD HH:MM`.
fn local(utc: &str) -> String {
    when::parse_utc(utc).map_or_else(|| utc.to_string(), when::format_local)
}

fn print_json<T: serde::Serialize>(items: &[T]) {
    println!("{}", serde_json::to_string_pretty(items).expect("plain data"));
}

fn print_task(t: &Task) {
    let source = t.source.as_ref().map(|s| format!("  (from {s})")).unwrap_or_default();
    println!("{}  {:<9}  {}{source}", t.id, t.state, t.title);
}

fn print_reminder(r: &Reminder) {
    let mut extra = String::new();
    if r.due_at() != r.next_fire_at {
        extra += &format!("  snoozed until {}", local(r.due_at()));
    }
    if r.check_cmd.is_some() {
        let last = r.last_checked_at.as_deref().map_or("never".into(), local);
        extra += &format!("  [check: last {last}, {} failed", r.check_failures);
        if let Some(e) = r.check_error.as_ref().filter(|_| r.check_failures > 0) {
            extra += &format!(" ({e})");
        }
        extra += "]";
    }
    if let Some(task) = &r.task_id {
        extra += &format!("  [{task}]");
    }
    println!("{}  {:<9}  {}  {:<14}  {}{extra}", r.id, r.state, local(&r.next_fire_at), rule_of(r), r.content);
}

fn rule_of(r: &Reminder) -> &str {
    r.every.as_deref().unwrap_or("once")
}
