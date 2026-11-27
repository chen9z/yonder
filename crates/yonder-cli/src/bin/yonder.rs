//! `yonder`: a task list and reminders, kept in the ledger in Yonder's
//! home `$YONDER_HOME` (default `~/.yonder`), the bots defined there, and
//! a look at yonderd, which fires the reminders. On failure it
//! exits non-zero with one line on stderr: a stable code, what went wrong
//! and what to do next, such as `check_failed: exit:3; ...`.

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use clap::{Args, Parser, Subcommand};
use yonder_core::config::Config;
use yonder_core::firing::ECHO_CHARS;
use yonder_core::botrun::{self, BotFail, StartWith};
use yonder_core::hc::{Hc, HcBots};
use yonder_core::keychain::{self, Keychain, KeychainError};
use yonder_core::redact::excerpt;
use yonder_core::when::{self, Rule};
use yonder_core::bot::{self, Bot, BotError, Level};
use yonder_core::{NewReminder, Store, StoreError, check_exec, daemon, home, service, store, sync};
use yonder_model::{BotDef, BotKind, Reminder, Task, TaskState};

/// What agents are told about Yonder: `yonder manual`, and the end of the
/// AGENTS.md that `yonder bot sync` pushes.
const MANUAL: &str = include_str!("../manual.md");

/// hc calls from `yonder` are never stopped early, only timed out.
static NEVER: AtomicBool = AtomicBool::new(false);

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
    /// Bots, as defined in bots/ in Yonder's home.
    #[command(subcommand)]
    Bot(BotCmd),
    /// How agents work with Yonder, for them to read.
    Manual,
}

