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
