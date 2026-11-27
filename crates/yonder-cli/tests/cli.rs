//! The `yonder` and `yonderd` binaries against a ledger in a temporary
//! Yonder home, with their own config (no notifications) and the fake
//! hc in tests/fake-hc.sh, which logs what it is sent to `hc.log` and
//! keeps other machines' files under `hc/`.

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
            .env("YONDER_HOME", &self.0)
            .env("YONDER_HC", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-hc.sh"))
            .env("FAKE_HC_LOG", self.0.join("hc.log"))
            .env("FAKE_HC_ROOT", self.0.join("hc"))
            // Bots' harnesses go through the login shell: sh, not the
            // developer's own, with whatever its startup files do.
            .env("SHELL", "/bin/sh")
            .env("FAKE_HC_LEAD", "t9");
        cmd
    }

    /// One yonderd round at `now`, which must succeed.
    fn round(&self, now: i64, lead: &str) {
        self.round_with(now, lead, "");
    }

    /// One round with the fake hc's send failing with `send_error`, if not empty.
    fn round_with(&self, now: i64, lead: &str, send_error: &str) {
        let mut cmd = self.yonderd(&["--once", "--now", &when::format_utc(now)]);
        cmd.env("FAKE_HC_LEAD", lead).env("FAKE_HC_SEND_ERROR", send_error);
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
    }

    fn status(&self) -> serde_json::Value {
        serde_json::from_str(&self.ok(&["ctl", "status", "--json"])).unwrap()
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
fn yonderd_stops_in_time_with_a_check_running() {
    let l = Ledger::with_config("yonderd_stops_in_time_with_a_check_running", "[daemon]\ntick_secs = 1\n");
    // Due now, and writing without end once it runs.
    let mut store = yonder_core::Store::open(&l.0).unwrap();
    let new = yonder_core::NewReminder {
        content: "endless".into(),
        next_fire_at: when::now() - 1,
        every: when::Rule::parse("1m"),
        check: Some(("yes".into(), "baseline".into())),
        check_once: false,
        check_until: None,
        task_id: None,
    };
    store.add_reminder(&new, when::now()).unwrap();
    drop(store);
    let mut daemon = l.yonderd(&[]).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while l.json(&["remind", "ls", "--json"])[0]["next_fire_at"] == when::format_utc(new.next_fire_at) {
        assert!(Instant::now() < deadline, "the check never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    // SAFETY: signals the child this test started.
    unsafe { libc::kill(daemon.id() as libc::pid_t, libc::SIGTERM) };
    let stopped = Instant::now();
    let status = loop {
        if let Some(status) = daemon.try_wait().unwrap() {
            break status;
        }
        assert!(stopped.elapsed() < Duration::from_secs(2), "still running 2s after SIGTERM");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0));
    // The stopped check left nothing behind.
    let r = &l.json(&["remind", "ls", "--json"])[0];
    assert_eq!((&r["check_failures"], &r["last_checked_at"]), (&0.into(), &serde_json::Value::Null), "{r}");
}

#[test]
fn yonderd_once_stops_in_time_with_a_check_running() {
    let l = Ledger::new("yonderd_once_stops_in_time_with_a_check_running");
    let now = when::now();
    // Set with a quick baseline, then made slow, so the round's run hangs.
    let flag = l.0.join("slow");
    let cmd = format!("if [ -f {0} ]; then sleep 30; fi; echo ok", flag.display());
    l.ok(&["remind", "set", "slow", "--every", "1m", "--check", &cmd]);
    std::fs::write(&flag, "").unwrap();
    let before = l.json(&["remind", "ls", "--json"])[0]["next_fire_at"].clone();
    let mut once = l.yonderd(&["--once", "--now", &when::format_utc(now + 120)]).stderr(Stdio::null()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    // The round has fired it (moved it on) and is waiting for its check.
    while l.json(&["remind", "ls", "--json"])[0]["next_fire_at"] == before {
        assert!(Instant::now() < deadline, "the check never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(once.try_wait().unwrap().is_none(), "the round should still be waiting for the check");
    // SAFETY: signals the child this test started.
    unsafe { libc::kill(once.id() as libc::pid_t, libc::SIGTERM) };
    let stopped = Instant::now();
    let status = loop {
        if let Some(status) = once.try_wait().unwrap() {
            break status;
        }
        if stopped.elapsed() > Duration::from_secs(2) {
            let _ = once.kill();
            panic!("yonderd --once still running 2s after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0));
    let r = &l.json(&["remind", "ls", "--json"])[0];
    assert_eq!((&r["check_failures"], &r["last_checked_at"]), (&0.into(), &serde_json::Value::Null), "{r}");
}

#[test]
fn hc_errors_after_typing_are_never_sent_again() {
    let l = Ledger::new("hc_errors_after_typing_are_never_sent_again");
    let now = when::now();
    l.ok(&["remind", "set", "喝水", "--in", "1m"]);
    // Typed, then hc failed with a kind that is not a refusal.
    l.round_with(now + 120, "t9", "timeout");
    assert_eq!(l.sent().matches("id=R-0001 ").count(), 1);
    assert_eq!((&l.status()["pending"], &l.status()["lost"]), (&0.into(), &1.into()));
    for t in [135, 150, 165] {
        l.round(now + t, "t9");
    }
    assert_eq!(l.sent().matches("id=R-0001 ").count(), 1, "{}", l.sent());
    assert_eq!(l.status()["lost"], 1);
}

#[test]
fn hc_refusals_are_sent_again() {
    let l = Ledger::new("hc_refusals_are_sent_again");
    let now = when::now();
    l.ok(&["remind", "set", "喝水", "--in", "1m"]);
    // Refused before typing: nothing arrived, so it waits.
    l.round_with(now + 120, "t9", "not_found");
    assert_eq!(l.sent(), "");
    assert_eq!((&l.status()["pending"], &l.status()["lost"]), (&1.into(), &0.into()));
    l.round(now + 135, "t9");
    l.round(now + 150, "t9");
    assert_eq!(l.sent().matches("id=R-0001 ").count(), 1, "{}", l.sent());
    assert_eq!((&l.status()["pending"], &l.status()["lost"]), (&0.into(), &0.into()));
}

#[test]
fn reminders_have_a_length_limit() {
    let l = Ledger::new("reminders_have_a_length_limit");
    let long = "长".repeat(1500);
    let out = l.run(&["remind", "set", &long, "--in", "5m"]);
    assert!(!out.status.success());
    assert!(stderr(&out).starts_with("bad_input: the reminder is 4500 bytes, more than 4096; "), "{}", stderr(&out));
    assert!(l.json(&["remind", "ls", "--all", "--json"]).is_empty());
    l.ok(&["remind", "set", &"x".repeat(4096), "--in", "5m"]);
}

#[test]
fn status_counts_lost_deliveries() {
    let l = Ledger::new("status_counts_lost_deliveries");
    let status: serde_json::Value = serde_json::from_str(&l.ok(&["ctl", "status", "--json"])).unwrap();
    assert_eq!(status["lost"], 0);
    assert!(l.ok(&["ctl", "status"]).contains("lost deliveries: 0"));
}

#[test]
fn service_plist_is_valid() {
    let l = Ledger::new("service_plist_is_valid");
    let plist = l.ok(&["ctl", "service-plist"]);
    assert!(plist.contains("<string>dev.yonder.daemon</string>") && plist.contains("<key>KeepAlive</key>"), "{plist}");
    assert!(plist.contains("/yonderd</string>") && !plist.contains(".zshrc"));
    // yonderd gets the same home.
    assert!(plist.contains("<key>YONDER_HOME</key>") && plist.contains(&format!("<string>{}</string>", l.0.display())), "{plist}");
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

#[test]
fn everything_is_in_the_yonder_home() {
    let l = Ledger::new("everything_is_in_the_yonder_home");
    l.ok(&["remind", "set", "x", "--in", "1h"]);
    l.round(when::now(), "t9");
    for kept in ["ledger.db", "yonderd.lock", "logs/yonderd.log"] {
        assert!(l.0.join(kept).exists(), "{kept}");
    }
    // Without YONDER_HOME: ~/.yonder, here a temporary HOME's.
    let home = l.0.join("someone");
    std::fs::create_dir(&home).unwrap();
    let out = l.cmd(&["task", "add", "x"]).env_remove("YONDER_HOME").env("HOME", &home).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(home.join(".yonder/ledger.db").is_file());
    assert!(!home.join(".local").exists() && !home.join(".config").exists());
}

#[test]
fn bots_are_made_listed_and_checked() {
    let l = Ledger::new("bots_are_made_listed_and_checked");
    let bots = l.0.join("bots");
    l.ok(&["bot", "new", "trainer", "--kind", "persistent", "--harness", "codex"]);
    for f in ["bot.toml", "template.md", "AGENTS.md"] {
        assert!(bots.join("trainer").join(f).is_file(), "{f}");
    }
    let again = l.run(&["bot", "new", "trainer"]);
    assert!(!again.status.success() && stderr(&again).starts_with("bot_exists: "), "{}", stderr(&again));

    let check = l.run(&["bot", "check", "trainer"]);
    let said = String::from_utf8_lossy(&check.stdout);
    assert_eq!(check.status.code(), Some(1));
    assert!(said.contains("error: trainer: a persistent bot needs cwd"), "{said}");
    let toml = bots.join("trainer/bot.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, format!("{text}cwd = \"/tmp/example\"\ncolour = \"red\"\n")).unwrap();
    let said = l.ok(&["bot", "check", "trainer"]);
    assert!(said.contains("warning: trainer: unknown field colour") && !said.contains("error:"), "{said}");

    // One broken, one misnamed: listed, and the others still are.
    std::fs::create_dir_all(bots.join("broken")).unwrap();
    std::fs::write(bots.join("broken/bot.toml"), "name = \"broken\"\nkind = ").unwrap();
    std::fs::create_dir_all(bots.join("ops")).unwrap();
    std::fs::write(bots.join("ops/bot.toml"), "name = \"nova\"\ndescription = \"x\"\nkind = \"oneoff\"\nharness = \"pi\"\n").unwrap();
    let listed = l.ok(&["bot", "ls"]);
    assert!(listed.lines().any(|line| line.starts_with("broken ") && line.contains("invalid") && line.contains("line 2")), "{listed}");
    assert!(listed.lines().any(|line| line.starts_with("trainer ") && line.contains("persistent") && line.contains("codex")), "{listed}");
    let all = l.json(&["bot", "ls", "--json"]);
    let valid: Vec<_> = all.iter().map(|b| (b["name"].as_str().unwrap(), b["valid"].as_bool().unwrap())).collect();
    assert_eq!(valid, [("broken", false), ("ops", false), ("trainer", true)]);
    let check = l.run(&["bot", "check"]);
    let said = String::from_utf8_lossy(&check.stdout);
    assert_eq!(check.status.code(), Some(1));
    assert!(said.contains("error: ops: name \"nova\" is not its directory's name"), "{said}");

    let shown: serde_json::Value = serde_json::from_str(&l.ok(&["bot", "show", "trainer", "--json"])).unwrap();
    assert_eq!((shown["name"].as_str(), shown["kind"].as_str(), shown["cwd"].as_str()), (Some("trainer"), Some("persistent"), Some("/tmp/example")));
    assert_eq!(shown["files"]["bot.toml"].as_str().map(std::path::PathBuf::from), Some(toml));
    let missing = l.run(&["bot", "show", "nova"]);
    assert!(!missing.status.success() && stderr(&missing).starts_with("not_found: "), "{}", stderr(&missing));
    let bad = l.run(&["bot", "new", "Nova"]);
    assert!(!bad.status.success() && stderr(&bad).starts_with("bad_name: "), "{}", stderr(&bad));
}

#[test]
fn a_relative_home_is_refused() {
    let l = Ledger::new("a_relative_home_is_refused");
    for (what, mut cmd) in [
        ("yonder", l.cmd(&["task", "ls"])),
        ("yonder bot", l.cmd(&["bot", "new", "trainer"])),
        ("yonder ctl service-plist", l.cmd(&["ctl", "service-plist"])),
        ("yonderd", l.yonderd(&["--once"])),
    ] {
        let out = cmd.env("YONDER_HOME", "relative/home").current_dir(&l.0).output().unwrap();
        assert!(!out.status.success(), "{what}");
        let said = stderr(&out);
        assert!(said.starts_with("bad_home: ") && said.contains("absolute path") && said.lines().count() == 1, "{what}: {said}");
    }
    assert!(!l.0.join("relative").exists(), "something was made in the relative home");
}

#[test]
fn bot_new_in_a_new_home_keeps_it_the_owners() {
    use std::os::unix::fs::PermissionsExt;
    let l = Ledger::new("bot_new_in_a_new_home_keeps_it_the_owners");
    let home = l.0.join("fresh");
    let out = l.cmd(&["bot", "new", "trainer"]).env("YONDER_HOME", &home).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn the_manual_is_for_agents() {
    let l = Ledger::new("the_manual_is_for_agents");
    let manual = l.ok(&["manual"]);
    assert!((30..=200).contains(&manual.lines().count()), "{} lines", manual.lines().count());
    for said in ["[yonder kind=", "untrusted", "yonder task add", "yonder remind set", "MEMORY.md"] {
        assert!(manual.contains(said), "{said}");
    }
}

fn sync(l: &Ledger, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let mut cmd = l.cmd(&[&["bot", "sync"], args].concat());
    cmd.envs(env.iter().copied());
    let out = cmd.output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), stderr(&out))
}

#[test]
fn bots_sync_to_where_they_work() {
    let l = Ledger::new("bots_sync_to_where_they_work");
    let manual = l.ok(&["manual"]);
    let (bots, work) = (l.0.join("bots"), l.0.join("ops-work"));
    let define = |name: &str, toml: &str, rules: Option<&str>| {
        std::fs::create_dir_all(bots.join(name)).unwrap();
        std::fs::write(bots.join(name).join("bot.toml"), toml).unwrap();
        if let Some(rules) = rules {
            std::fs::write(bots.join(name).join("AGENTS.md"), rules).unwrap();
        }
    };
    std::fs::create_dir(&work).unwrap();
    let ops = format!("name = \"ops\"\ndescription = \"x\"\nkind = \"persistent\"\ncwd = \"{}\"\nharness = \"codex\"\n", work.display());
    define("ops", &ops, Some("ops rules\n"));
    let target = work.join("AGENTS.md");
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();

    let (code, out, _) = sync(&l, &["ops"], &[]);
    assert_eq!((code, out.as_str()), (Some(0), "ops: AGENTS.md pushed; MEMORY.md missing\n"));
    let pushed = read(&target);
    assert!(pushed.starts_with("ops rules\n\n<!-- yonder manual: generated by yonder bot sync; edits below are overwritten -->\n\n"));
    assert!(pushed.ends_with(&manual));
    assert_eq!(sync(&l, &["ops"], &[]).1, "ops: AGENTS.md unchanged; MEMORY.md missing\n");

    std::fs::write(work.join("MEMORY.md"), "m1\n").unwrap();
    assert_eq!(sync(&l, &["ops"], &[]).1, "ops: AGENTS.md unchanged; MEMORY.md pulled\n");
    assert_eq!(read(&bots.join("ops/MEMORY.md")), "m1\n");
    std::fs::remove_file(work.join("MEMORY.md")).unwrap();
    assert_eq!(sync(&l, &["ops"], &[]).1, "ops: AGENTS.md unchanged; MEMORY.md missing\n");
    assert_eq!(read(&bots.join("ops/MEMORY.md")), "m1\n");

    // Changed there: left alone until forced.
    std::fs::write(&target, format!("{pushed}hand edit\n")).unwrap();
    let (code, out, err) = sync(&l, &["ops"], &[]);
    assert_eq!((code, out.as_str()), (Some(1), "ops: AGENTS.md drifted; MEMORY.md missing\n"));
    assert_eq!(err, "sync_incomplete: ops: AGENTS.md was changed there since the last sync; see the lines above; --force writes over a changed AGENTS.md\n");
    assert!(read(&target).contains("hand edit"));
    assert_eq!(sync(&l, &["ops", "--force"], &[]).0, Some(0));
    assert_eq!(read(&target), pushed);

    // On another machine, through hc.
    let trainer = "name = \"trainer\"\ndescription = \"x\"\nkind = \"persistent\"\nmachine = \"gpu-box\"\ncwd = \"~/bots/trainer\"\nharness = \"codex\"\n";
    define("trainer", trainer, Some("trainer rules\n"));
    let there = l.0.join("hc/gpu-box/home/bots/trainer");
    assert_eq!(sync(&l, &["trainer"], &[]).1, "trainer: AGENTS.md pushed; MEMORY.md missing\n");
    assert!(read(&there.join("AGENTS.md")).starts_with("trainer rules\n\n<!-- yonder manual"));
    assert!(l.sent().contains(">>> file write --on gpu-box bots/trainer/AGENTS.md\n"), "{}", l.sent());
    std::fs::write(there.join("MEMORY.md"), "t-mem\n").unwrap();
    assert_eq!(sync(&l, &["trainer"], &[]).1, "trainer: AGENTS.md unchanged; MEMORY.md pulled\n");
    assert_eq!(read(&bots.join("trainer/MEMORY.md")), "t-mem\n");

    // hc failing for one: the other goes on.
    let (code, out, err) = sync(&l, &["trainer", "ops"], &[("FAKE_HC_FILE_ERROR", "machine_unknown")]);
    assert_eq!(code, Some(1));
    let lines: Vec<_> = out.lines().collect();
    assert!(lines[0].starts_with("trainer: AGENTS.md failed; MEMORY.md failed — hc file read: exit:3: machine_unknown"), "{out}");
    assert_eq!(lines[1], "ops: AGENTS.md unchanged; MEMORY.md missing");
    assert!(err.starts_with("sync_incomplete: trainer: failed"), "{err}");

    // A oneoff one is skipped, an invalid one told; the rest go on.
    define("nova", "name = \"nova\"\ndescription = \"x\"\nkind = \"oneoff\"\nharness = \"pi\"\n", Some("nova rules\n"));
    define("broken", "name = \"broken\"\nkind = \n", None);
    let (code, out, err) = sync(&l, &[], &[]);
    assert_eq!(code, Some(1));
    assert_eq!(out, "error: broken: bot.toml line 2: string values must be quoted, expected literal string\nnova: skipped, oneoff\nops: AGENTS.md unchanged; MEMORY.md missing\ntrainer: AGENTS.md unchanged; MEMORY.md unchanged\n");
    assert_eq!(err, "sync_incomplete: broken: invalid; see the lines above\n");
    assert!(sync(&l, &["homelab"], &[]).1.starts_with("error: homelab: no bot homelab"));
    std::fs::remove_dir_all(bots.join("nova")).unwrap();
    std::fs::remove_dir_all(bots.join("broken")).unwrap();

    // A dry run says, writes nothing, and leaves the ledger as it was.
    std::fs::write(bots.join("ops/AGENTS.md"), "ops rules v2\n").unwrap();
    std::fs::write(there.join("MEMORY.md"), "t-mem 2\n").unwrap();
    let writes = l.sent().matches("file write").count();
    let (code, out, _) = sync(&l, &["--dry-run"], &[]);
    assert_eq!((code, out.as_str()), (Some(0), "ops: AGENTS.md pushed; MEMORY.md missing\ntrainer: AGENTS.md unchanged; MEMORY.md pulled\n"));
    assert_eq!((read(&target), read(&bots.join("trainer/MEMORY.md"))), (pushed.clone(), "t-mem\n".into()));
    assert_eq!(l.sent().matches("file write").count(), writes);
    let (code, out, _) = sync(&l, &[], &[]);
    assert_eq!((code, out.as_str()), (Some(0), "ops: AGENTS.md pushed; MEMORY.md missing\ntrainer: AGENTS.md unchanged; MEMORY.md pulled\n"));
    assert!(read(&target).starts_with("ops rules v2\n"));
}

#[test]
fn a_dry_run_makes_no_ledger() {
    let l = Ledger::new("a_dry_run_makes_no_ledger");
    let (code, out, _) = sync(&l, &["--dry-run"], &[]);
    assert_eq!((code, out.trim_end().ends_with("bots")), (Some(0), true), "{out}");
    assert!(!l.0.join("ledger.db").exists());
}

/// A bot `name` in `l`'s home, its bot.toml `toml` and template `template`.
fn bot(l: &Ledger, name: &str, toml: &str, template: Option<&str>) {
    let dir = l.0.join("bots").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bot.toml"), toml).unwrap();
    if let Some(t) = template {
        std::fs::write(dir.join("template.md"), t).unwrap();
    }
}

/// Runs `args` with `env` set; its exit code, stdout and stderr.
fn with_env(l: &Ledger, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let out = l.cmd(args).envs(env.iter().copied()).output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), stderr(&out))
}

fn runs(l: &Ledger) -> Vec<String> {
    l.sent().lines().filter(|line| line.starts_with(">>> run ")).map(String::from).collect()
}

#[test]
fn bots_start_get_ready_and_stop() {
    let l = Ledger::new("bots_start_get_ready_and_stop");
    let work = l.0.join("ops-work");
    std::fs::create_dir(&work).unwrap();
    let ops = format!("name = \"ops\"\ndescription = \"x\"\nkind = \"persistent\"\ncwd = \"{}\"\nharness = \"/bin/sh\"\nargs = [\"-c\", \"sleep 1\"]\nmemory_id = \"ops-mem\"\n", work.display());
    bot(&l, "ops", &ops, Some("---\ntitle: ops\n---\nhello ops\n"));
    let (code, out, err) = with_env(&l, &["bot", "start", "ops"], &[]);
    assert_eq!((code, out.as_str()), (Some(0), "ops: ready at t70\n"), "{err}");
    let run = format!(
        ">>> run --bg --json --name ops --label yonder.bot=ops --outlive-parent --keep --cwd {} -- env NMEM_AGENT_ID=ops-mem /bin/sh -c {} yonder-bot /bin/sh -c sleep 1",
        work.display(),
        yonder_core::botrun::LOGIN_EXEC
    );
    assert_eq!(runs(&l), [run]);
    assert!(l.sent().contains(">>> send to t70\nhello ops\n"), "{}", l.sent());
    assert!(l.ok(&["bot", "status", "ops"]).starts_with("ops               ready      t70"));
    let status: Vec<serde_json::Value> = l.json(&["bot", "status", "--json"]);
    assert_eq!((status[0]["state"].as_str(), status[0]["address"].as_str()), (Some("ready"), Some("t70")));

    let (code, _, err) = with_env(&l, &["bot", "start", "ops"], &[]);
    assert_eq!(code, Some(1));
    assert_eq!(err, "already_running: ops runs at t70\n");
    assert_eq!(runs(&l).len(), 1);

    assert_eq!(l.ok(&["bot", "stop", "ops"]), "ops: interrupted at t70 (hc says: unknown)\n");
    assert!(l.sent().contains(">>> interrupt t70\n"));
    let kill = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-hc.sh"))
        .args(["kill", "t70"])
        .env("FAKE_HC_ROOT", l.0.join("hc"))
        .env("FAKE_HC_LOG", l.0.join("hc.log"))
        .output()
        .unwrap();
    assert!(kill.status.success());
    assert!(l.ok(&["bot", "status", "ops"]).contains(" gone "));
    let (code, _, err) = with_env(&l, &["bot", "stop", "ops"], &[]);
    assert_eq!((code, err.as_str()), (Some(1), "not_running: ops has no ready terminal\n"));
    // Gone is final: it starts again.
    assert_eq!(with_env(&l, &["bot", "start", "ops"], &[]).1, "ops: ready at t71\n");
}

#[test]
fn bots_pause_resume_reset_and_go_to_the_trash() {
    let l = Ledger::new("bots_pause_resume_reset_and_go_to_the_trash");
    let trainer = "name = \"trainer\"\ndescription = \"x\"\nkind = \"persistent\"\nmachine = \"gpu-box\"\ncwd = \"~/bots/trainer\"\nharness = \"codex\"\n";
    bot(&l, "trainer", trainer, Some("hello trainer\n"));
    let calls = |l: &Ledger| l.sent().lines().filter(|x| x.starts_with(">>> ")).count();
    // Not running: only marked.
    assert_eq!(l.ok(&["bot", "pause", "trainer"]), "trainer: paused (it was not running); its session stays\n");
    assert_eq!(l.ok(&["bot", "pause", "trainer"]), "trainer: already paused\n");
    assert_eq!(calls(&l), 0);
    let (code, _, err) = with_env(&l, &["bot", "start", "trainer"], &[]);
    assert_eq!((code, err.as_str()), (Some(1), "bot_paused: trainer is paused; yonder bot resume trainer starts it\n"));
    assert_eq!(calls(&l), 0, "a paused bot asks hc nothing");
    assert!(l.ok(&["bot", "status", "trainer"]).starts_with("trainer           paused "));

    l.ok(&["bot", "session", "trainer", "s-1"]);
    assert_eq!(l.ok(&["bot", "resume", "trainer"]), "trainer: ready at gpu-box/t70\n");
    assert!(runs(&l)[0].contains(" codex resume s-1"), "{:?}", runs(&l));
    let (code, _, err) = with_env(&l, &["bot", "resume", "trainer"], &[]);
    assert_eq!((code, err.as_str()), (Some(1), "not_paused: trainer is not paused; yonder bot start trainer starts it\n"));

    // Running: ctrl+c, then kill; the session stays.
    assert_eq!(l.ok(&["bot", "pause", "trainer"]), "trainer: paused; ended gpu-box/t70 (kept for hc read); its session stays\n");
    assert!(l.sent().contains(">>> interrupt gpu-box/t70\n>>> kill gpu-box/t70\n"), "{}", l.sent());
    let status: Vec<serde_json::Value> = l.json(&["bot", "status", "trainer", "--json"]);
    assert_eq!((status[0]["state"].as_str(), status[0]["reason"].as_str(), status[0]["session"].as_str()), (Some("paused"), Some("paused"), Some("s-1")));

    // Reset: confirmed, or nothing changes.
    std::fs::write(l.0.join("bots/trainer/MEMORY.md"), "mem-head-line\n").unwrap();
    let before = calls(&l);
    let (code, _, err) = with_env(&l, &["bot", "reset", "trainer"], &[]);
    assert_eq!((code, err.as_str()), (Some(1), "confirm_needed: reset ends trainer's terminal and forgets its session for good; add --yes\n"));
    assert_eq!((calls(&l), l.ok(&["bot", "session", "trainer"]).as_str()), (before, "s-1\n"));
    let out = l.ok(&["bot", "reset", "trainer", "--yes"]);
    assert!(out.starts_with("trainer: ready at gpu-box/t71\n"), "{out}");
    assert_eq!(l.ok(&["bot", "session", "trainer"]), "none\n");
    assert!(!runs(&l)[1].contains("resume"), "{:?}", runs(&l));
    let sent = l.sent();
    let first = sent.split(">>> send to gpu-box/t71\n").nth(1).unwrap();
    assert!(first.starts_with("hello trainer\n\n[yonder kind=recap bot=trainer]\n"), "{first}");
    assert!(first.contains("<payload untrusted=\"true\" source=\"memory\">\nmem-head-line\n</payload>"), "{first}");

    // Diagnostics: what is kept, not what files hold.
    let diag = l.ok(&["bot", "diagnostics", "trainer"]);
    for want in ["hc          hc 0.0.0-fake\n", "harness     codex\n", "paused      false\n", "run         ready at gpu-box/t71", "MEMORY.md   14 bytes\n", "files       bot.toml template.md MEMORY.md\n"] {
        assert!(diag.contains(want), "{want:?} in {diag}");
    }
    assert!(!diag.contains("mem-head-line") && !diag.contains("hello trainer"), "{diag}");
    let diag: serde_json::Value = serde_json::from_str(&l.ok(&["bot", "diagnostics", "trainer", "--json"])).unwrap();
    assert_eq!(diag["run"]["address"], "gpu-box/t71");

    // rm: never while running; confirmed; to the trash, and forgotten.
    let (code, _, err) = with_env(&l, &["bot", "rm", "trainer", "--yes"], &[]);
    assert_eq!((code, err.as_str()), (Some(1), "bot_running: trainer runs at gpu-box/t71; yonder bot pause trainer first\n"));
    l.ok(&["bot", "pause", "trainer"]);
    let (code, _, err) = with_env(&l, &["bot", "rm", "trainer"], &[]);
    assert!(code == Some(1) && err.starts_with("confirm_needed: "), "{err}");
    let out = l.ok(&["bot", "rm", "trainer", "--yes"]);
    assert!(!l.0.join("bots/trainer").exists());
    let trash: Vec<_> = std::fs::read_dir(l.0.join("bots/.trash")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert!(trash.len() == 1 && trash[0].starts_with("trainer-20") && trash[0].ends_with('Z'), "{trash:?}");
    assert!(out.contains(&format!("bots/.trash/{}", trash[0])), "{out}");
    assert!(l.0.join("bots/.trash").join(&trash[0]).join("MEMORY.md").is_file());
    assert_eq!(l.ok(&["bot", "ls"]), format!("no bots in {}\n", l.0.join("bots").display()));
    bot(&l, "trainer", trainer, None);
    assert_eq!(l.ok(&["bot", "session", "trainer"]), "none\n", "the ledger forgot it");
    assert!(l.ok(&["bot", "status", "trainer"]).starts_with("trainer           stopped "));

    // The bots' log: each step, a line; --bot keeps whole words only.
    bot(&l, "trainer-2", &trainer.replace("\"trainer\"", "\"trainer-2\""), None);
    l.ok(&["bot", "pause", "trainer-2"]);
    let log = l.ok(&["log", "--bot", "trainer"]);
    let events: Vec<&str> = log.lines().filter_map(|x| x.split(' ').nth(2).filter(|_| x.contains(" trainer "))).collect();
    assert_eq!(events, ["pause", "resume", "start", "ready", "pause", "reset", "start", "ready", "pause", "rm"], "{log}");
    assert!(log.starts_with("== yonderd.log (last 50 lines) ==\n(none)\n== bots.log (last 50 lines) ==\n"), "{log}");
    assert!(!log.contains("trainer-2"), "{log}");
    let all = l.ok(&["log", "--lines", "1"]);
    assert!(all.ends_with(" trainer-2 pause\n"), "{all}");
}

#[test]
fn bots_elsewhere_resume_their_session() {
    let l = Ledger::new("bots_elsewhere_resume_their_session");
    let trainer = "name = \"trainer\"\ndescription = \"x\"\nkind = \"persistent\"\nmachine = \"gpu-box\"\ncwd = \"~/bots/trainer\"\nharness = \"codex\"\nargs = [\"--no-daemon\"]\nmemory_id = \"trainer\"\n";
    bot(&l, "trainer", trainer, None);
    assert_eq!(l.ok(&["bot", "session", "trainer"]), "none\n");
    l.ok(&["bot", "session", "trainer", "s-1"]);
    assert_eq!(l.ok(&["bot", "session", "trainer"]), "s-1\n");
    let (code, _, err) = with_env(&l, &["bot", "session", "trainer", "-x"], &[]);
    assert!(code == Some(1) || code == Some(2), "{err}");
    assert_eq!(with_env(&l, &["bot", "start", "trainer"], &[]).1, "trainer: ready at gpu-box/t70\n");
    let sent = l.sent();
    assert!(sent.contains(">>> exec --on gpu-box -- /bin/sh -c "), "the startup check runs there: {sent}");
    assert_eq!(
        runs(&l),
        [format!(
            ">>> run --bg --json --name trainer --label yonder.bot=trainer --outlive-parent --keep --on gpu-box --cwd bots/trainer -- env NMEM_AGENT_ID=trainer /bin/sh -c {} yonder-bot codex resume s-1 --no-daemon",
            yonder_core::botrun::LOGIN_EXEC
        )]
    );
    // No template, no task: nothing is sent.
    assert!(!sent.contains(">>> send"));
    l.ok(&["bot", "session", "trainer", "--clear"]);
    assert_eq!(l.ok(&["bot", "session", "trainer"]), "none\n");

    // With env, only here.
    std::fs::write(l.0.join("bots/trainer/bot.toml"), format!("{trainer}env = [\"EXAMPLE_API_KEY\"]\n")).unwrap();
    let (code, _, err) = with_env(&l, &["bot", "start", "trainer"], &[]);
    assert_eq!(code, Some(1));
    assert!(err.starts_with("env_remote_unsupported: "), "{err}");
    assert_eq!(runs(&l).len(), 1);

    // What a start needs there, checked in one hc exec.
    std::fs::write(l.0.join("bots/trainer/bot.toml"), trainer).unwrap();
    let (code, out, _) = with_env(&l, &["bot", "check", "--live", "trainer"], &[("FAKE_HC_EXEC_CODE", "10")]);
    assert_eq!((code, out.as_str()), (Some(1), "error: trainer: cwd bots/trainer is not a directory on gpu-box\n"));
    let (code, out, _) = with_env(&l, &["bot", "check", "--live", "trainer"], &[("FAKE_HC_EXEC_CODE", "11")]);
    assert_eq!((code, out.as_str()), (Some(1), "error: trainer: harness codex is not found on gpu-box (in PATH there)\n"));
    assert_eq!(with_env(&l, &["bot", "check", "--live", "trainer"], &[]).0, Some(0));
}

#[test]
fn starts_that_cannot_get_ready_fail_and_say_why() {
    let l = Ledger::new("starts_that_cannot_get_ready_fail_and_say_why");
    let work = l.0.join("work");
    std::fs::create_dir(&work).unwrap();
    let stuck = format!("name = \"stuck\"\ndescription = \"x\"\nkind = \"persistent\"\ncwd = \"{}\"\nharness = \"/bin/sh\"\n", work.display());
    bot(&l, "stuck", &stuck, Some("never sent\n"));
    let kill = |t: &str| {
        Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-hc.sh"))
            .args(["kill", t])
            .env("FAKE_HC_ROOT", l.0.join("hc"))
            .env("FAKE_HC_LOG", l.0.join("hc.log"))
            .output()
            .unwrap();
    };
    for (env, said) in [
        (("FAKE_HC_SCREEN", "Do you trust this?\nHooks need review\n"), "needs_attention: Hooks need review; attach with hc attach t70\n"),
        (("FAKE_HC_SCREEN", "open in another app"), "needs_attention: open in another app; attach with hc attach t71\n"),
        (("FAKE_HC_STATUS", "blocked"), "needs_attention: blocked (blocked); attach with hc attach t72\n"),
    ] {
        let (code, _, err) = with_env(&l, &["bot", "start", "stuck"], &[env]);
        assert_eq!((code, err.as_str()), (Some(1), said));
        assert!(l.ok(&["bot", "status", "stuck"]).contains(" failed "));
        // The terminal is left for a person; this test kills it to go on.
        kill(&said[said.len() - 4..said.len() - 1]);
    }
    assert!(!l.sent().contains(">>> send"));
    std::fs::write(l.0.join("config.toml"), "[notify]\nlocal = false\n[bot]\nstart_timeout_secs = 1\n").unwrap();
    let start = Instant::now();
    let (code, _, err) = with_env(&l, &["bot", "start", "stuck"], &[("FAKE_HC_STATUS", "working")]);
    assert_eq!(code, Some(1));
    assert!(err.starts_with("start_timeout: stuck at t73 was not ready in 1s"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(8));
    assert!(l.ok(&["bot", "status", "stuck"]).contains("(start_timeout)"));
}

#[test]
fn a_start_cut_short_is_failed_later() {
    let l = Ledger::new("a_start_cut_short_is_failed_later");
    let work = l.0.join("work");
    std::fs::create_dir(&work).unwrap();
    bot(&l, "ops", &format!("name = \"ops\"\ndescription = \"x\"\nkind = \"persistent\"\ncwd = \"{}\"\nharness = \"/bin/sh\"\n", work.display()), None);
    // As if a start died while waiting, long ago, its terminal never seen.
    let store = yonder_core::Store::open(&l.0).unwrap();
    assert!(store.begin_run("ops", "t99", None, when::now() - 3600).unwrap());
    drop(store);
    // hc unreachable: shown as unknown, and the ledger stays as it was...
    let (code, out, _) = with_env(&l, &["bot", "status", "ops"], &[("YONDER_HC", "/nonexistent/hc")]);
    assert_eq!(code, Some(0));
    assert!(out.contains(" starting ") || out.contains(" failed "), "{out}");
    // ...except for what needs no hc: long past its time, it failed.
    assert!(out.contains("(start_timeout)"), "{out}");
    let store = yonder_core::Store::open(&l.0).unwrap();
    assert!(store.begin_run("ops", "t98", None, when::now()).unwrap());
    drop(store);
    let (_, out, _) = with_env(&l, &["bot", "status", "ops"], &[("YONDER_HC", "/nonexistent/hc")]);
    assert!(out.starts_with("ops               unknown    t98") && out.contains("[recorded starting; hc: hc get: cannot run"), "{out}");
    // With hc: no such terminal, so it never got ready.
    assert!(l.ok(&["bot", "status", "ops"]).contains("failed     t98             session -  (its terminal is gone)"));
}

#[test]
fn oneoff_bots_get_a_place_and_a_task() {
    let l = Ledger::new("oneoff_bots_get_a_place_and_a_task");
    let work = l.0.join("nova-work");
    std::fs::create_dir(&work).unwrap();
    bot(&l, "nova", "name = \"nova\"\ndescription = \"x\"\nkind = \"oneoff\"\nharness = \"/bin/sh\"\n", Some("you are nova\n"));
    let w = work.display().to_string();
    for args in [vec!["bot", "start", "nova", "--cwd", &w], vec!["bot", "start", "nova", "--task", "do x"]] {
        let (code, _, err) = with_env(&l, &args, &[]);
        assert_eq!(code, Some(1));
        assert!(err.starts_with("bad_input: nova is oneoff"), "{err}");
    }
    assert_eq!(with_env(&l, &["bot", "start", "nova", "--cwd", &w, "--task", "do x"], &[]).1, "nova: ready at t70\n");
    assert!(l.sent().contains(">>> send to t70\nyou are nova\n\ndo x\n"), "{}", l.sent());
    assert!(runs(&l)[0].contains(&format!("--cwd {w} -- env /bin/sh")));
}

/// The person's keychain, under the test service, with this test's own
/// bot name; deleted after.
#[cfg(target_os = "macos")]
#[test]
fn env_values_go_from_the_keychain_to_the_bot_only() {
    let l = Ledger::new("env_values_go_from_the_keychain_to_the_bot_only");
    let name = format!("envtest-{}", std::process::id() % 100000);
    let account = format!("{name}/EXAMPLE_API_KEY");
    let forget = || {
        let _ = Command::new("/usr/bin/security").args(["delete-generic-password", "-s", "dev.yonder.bots.test", "-a", &account]).output();
    };
    forget();
    let work = l.0.join("work");
    std::fs::create_dir(&work).unwrap();
    let got = l.0.join("got");
    let toml = format!(
        "name = \"{name}\"\ndescription = \"x\"\nkind = \"persistent\"\ncwd = \"{}\"\nharness = \"/bin/sh\"\nargs = [\"-c\", \"printf '%s|%s|%s' \\\"$EXAMPLE_API_KEY\\\" \\\"$NMEM_AGENT_ID\\\" \\\"$PWD\\\" > {}\"]\nmemory_id = \"m-1\"\nenv = [\"EXAMPLE_API_KEY\"]\n",
        work.display(),
        got.display()
    );
    bot(&l, &name, &toml, None);
    let service = [("YONDER_KEYCHAIN_SERVICE", "dev.yonder.bots.test")];
    let set = |var: &str, value: &str| {
        let mut child = l.cmd(&["bot", "env", "set", &name, var]).envs(service).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), value.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code(), stderr(&out))
    };
    // Missing: bot exec does not start the harness.
    let (code, _, err) = with_env(&l, &["bot", "exec", &name], &service);
    assert_eq!((code, err.lines().next()), (Some(1), Some("env_missing: EXAMPLE_API_KEY; keep it first: yonder bot env set <bot> <variable> < value-file")));
    assert!(!got.exists());
    let (code, err) = set("OTHER_KEY", "x");
    assert!(code == Some(1) && err.starts_with("env_not_declared: "), "{err}");
    let (code, err) = set("PATH", "x");
    assert!(code == Some(1) && err.starts_with("bad_env: "), "{err}");
    let (code, err) = set("EXAMPLE_API_KEY", "two\nlines\n");
    assert!(code == Some(1) && err.starts_with("bad_value: "), "{err}");
    assert_eq!(set("EXAMPLE_API_KEY", "synthetic \"v1\"\n").0, Some(0));

    let (code, out, err) = with_env(&l, &["bot", "start", &name], &[service[0], ("FAKE_HC_RUN_EXEC", "1")]);
    assert_eq!((code, out.as_str()), (Some(0), format!("{name}: ready at t70\n").as_str()), "{err}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_to_string(&got).unwrap_or_default().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    // The value, the memory id, and its cwd (bot exec goes there).
    let said = std::fs::read_to_string(&got).unwrap();
    assert!(said.starts_with("synthetic \"v1\"|m-1|/") && said.ends_with("/work"), "{said}");
    let log = l.sent();
    assert!(!log.contains("synthetic"), "{log}");
    let run = &runs(&l)[0];
    assert!(run.contains(&format!("-- env YONDER_HOME={} YONDER_KEYCHAIN_SERVICE=dev.yonder.bots.test ", l.0.display())) && run.ends_with(&format!(" bot exec {name}")), "{run}");
    forget();
}

#[test]
fn harnesses_are_found_and_run_by_the_login_shell() {
    let l = Ledger::new("harnesses_are_found_and_run_by_the_login_shell");
    let work = l.0.join("work");
    std::fs::create_dir(&work).unwrap();
    // A harness only the login shell finds: on the PATH its profile sets.
    let bin = l.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let got = l.0.join("got");
    std::fs::write(bin.join("harness-x"), format!("#!/bin/sh\nprintf '%s|' \"$@\" > {}\n", got.display())).unwrap();
    std::fs::set_permissions(bin.join("harness-x"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let profile = l.0.join("profile");
    std::fs::write(&profile, format!("PATH={}:$PATH; export PATH\n", bin.display())).unwrap();
    let toml = format!("name = \"ops\"\ndescription = \"x\"\nkind = \"persistent\"\ncwd = \"{}\"\nharness = \"harness-x\"\nmodel = \"m-1\"\nargs = [\"a b\", \"$HOME\"]\n", work.display());
    bot(&l, "ops", &toml, None);
    // sh as a login shell reads $ENV when interactive.
    let env = [("ENV", profile.to_str().unwrap()), ("FAKE_HC_RUN_EXEC", "1")];
    let (code, out, err) = with_env(&l, &["bot", "check", "--live", "ops"], &env[..1]);
    assert_eq!((code, out.as_str()), (Some(0), "ops: ok\n"), "{err}");
    assert!(l.sent().contains(">>> exec --on local -- /bin/sh -c "));
    // Not found where the login shell looks: an error.
    let (code, out, _) = with_env(&l, &["bot", "check", "--live", "ops"], &[]);
    assert_eq!((code, out.as_str()), (Some(1), "error: ops: harness harness-x is not found here (in PATH, or an executable absolute path)\n"));
    // Started for real: found, and its arguments arrive whole. Its model
    // is for codex, pi and claude only: a warning, not an argument.
    let (code, out, err) = with_env(&l, &["bot", "start", "ops"], &env);
    assert_eq!((code, out.as_str()), (Some(0), "ops: ready at t70\n"), "{err}");
    assert_eq!(err, "warning: ops: harness harness-x is not known to take a model; model m-1 is not passed\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !got.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(std::fs::read_to_string(&got).unwrap_or_default(), "a b|$HOME|");
}