#[derive(Subcommand)]
enum BotCmd {
    /// Make a bot's directory: a commented bot.toml, empty template.md and AGENTS.md.
    New {
        name: String,
        #[arg(long, default_value = "persistent", value_parser = ["persistent", "oneoff"])]
        kind: String,
        /// The program it runs in: codex, pi, claude or another.
        #[arg(long)]
        harness: Option<String>,
    },
    /// List the bots, broken definitions too.
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// A bot's fields and files.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Check the definitions, one bot's or all; exits 1 on any error.
    Check {
        name: Option<String>,
        /// Also what starting it needs: its machine reached, its cwd there,
        /// its harness runnable there.
        #[arg(long)]
        live: bool,
    },
    /// Start a bot in an hc terminal, wait until it is ready, and send it
    /// its template (and task).
    Start {
        name: String,
        /// What to do: after the template, in the first message. A oneoff
        /// bot needs one.
        #[arg(long)]
        task: Option<String>,
        /// Where a oneoff bot works.
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Each bot's run: stopped, starting, ready, failed or gone (unknown
    /// when hc does not answer), its terminal, session and last reason.
    Status {
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Stop what a ready bot is doing now (its stop key); its terminal and
    /// session stay.
    Stop { name: String },
    /// The session a bot resumes when started: show it, set it, or --clear it.
    Session {
        name: String,
        #[arg(conflicts_with = "clear")]
        id: Option<String>,
        #[arg(long)]
        clear: bool,
    },
    /// A bot's environment values, kept in the login keychain.
    #[command(subcommand)]
    Env(EnvCmd),
    /// Internal: what a bot's terminal runs. Takes its env from the
    /// keychain and becomes its harness. Not for typing yourself.
    Exec { name: String },
    /// Push each persistent bot's AGENTS.md (with the manual) to its cwd,
    /// and pull its MEMORY.md back; all bots if none are named. Exits 1 if
    /// any drifted, failed or is invalid.
    Sync {
        names: Vec<String>,
        /// Write AGENTS.md even where it was changed since the last sync.
        #[arg(long)]
        force: bool,
        /// Say what would happen; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum EnvCmd {
    /// Keep the value of one of a bot's env variables, read from stdin (one
    /// trailing newline dropped), in the login keychain. It is never shown.
    Set { name: String, var: String },
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
            StoreError::Home(home::HomeError::Relative(_)) => home::ABSOLUTE,
            StoreError::Home(_) | StoreError::Dir(..) => "set YONDER_HOME to a writable directory",
            StoreError::TooNew(_) => "use a newer yonder",
            StoreError::Sql(_) => "try again; if it stays, look at ledger.db in Yonder's home",
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
            if f.hint.is_empty() {
                eprintln!("{}: {}", f.code, one_line(&f.msg));
            } else {
                eprintln!("{}: {}; {}", f.code, one_line(&f.msg), f.hint);
            }
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
    let cmd = match cmd {
        Cmd::Ctl(CtlCmd::ServicePlist) => return service_plist(),
        Cmd::Bot(cmd) => return bot_cmd(cmd),
        Cmd::Manual => {
            print!("{MANUAL}");
            return Ok(());
        }
        cmd => cmd,
    };
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
        Cmd::Ctl(CtlCmd::ServicePlist) | Cmd::Bot(_) | Cmd::Manual => unreachable!("handled before the ledger opens"),
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
    let dir = store::home_dir()?;
    let pid = daemon::running_pid(&dir).map_err(|e| Fail {
        code: "home",
        msg: format!("reading {}: {e}", daemon::lock_path(&dir).display()),
        hint: "check the permissions of Yonder's home",
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
    let dir = store::home_dir()?;
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
    for key in ["YONDER_HOME", "YONDER_HC"] {
        if let Some(v) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            env.push((key.into(), v.to_string_lossy().into_owned()));
        }
    }
    let log = dir.join("logs/yonderd.out");
    print!("{}", service::plist(&config.service.label, &program, &env, &log));
    Ok(())
}

impl From<BotError> for Fail {
    fn from(e: BotError) -> Fail {
        let hint = match &e {
            BotError::BadName(_) => "use a-z, 0-9 and -, starting with a letter, at most 32",
            BotError::Exists(..) => "pick another name, or change the one there",
            BotError::NotFound(_) => "see yonder bot ls",
            BotError::Link(_) => "make it a directory of its own, not a link",
            BotError::Io(..) => "check the permissions of Yonder's home",
        };
        Fail { code: e.code(), msg: e.to_string(), hint }
    }
}

fn bot_cmd(cmd: BotCmd) -> Result<(), Fail> {
    let home = store::home_dir()?;
    let bots = home::bots_dir(&home);
    match cmd {
        BotCmd::New { name, kind, harness } => {
            let kind = BotKind::parse(&kind).expect("clap allows only the kinds");
            let dir = bot::create(&home, &name, kind, harness.as_deref())?;
            println!("{name} made: {}", dir.display());
            println!("next: fill in {}, then yonder bot check {name}", dir.join("bot.toml").display());
        }
        BotCmd::Ls { json } => {
            let all = bot::list(&bots)?;
            if json {
                let items: Vec<_> = all.iter().map(bot_json).collect();
                println!("{}", serde_json::to_string_pretty(&items).expect("plain data"));
            } else if all.is_empty() {
                println!("no bots in {}", bots.display());
            }
            for b in all.iter().filter(|_| !json) {
                let files = b.files.iter().map(|(f, _)| *f).collect::<Vec<_>>().join(" ");
                match &b.def {
                    Some(d) => println!("{:<16}  {:<10}  {:<10}  {:<8}  {files}", b.name, d.kind, d.machine, d.harness),
                    None => println!("{:<16}  invalid     {}", b.name, first_error(b)),
                }
            }
        }
        BotCmd::Show { name, json } => {
            let b = bot::find(&bots, &name)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&bot_json(&b)).expect("plain data"));
                return Ok(());
            }
            println!("{}  {}", b.name, b.dir.display());
            if let Some(d) = &b.def {
                let none = || "-".to_owned();
                for (key, value) in [
                    ("description", d.description.clone()),
                    ("kind", d.kind.to_string()),
                    ("machine", d.machine.clone()),
                    ("cwd", d.cwd.clone().unwrap_or_else(none)),
                    ("harness", d.harness.clone()),
                    ("model", d.model.clone().unwrap_or_else(none)),
                    ("args", format!("{:?}", d.args)),
                    ("memory_id", d.memory_id.clone().unwrap_or_else(none)),
                    ("env", format!("{:?}", d.env)),
                ] {
                    println!("  {key:<12} {value}");
                }
            }
            for (file, path) in &b.files {
                println!("  {file:<12} {}", path.display());
            }
            print_problems(&b, "  ");
        }
        BotCmd::Check { name, live } => {
            let all = match name {
                Some(name) => vec![bot::find(&bots, &name)?],
                None => bot::list(&bots)?,
            };
            if all.is_empty() {
                println!("no bots in {}", bots.display());
            }
            let (hc, mut live_wrong) = (live.then(hc_bots).transpose()?, false);
            for b in &all {
                let wrong = match (&hc, &b.def) {
                    (Some(hc), Some(def)) => live_check(hc, def),
                    _ => Vec::new(),
                };
                if b.problems.is_empty() && wrong.is_empty() {
                    println!("{}: ok", b.name);
                }
                print_problems(b, "");
                for w in &wrong {
                    println!("error: {}: {w}", b.name);
                }
                live_wrong |= !wrong.is_empty();
            }
            if all.iter().any(|b| b.problems.iter().any(|p| p.level == Level::Error)) {
                return Err(Fail { code: "bot_invalid", msg: "a bot's definition has errors".into(), hint: "fix what the error lines say" });
            }
            if live_wrong {
                return Err(Fail { code: "start_check", msg: "what a bot needs to start is missing".into(), hint: "fix what the error lines say" });
            }
        }
        BotCmd::Sync { names, force, dry_run } => bot_sync(&home, &bots, names, sync::Options { force, dry_run })?,
        BotCmd::Start { name, task, cwd } => {
            let b = bot::find(&bots, &name)?;
            let def = valid(&b)?;
            let template = b.files.iter().find(|(f, _)| *f == "template.md").map(|(_, p)| std::fs::read_to_string(p)).transpose().map_err(|e| Fail {
                code: "home",
                msg: format!("reading {name}'s template.md: {e}"),
                hint: "check its permissions",
            })?;
            let store = Store::open(&home)?;
            let config = config()?;
            let with = StartWith {
                task: task.as_deref(),
                cwd: cwd.as_deref(),
                yonder: std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| Fail {
                    code: "no_program",
                    msg: format!("where yonder runs from: {e}"),
                    hint: "run yonder by its path",
                })?,
                home: home.clone(),
                service: std::env::var("YONDER_KEYCHAIN_SERVICE").ok().filter(|s| !s.is_empty()),
                user_home: std::env::var_os("HOME"),
                here: std::env::current_dir().unwrap_or_default(),
                timeout: std::time::Duration::from_secs(config.bot.start_timeout_secs),
                poll: std::time::Duration::from_secs(1),
            };
            let started = botrun::start(&store, &hc_bots()?, def, template.as_deref().unwrap_or(""), &with).map_err(bot_fail)?;
            for w in &started.warnings {
                eprintln!("warning: {name}: {w}");
            }
            println!("{name}: ready at {}", started.address);
        }
        BotCmd::Status { name, json } => bot_status(&home, &bots, name, json)?,
        BotCmd::Stop { name } => {
            bot::find(&bots, &name)?;
            let store = Store::open(&home)?;
            let (address, outcome) = botrun::stop(&store, &hc_bots()?, &name, config()?.bot.start_timeout_secs).map_err(bot_fail)?;
            println!("{name}: interrupted at {address} (hc says: {outcome})");
        }
        BotCmd::Session { name, id, clear } => {
            bot::find(&bots, &name)?;
            let store = Store::open(&home)?;
            match (id, clear) {
                (Some(id), _) => {
                    if id.is_empty() || id.len() > 256 || id.starts_with('-') || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
                        return Err(Fail {
                            code: "bad_input",
                            msg: format!("{id:?} is not a session id"),
                            hint: "give it as the harness printed it: no spaces, not starting with -",
                        });
                    }
                    store.set_session(&name, Some(&id), when::now())?;
                    println!("{name}: session {id}");
                }
                (None, true) => {
                    store.set_session(&name, None, when::now())?;
                    println!("{name}: no session");
                }
                (None, false) => println!("{}", store.session(&name)?.unwrap_or_else(|| "none".into())),
            }
        }
        BotCmd::Env(EnvCmd::Set { name, var }) => env_set(&bots, &name, &var)?,
        BotCmd::Exec { name } => return bot_exec(&home, &bots, &name),
    }
    Ok(())
}

