//! The `yonder` binary against a ledger in a temporary state directory.

use std::path::PathBuf;
use std::process::{Command, Output};

use yonder_core::when;

struct Ledger(PathBuf);

impl Ledger {
    fn new(name: &str) -> Ledger {
        let dir = std::env::temp_dir().join(format!("yonder-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Ledger(dir)
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_yonder"));
        cmd.args(args).env("YONDER_STATE_DIR", &self.0);
        cmd
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
    let next = when::Rule::parse("daily@09:00").unwrap().next_after(before);
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
    ] {
        let out = l.run(args);
        assert!(!out.status.success(), "{args:?}");
        let err = stderr(&out);
        assert!(err.starts_with(code) && err.lines().count() == 1, "{args:?}: {err}");
    }
}
