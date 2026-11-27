//! `yonderd`: fires reminders when they are due. Every `daemon.tick_secs`
//! it sends what is due to the lead agent's terminal (`hc send`) and shows
//! a notification here; it runs reminder checks and tells what changed.
//! One per Yonder home; SIGTERM or SIGINT stop it within two seconds.
//! Logs to `logs/yonderd.log` in Yonder's home.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use yonder_core::config::Config;
use yonder_core::daemon::{Daemon, InstanceLock, LocalNotifier, running_pid};
use yonder_core::handoff;
use yonder_core::hc::{Hc, HcBots};
use yonder_core::when::{self, format_utc};
use yonder_core::{Store, store};

/// The log is started afresh (the old one kept as `.1`) past this.
const MAX_LOG: u64 = 10 << 20;

static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(name = "yonderd", version, about = "Fires Yonder's reminders when they are due")]
struct Cli {
    /// One round, then exit.
    #[arg(long)]
    once: bool,
    /// The round's time instead of now: YYYY-MM-DDTHH:MM:SSZ.
    #[arg(long, value_name = "UTC", requires = "once")]
    now: Option<String>,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if !e.use_stderr() => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let text = e.render().to_string();
            let line = text.lines().next().unwrap_or("").trim_start_matches("error: ").to_string();
            eprintln!("usage: {line}; see yonderd --help");
            return ExitCode::from(2);
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, msg, hint)) => {
            eprintln!("{code}: {msg}; {hint}");
            ExitCode::FAILURE
        }
    }
}

type Fail = (&'static str, String, &'static str);

fn run(cli: Cli) -> Result<(), Fail> {
    let now = match &cli.now {
        None => None,
        Some(s) => Some(when::parse_utc(s).ok_or(("bad_time", format!("{s:?} is not a UTC time"), "use YYYY-MM-DDTHH:MM:SSZ"))?),
    };
    let config = Config::load_default().map_err(|e| ("config", e.to_string(), "fix the config file"))?;
    let state_err = |e: store::StoreError| {
        let hint = if e.code() == "bad_home" { yonder_core::home::ABSOLUTE } else { "set YONDER_HOME to a writable directory" };
        (e.code(), e.to_string(), hint)
    };
    let dir = store::home_dir().map_err(state_err)?;
    let store = Store::open(&dir).map_err(state_err)?;
    let io_err = |what: &str, e: std::io::Error| ("home", format!("{what}: {e}"), "check the permissions of Yonder's home");
    let Some(_lock) = InstanceLock::acquire(&dir).map_err(|e| io_err("the lock", e))? else {
        let pid = running_pid(&dir).ok().flatten().map_or("another".into(), |p| format!("pid {p}"));
        return Err(("already_running", format!("yonderd {pid} uses {}", dir.display()), "stop it first, or use another YONDER_HOME"));
    };
    let log = Rc::new(RefCell::new(Log::open(dir.join("logs"), cli.once).map_err(|e| io_err("the log", e))?));
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only stores to an atomic.
        unsafe { libc::signal(sig, on_signal as *const () as libc::sighandler_t) };
    }

    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let hc = Hc { bin: Hc::bin(&config.hc.bin, &home), label: config.lead.label.clone(), stop: &STOP };
    let bots = HcBots { bin: hc.bin.clone(), stop: &STOP };
    // Each round, after the lead's: handoffs to the bots free for them.
    let handoffs = |store: &Store, log: &Rc<RefCell<Log>>| {
        if STOP.load(Ordering::Relaxed) {
            return;
        }
        match handoff::deliver_all(&dir, store, &bots, &STOP) {
            Ok(lines) => lines.iter().for_each(|l| log.borrow_mut().line(l)),
            Err(e) => log.borrow_mut().line(&format!("handoffs: {e}")),
        }
    };
    let notifier = LocalNotifier { enabled: config.notify.local, stop: &STOP };
    let to_log = log.clone();
    let mut daemon = Daemon::new(store, hc, notifier, &STOP, Box::new(move |line| to_log.borrow_mut().line(line)))
        .map_err(|e| (e.code(), e.to_string(), "look at ledger.db in Yonder's home"))?;

    if cli.once {
        let now = now.unwrap_or_else(when::now);
        let ticked = daemon.tick(now, true);
        handoffs(&daemon.store, &log);
        // Stopped mid-round: the checks still running end now, not orphaned.
        daemon.finish();
        return ticked.map_err(|e| (e.code(), e.to_string(), "try again; the log says more"));
    }
    let tick = Duration::from_secs(config.daemon.tick_secs);
    log.borrow_mut().line(&format!("started, pid {}, a round every {}s", std::process::id(), tick.as_secs()));
    let mut last = when::now();
    while !STOP.load(Ordering::Relaxed) {
        let started = Instant::now();
        let now = when::now();
        // Far longer than a round: the computer slept. Repeats missed meanwhile go off once.
        if now - last > 5 * tick.as_secs() as i64 {
            log.borrow_mut().line(&format!("woke after {}s", now - last));
        }
        last = now;
        if let Err(e) = daemon.tick(now, false) {
            log.borrow_mut().line(&format!("round failed: {e}"));
        }
        handoffs(&daemon.store, &log);
        while !STOP.load(Ordering::Relaxed) && started.elapsed() < tick {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    daemon.finish();
    log.borrow_mut().line("stopped");
    Ok(())
}

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// `yonderd.log`, moved to `yonderd.log.1` when it grows past `MAX_LOG`.
struct Log {
    path: PathBuf,
    file: File,
    max: u64,
    /// Also on stderr (with --once).
    echo: bool,
}

impl Log {
    fn open(dir: PathBuf, echo: bool) -> std::io::Result<Log> {
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("yonderd.log");
        Ok(Log { file: append(&path)?, path, max: MAX_LOG, echo })
    }

    fn line(&mut self, msg: &str) {
        let line = format!("{} {msg}\n", format_utc(when::now()));
        if self.echo {
            eprint!("{line}");
        }
        if self.file.metadata().is_ok_and(|m| m.len() >= self.max) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
            if let Ok(file) = append(&self.path) {
                self.file = file;
            }
        }
        let _ = self.file.write_all(line.as_bytes());
    }
}

fn append(path: &std::path::Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_starts_afresh_when_full() {
        let dir = std::env::temp_dir().join(format!("yonderd-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = Log::open(dir.clone(), false).unwrap();
        log.max = 100;
        for i in 0..10 {
            log.line(&format!("line {i} of a log that fills up"));
        }
        let now = std::fs::read_to_string(dir.join("yonderd.log")).unwrap();
        let old = std::fs::read_to_string(dir.join("yonderd.log.1")).unwrap();
        assert!(now.len() < 200 && old.len() < 200, "{now:?} {old:?}");
        assert!(now.ends_with("line 9 of a log that fills up\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