fn config() -> Result<Config, Fail> {
    Config::load_default().map_err(|e| Fail { code: "config", msg: e.to_string(), hint: "fix the config file" })
}

fn hc_bots() -> Result<HcBots, Fail> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    Ok(HcBots { bin: Hc::bin(&config()?.hc.bin, &home), stop: &NEVER })
}

/// What `bot_fail` prints has no hint of its own: the message says it.
fn bot_fail(e: BotFail) -> Fail {
    Fail { code: e.code, msg: e.msg, hint: "" }
}

/// The definition of a valid bot.
fn valid(b: &Bot) -> Result<&BotDef, Fail> {
    b.def.as_ref().ok_or_else(|| Fail {
        code: "bot_invalid",
        msg: format!("{}: {}", b.name, first_error(b)),
        hint: "see yonder bot check",
    })
}

/// The startup check of `def`, beyond its definition: what is wrong.
fn live_check(hc: &HcBots, def: &BotDef) -> Vec<String> {
    // A oneoff bot's cwd comes with each start: not looked at here.
    match botrun::hc_cwd(def, None, std::env::var_os("HOME").as_ref(), &std::env::current_dir().unwrap_or_default()) {
        Ok(cwd) => botrun::check_live(def, cwd.as_deref(), hc),
        Err(e) => vec![e.msg],
    }
}

