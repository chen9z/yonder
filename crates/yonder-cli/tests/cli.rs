//! The `yonder` and `yonderd` binaries against a ledger in a temporary
//! state directory, with their own config (no notifications) and the fake
//! hc in tests/fake-hc.sh, which logs what it is sent to `hc.log`.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use yonder_core::when;

struct Ledger(PathBuf);

impl Ledger {
    fn new(name: &str) -> Ledger {
        Ledger::with_config(name, "")
    }

    fn with_config(name: &str, config: &str) -> Ledger {
        let dir = std::env::temp_dir().join(format!("yonder-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), format!("[notify]\nlocal = false\n{config}")).unwrap();
        Ledger(dir)
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.program(env!("CARGO_BIN_EXE_yonder"), args)
    }

    fn yonderd(&self, args: &[&str]) -> Command {
        self.program(env!("CARGO_BIN_EXE_yonderd"), args)
    }

    fn program(&self, bin: &str, args: &[&str]) -> Command {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .env("YONDER_STATE_DIR", &self.0)
            .env("YONDER_CONFIG", self.0.join("config.toml"))
            .env("YONDER_HC", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-hc.sh"))
            .env("FAKE_HC_LOG", self.0.join("hc.log"))
            .env("FAKE_HC_LEAD", "t9");
        cmd
    }

    /// One yonderd round at `now`, which must succeed.
    fn round(&self, now: i64, lead: &str) {
        let out = self.yonderd(&["--once", "--now", &when::format_utc(now)]).env("FAKE_HC_LEAD", lead).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
    }

    /// What the fake hc was sent.
    fn sent(&self) -> String {
        std::fs::read_to_string(self.0.join("hc.log")).unwrap_or_default()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    /// Stdout of a run that must succeed.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Vec<serde_json::Value> {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ids(items: &[serde_json::Value]) -> Vec<&str> {
    items.iter().map(|i| i["id"].as_str().unwrap()).collect()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn task_list() {
    let l = Ledger::new("task_list");
    let added = l.ok(&["task", "add", "nova 测试", "--source", "hc:nova"]);
    assert!(added.starts_with("T-0001 added: nova 测试"), "{added}");
    assert!(l.ok(&["task", "ls"]).contains("T-0001"));
    l.ok(&["task", "done", "T-0001"]);
    assert!(!l.ok(&["task", "ls"]).contains("T-0001"));
    let all = l.json(&["task", "ls", "--all", "--json"]);
    assert_eq!(ids(&all), ["T-0001"]);
    assert_eq!(all[0]["state"], "done");
    assert_eq!(all[0]["source"], "hc:nova");

    let again = l.run(&["task", "cancel", "T-0001"]);
    assert!(!again.status.success());
    assert!(stderr(&again).starts_with("wrong_state: T-0001 is done"), "{}", stderr(&again));
    let missing = l.run(&["task", "done", "T-0404"]);
    assert_eq!(stderr(&missing).lines().count(), 1);
    assert!(stderr(&missing).starts_with("not_found: "));
}

#[test]
fn reminder_due_snooze_cancel() {
    let l = Ledger::new("reminder_due_snooze_cancel");
    let now = when::now();
    let set = l.ok(&["remind", "set", "喝水", "--in", "2m"]);
    assert!(set.starts_with("R-0001 set: 喝水"), "{set}");
    let due = |t: i64| l.json(&["remind", "due", "--now", &when::format_utc(t), "--json"]);
    assert_eq!(ids(&due(now + 180)), ["R-0001"]);
    assert!(due(now).is_empty());

    l.ok(&["remind", "snooze", "R-0001", "--by", "1h"]);
    assert!(due(now + 180).is_empty());
    assert_eq!(ids(&due(now + 3700)), ["R-0001"]);

    l.ok(&["remind", "cancel", "R-0001"]);
    assert!(!l.ok(&["remind", "ls"]).contains("R-0001"));
    assert!(due(now + 3700).is_empty());
}

#[test]
fn daily_reminder_shows_next_local_time() {
    let l = Ledger::new("daily_reminder_shows_next_local_time");
    let before = when::now();
    l.ok(&["remind", "set", "早报", "--every", "daily@09:00"]);
    let listed = l.ok(&["remind", "ls"]);
    let next = when::Rule::parse("daily@09:00").unwrap().next_after(before).unwrap();
    assert!(listed.contains(&when::format_local(next)), "{listed}");
    assert!(listed.contains("daily@09:00"));
}

#[test]
fn failing_check_is_not_stored() {
    let l = Ledger::new("failing_check_is_not_stored");
    let out = l.run(&["remind", "set", "坏探针", "--every", "5m", "--check", "exit 3"]);
    assert!(!out.status.success());
    assert!(stderr(&out).starts_with("check_failed: exit:3; "), "{}", stderr(&out));
    assert!(l.json(&["remind", "ls", "--all", "--json"]).is_empty());

    let once = l.run(&["remind", "set", "x", "--in", "5m", "--check", "true"]);
    assert!(stderr(&once).starts_with("check_needs_every: "), "{}", stderr(&once));
}

#[test]
fn check_does_not_see_secrets() {
    let l = Ledger::new("check_does_not_see_secrets");
    let out = l
        .cmd(&["remind", "set", "环境", "--every", "5m", "--check", "env | grep -c SECRET_API_KEY || true"])
        .env("SECRET_API_KEY", "leak")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.ends_with("check baseline:\n0\n"), "{stdout}");
    let listed = l.json(&["remind", "ls", "--json"]);
    assert_eq!(listed[0]["check_cmd"], "env | grep -c SECRET_API_KEY || true");
    assert_eq!(listed[0]["check_hash"].as_str().unwrap().len(), 64);
}

#[test]
fn snooze_past_9999_changes_nothing() {
    let l = Ledger::new("snooze_past_9999_changes_nothing");
    l.ok(&["remind", "set", "x", "--in", "5m"]);
    let before = l.json(&["remind", "ls", "--json"]);
    let out = l.run(&["remind", "snooze", "R-0001", "--by", "9223372036854775807s"]);
    assert!(!out.status.success());
    assert!(stderr(&out).starts_with("bad_time: ") && stderr(&out).lines().count() == 1, "{}", stderr(&out));
    assert_eq!(l.json(&["remind", "ls", "--json"]), before);
}

#[test]
fn baseline_is_masked_before_it_is_cut() {
    let l = Ledger::new("baseline_is_masked_before_it_is_cut");
    // The key straddles the 1500th character: cut first, its head would
    // be too short to look like a key and would show.
    let check = "printf '%1489s' '' | tr ' ' x; echo ' sk-1234567890abcdefghijklmno'";
    let out = l.ok(&["remind", "set", "x", "--every", "5m", "--check", check]);
    assert!(!out.contains("sk-"), "{out}");
    assert!(out.contains("[REDACTED]"), "{out}");
}

#[test]
fn bad_input_is_one_line() {
    let l = Ledger::new("bad_input_is_one_line");
    for (args, code) in [
        (&["remind", "set", "x"][..], "usage: "),
        (&["remind", "set", "x", "--in", "5m", "--at", "2030-01-01 09:00"][..], "usage: "),
        (&["remind", "set", "x", "--in", "soon"][..], "bad_time: "),
        (&["remind", "set", "x", "--at", "2020-01-01 09:00"][..], "bad_time: "),
        (&["remind", "set", "x", "--every", "hourly"][..], "bad_rule: "),
        (&["remind", "set", "x", "--in", "5m", "--task", "T-0009"][..], "not_found: "),
        (&["remind", "due", "--now", "yesterday"][..], "bad_time: "),
        (&["remind", "set", "x", "--in", "9223372036854775807s"][..], "bad_time: "),
        (&["remind", "set", "x", "--in", "3000000d"][..], "bad_time: "),
        (&["remind", "set", "x", "--every", "99999999999d"][..], "bad_rule: "),
        (&["remind", "set", "x", "--every", "daily@-1:00"][..], "bad_rule: "),
        (&["remind", "set", "x", "--at", "2030-01-01 -1:00"][..], "bad_time: "),
    ] {
        let out = l.run(args);
        assert!(!out.status.success(), "{args:?}");
        let err = stderr(&out);
        assert!(err.starts_with(code) && err.lines().count() == 1, "{args:?}: {err}");
    }
}

#[test]
fn yonderd_fires_a_one_off_once() {
    let l = Ledger::new("yonderd_fires_a_one_off_once");
    let now = when::now();
    l.ok(&["remind", "set", "喝水", "--in", "1m"]);
    l.round(now + 120, "t9");
    let sent = l.sent();
    assert!(sent.starts_with(">>> send to t9\n[yonder kind=remind id=R-0001 trigger=time due="), "{sent}");
    assert!(sent.contains("]\n喝水\n"), "{sent}");
    assert_eq!(l.json(&["remind", "ls", "--all", "--json"])[0]["state"], "fired");
    l.round(now + 180, "t9");
    assert_eq!(l.sent().matches("id=R-0001").count(), 1);
}

#[test]
fn yonderd_moves_repeats_on() {
    let l = Ledger::new("yonderd_moves_repeats_on");
    let now = when::now();
    l.ok(&["remind", "set", "站会", "--every", "5m"]);
    l.round(now + 360, "t9");
    assert_eq!(l.sent().matches("id=R-0001").count(), 1);
    let r = &l.json(&["remind", "ls", "--json"])[0];
    assert_eq!(r["state"], "active");
    assert!(when::parse_utc(r["next_fire_at"].as_str().unwrap()).unwrap() > now + 360);
}

#[test]
fn yonderd_tells_a_changed_check() {
    let l = Ledger::new("yonderd_tells_a_changed_check");
    let f = l.0.join("watched");
    std::fs::write(&f, "a\n").unwrap();
    let now = when::now();
    l.ok(&["remind", "set", "文件变了", "--every", "1m", "--check", &format!("cat {}", f.display())]);
    l.round(now + 120, "t9");
    assert_eq!(l.sent(), "");
    let listed = l.ok(&["remind", "ls"]);
    assert!(listed.contains("[check: last ") && listed.contains(", 0 failed]"), "{listed}");
    std::fs::write(&f, "b sk-1234567890abcdefghijklmno\n").unwrap();
    l.round(now + 240, "t9");
    let sent = l.sent();
    assert_eq!(sent.matches("trigger=check_changed").count(), 1, "{sent}");
    assert!(sent.contains("Check output now:\n<payload untrusted=\"true\" source=\"check_output\">\nb [REDACTED]\n</payload>"), "{sent}");
}

#[test]
fn yonderd_tells_the_third_failure_once() {
    let l = Ledger::new("yonderd_tells_the_third_failure_once");
    let g = l.0.join("probe");
    std::fs::write(&g, "x\n").unwrap();
    let now = when::now();
    l.ok(&["remind", "set", "会坏", "--every", "1m", "--check", &format!("test -f {0} && cat {0}", g.display())]);
    std::fs::remove_file(&g).unwrap();
    for m in [2, 4, 6, 8] {
        l.round(now + 60 * m, "t9");
    }
    assert_eq!(l.sent().matches("trigger=check_failing").count(), 1, "{}", l.sent());
    let listed = l.ok(&["remind", "ls"]);
    assert!(listed.contains(", 4 failed (exit:1)]"), "{listed}");
}

#[test]
fn yonderd_keeps_what_it_cannot_deliver() {
    let l = Ledger::new("yonderd_keeps_what_it_cannot_deliver");
    let now = when::now();
    l.ok(&["remind", "set", "喝水", "--in", "1m"]);
    l.round(now + 120, "");
    assert_eq!(l.sent(), "");
    let status = l.ok(&["ctl", "status"]);
    assert!(status.contains("yonderd: not running") && status.contains("pending deliveries: 1"), "{status}");
    l.round(now + 135, "t9");
    l.round(now + 150, "t9");
    assert_eq!(l.sent().matches("id=R-0001").count(), 1);
    let status: serde_json::Value = serde_json::from_str(&l.ok(&["ctl", "status", "--json"])).unwrap();
    assert_eq!((&status["running"], &status["pending"]), (&false.into(), &0.into()), "{status}");
    assert_eq!(status["last_tick_at"], when::format_utc(now + 150));
}

#[test]
fn yonderd_runs_alone_and_stops_on_sigterm() {
    let l = Ledger::with_config("yonderd_runs_alone_and_stops_on_sigterm", "[daemon]\ntick_secs = 1\n");
    let mut first = l.yonderd(&[]).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !l.ok(&["ctl", "status"]).contains("yonderd: running, pid ") {
        assert!(Instant::now() < deadline, "yonderd never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = l.ok(&["ctl", "status"]);
    assert!(status.contains(&format!("pid {}", first.id())), "{status}");
    let second = l.yonderd(&[]).output().unwrap();
    assert!(!second.status.success());
    assert!(stderr(&second).starts_with("already_running: "), "{}", stderr(&second));
    assert!(stderr(&l.yonderd(&["--once"]).output().unwrap()).starts_with("already_running: "));

    // SAFETY: signals the child this test started.
    unsafe { libc::kill(first.id() as libc::pid_t, libc::SIGTERM) };
    let stopped = Instant::now();
    let status = loop {
        if let Some(status) = first.try_wait().unwrap() {
            break status;
        }
        assert!(stopped.elapsed() < Duration::from_secs(2), "still running 2s after SIGTERM");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0));
    let log = std::fs::read_to_string(l.0.join("logs/yonderd.log")).unwrap();
    assert!(log.contains("started, pid ") && log.ends_with("stopped\n"), "{log}");
    assert!(l.ok(&["ctl", "status"]).contains("yonderd: not running"));
}

#[test]
fn service_plist_is_valid() {
    let l = Ledger::new("service_plist_is_valid");
    let plist = l.ok(&["ctl", "service-plist"]);
    assert!(plist.contains("<string>dev.yonder.daemon</string>") && plist.contains("<key>KeepAlive</key>"), "{plist}");
    assert!(plist.contains("/yonderd</string>") && !plist.contains(".zshrc"));
    let Ok(mut lint) = Command::new("plutil").args(["-lint", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else {
        return; // Not macOS.
    };
    std::io::Write::write_all(&mut lint.stdin.take().unwrap(), plist.as_bytes()).unwrap();
    let out = lint.wait_with_output().unwrap();
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("OK"), "{out:?}");
}

#[test]
fn checks_last_seven_days_unless_told() {
    let l = Ledger::new("checks_last_seven_days_unless_told");
    let now = when::now();
    l.ok(&["remind", "set", "a", "--every", "5m", "--check", "echo a"]);
    l.ok(&["remind", "set", "b", "--every", "5m", "--check", "echo b", "--for", "forever", "--once"]);
    l.ok(&["remind", "set", "c", "--every", "5m", "--check", "echo c", "--for", "90d"]);
    let listed = l.json(&["remind", "ls", "--json"]);
    let until = |i: usize| listed[i]["check_until"].as_str().and_then(when::parse_utc);
    assert!(until(0).is_some_and(|t| (t - now - 7 * 86400).abs() < 60), "{listed:?}");
    assert_eq!((until(1), listed[1]["check_once"].as_bool()), (None, Some(true)));
    assert!(until(2).is_some_and(|t| (t - now - 90 * 86400).abs() < 60));
    for (args, code) in [
        (&["remind", "set", "x", "--every", "5m", "--check", "true", "--for", "91d"][..], "bad_time: "),
        (&["remind", "set", "x", "--every", "5m", "--check", "true", "--for", "soon"][..], "bad_time: "),
        (&["remind", "set", "x", "--every", "5m", "--for", "1d"][..], "usage: "),
        (&["remind", "set", "x", "--every", "5m", "--once"][..], "usage: "),
    ] {
        let out = l.run(args);
        assert!(!out.status.success() && stderr(&out).starts_with(code), "{args:?}: {}", stderr(&out));
    }
    assert_eq!(l.json(&["remind", "ls", "--all", "--json"]).len(), 3);
}

#[test]
fn the_last_minute_of_9999_west_of_greenwich() {
    let l = Ledger::new("the_last_minute_of_9999_west_of_greenwich");
    let out = l.cmd(&["remind", "set", "末年", "--at", "9999-12-31 23:59"]).env("TZ", "Etc/GMT+12").output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).starts_with("bad_time: "), "{}", stderr(&out));
    assert!(l.json(&["remind", "ls", "--all", "--json"]).is_empty());
}