/// One line a bot: name, state, terminal, session, last reason.
fn bot_status(home: &std::path::Path, bots: &std::path::Path, name: Option<String>, json: bool) -> Result<(), Fail> {
    let all = match name {
        Some(name) => vec![bot::find(bots, &name)?],
        None => bot::list(bots)?,
    };
    let store = Store::open(home)?;
    let (hc, timeout) = (hc_bots()?, config()?.bot.start_timeout_secs);
    let mut items = Vec::new();
    for b in &all {
        let seen = botrun::observe(&store, &hc, &b.name, timeout, when::now()).map_err(bot_fail)?;
        let session = store.session(&b.name)?;
        let recorded = seen.run.as_ref().map(|r| r.state.as_str());
        let state = if seen.unknown.is_some() { "unknown" } else { recorded.unwrap_or("stopped") };
        let run = seen.run.as_ref();
        if json {
            items.push(serde_json::json!({
                "name": b.name,
                "state": state,
                "recorded_state": recorded,
                "address": run.map(|r| &r.address),
                "session": session,
                "reason": run.and_then(|r| r.reason.as_ref()),
                "started_at": run.map(|r| &r.started_at),
                "updated_at": run.map(|r| &r.updated_at),
                "hc_error": seen.unknown,
            }));
            continue;
        }
        let address = run.map_or("-", |r| r.address.as_str());
        let mut line = format!("{:<16}  {state:<9}  {address:<14}  session {}", b.name, session.as_deref().unwrap_or("-"));
        if let Some(reason) = run.and_then(|r| r.reason.as_ref()) {
            line += &format!("  ({reason})");
        }
        if let (Some(why), Some(recorded)) = (&seen.unknown, recorded) {
            line += &format!("  [recorded {recorded}; hc: {why}]");
        }
        println!("{line}");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&items).expect("plain data"));
    } else if all.is_empty() {
        println!("no bots in {}", bots.display());
    }
    Ok(())
}

impl From<KeychainError> for Fail {
    fn from(e: KeychainError) -> Fail {
        let hint = match &e {
            KeychainError::BadValue(_) => "give it as printable ASCII on one line",
            KeychainError::BadName(_) => "use a keychain service name without spaces or quotes",
            KeychainError::Failed(_) => "check that the login keychain is unlocked",
        };
        Fail { code: e.code(), msg: e.to_string(), hint }
    }
}

/// Keeps a value, read from stdin and never shown, in the keychain.
fn env_set(bots: &std::path::Path, name: &str, var: &str) -> Result<(), Fail> {
    let b = bot::find(bots, name)?;
    let def = valid(&b)?;
    if let Some(problem) = bot::env_problem(var) {
        return Err(Fail { code: "bad_env", msg: problem, hint: "use one of the names in the bot's env" });
    }
    if !def.env.iter().any(|v| v == var) {
        return Err(Fail {
            code: "env_not_declared",
            msg: format!("{name} has no {var} in its env"),
            hint: "add it to env in its bot.toml first",
        });
    }
    let value = read_secret(&format!("{var} for {name} (not shown): "))
        .map_err(|e| Fail { code: "bad_input", msg: format!("reading the value: {e}"), hint: "pipe it in on stdin" })?;
    let keychain = Keychain::from_env();
    keychain.set(&keychain::account(name, var), &value)?;
    println!("{name}: {var} kept in the keychain ({})", keychain.service);
    Ok(())
}

/// All of stdin less one trailing newline; from a terminal, one line,
/// asked for on stderr and not echoed.
fn read_secret(prompt: &str) -> std::io::Result<String> {
    use std::io::{BufRead, Read};
    // SAFETY: asks about fd 0.
    if unsafe { libc::isatty(0) } == 0 {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        return Ok(text.strip_suffix('\n').map(String::from).unwrap_or(text));
    }
    eprint!("{prompt}");
    // SAFETY: a termios we own, read and set on fd 0, put back below.
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut term) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut quiet = term;
    quiet.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &quiet) };
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &term) };
    eprintln!();
    read?;
    Ok(line.strip_suffix('\n').map(String::from).unwrap_or(line))
}

/// Becomes the bot's harness, with its env from the keychain. Returns only
/// on failure.
fn bot_exec(home: &std::path::Path, bots: &std::path::Path, name: &str) -> Result<(), Fail> {
    use std::os::unix::process::CommandExt;
    let b = bot::find(bots, name)?;
    let def = valid(&b)?;
    let session = match Store::open_read_only(home)? {
        Some(store) => store.session(name)?,
        None => None,
    };
    let keychain = Keychain::from_env();
    let mut values = Vec::new();
    for var in &def.env {
        match keychain.get(&keychain::account(name, var))? {
            Some(value) => values.push((var.as_str(), value)),
            None => {
                return Err(Fail { code: "env_missing", msg: var.clone(), hint: "keep it first: yonder bot env set <bot> <variable> < value-file" });
            }
        }
    }
    let (argv, warnings) = botrun::harness_argv(def, session.as_deref());
    for w in warnings {
        eprintln!("warning: {name}: {w}");
    }
    // In the login shell, as a terminal without env runs its harness.
    let argv = botrun::login_exec(argv);
    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]).envs(values);
    if let Some(id) = &def.memory_id {
        command.env("NMEM_AGENT_ID", id);
    }
    if def.kind == BotKind::Persistent {
        let cwd = botrun::hc_cwd(def, None, std::env::var_os("HOME").as_ref(), std::path::Path::new("/")).map_err(bot_fail)?;
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
    }
    let e = command.exec();
    Err(Fail { code: "exec_failed", msg: format!("{}: {e}", def.harness), hint: "check the harness with yonder bot check --live" })
}

/// One line a bot; one bot's trouble never stops the others. A dry run
/// opens the ledger only to read, and writes nothing anywhere.
fn bot_sync(home: &std::path::Path, bots: &std::path::Path, names: Vec<String>, opts: sync::Options) -> Result<(), Fail> {
    let config = Config::load_default().map_err(|e| Fail { code: "config", msg: e.to_string(), hint: "fix the config file" })?;
    let user_home = std::env::var_os("HOME");
    let hc = Hc::bin(&config.hc.bin, std::path::Path::new(user_home.as_deref().unwrap_or_default()));
    let store = if opts.dry_run { Store::open_read_only(home)? } else { Some(Store::open(home)?) };
    let chosen: Vec<Result<Bot, (String, BotError)>> = if names.is_empty() {
        bot::list(bots)?.into_iter().map(Ok).collect()
    } else {
        names.into_iter().map(|n| bot::find(bots, &n).map_err(|e| (n, e))).collect()
    };
    if chosen.is_empty() {
        println!("no bots in {}", bots.display());
    }
    let (mut incomplete, mut any_drifted) = (Vec::new(), false);
    for item in chosen {
        let b = match item {
            Ok(b) => b,
            // bots/ itself a link: nothing is read through it.
            Err((_, e @ BotError::Link(_))) => return Err(e.into()),
            Err((name, e)) => {
                println!("error: {name}: {e}");
                incomplete.push(format!("{name}: {e}"));
                continue;
            }
        };
        let def = match &b.def {
            None => {
                println!("error: {}: {}", b.name, first_error(&b));
                incomplete.push(format!("{}: invalid", b.name));
                continue;
            }
            Some(d) if d.kind == BotKind::Oneoff => {
                println!("{}: skipped, oneoff", b.name);
                continue;
            }
            Some(d) => d,
        };
        let target = sync::target(def);
        let last = match &store {
            Some(s) => s.pushed(&b.name, &target)?,
            None => None,
        };
        let mut synced = match sync::files_of(def, user_home.clone(), &hc, &NEVER) {
            Ok(mut there) => sync::sync(&b.dir, there.as_mut(), MANUAL, last.as_deref(), opts),
            Err(e) => sync::Synced::failed(e),
        };
        if let (Some(hash), Some(s), false) = (&synced.record, &store, opts.dry_run)
            && let Err(e) = s.set_pushed(&b.name, &target, hash, when::now())
        {
            synced.push = sync::Push::Failed;
            synced.why.push(format!("AGENTS.md is there, but the ledger did not take it: {e}"));
        }
        println!("{}", synced.line(&b.name));
        if !synced.complete() {
            let drifted = synced.push == sync::Push::Drifted;
            any_drifted |= drifted;
            let failed = synced.push == sync::Push::Failed || synced.pull == sync::Pull::Failed;
            let what = match (drifted, failed) {
                (true, false) => "AGENTS.md was changed there since the last sync",
                (true, true) => "AGENTS.md was changed there, and MEMORY.md failed",
                _ => "failed",
            };
            incomplete.push(format!("{}: {what}", b.name));
        }
    }
    if incomplete.is_empty() {
        return Ok(());
    }
    Err(Fail {
        code: "sync_incomplete",
        msg: incomplete.join("; "),
        hint: if any_drifted { "see the lines above; --force writes over a changed AGENTS.md" } else { "see the lines above" },
    })
}

fn first_error(b: &Bot) -> &str {
    b.problems.iter().find(|p| p.level == Level::Error).map_or("", |p| p.message.as_str())
}

/// `error: <bot>: <what>` or `warning: ...`, one a line.
fn print_problems(b: &Bot, indent: &str) {
    for p in &b.problems {
        println!("{indent}{}: {}: {}", p.level.as_str(), b.name, p.message);
    }
}

/// A bot for --json: its fields at the top (none when invalid), its
/// directory, its files by name, and its problems.
fn bot_json(b: &Bot) -> serde_json::Value {
    let mut v = match &b.def {
        Some(d) => serde_json::to_value(d).expect("plain data"),
        None => serde_json::json!({ "name": b.name }),
    };
    let files: serde_json::Map<_, _> = b.files.iter().map(|(f, p)| ((*f).to_owned(), p.display().to_string().into())).collect();
    let problems: Vec<_> = b.problems.iter().map(|p| serde_json::json!({ "level": p.level.as_str(), "message": p.message })).collect();
    v["valid"] = b.def.is_some().into();
    v["dir"] = b.dir.display().to_string().into();
    v["files"] = files.into();
    v["problems"] = problems.into();
    v
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
