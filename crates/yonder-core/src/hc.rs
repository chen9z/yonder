//! The lead agent's terminal, through the `hc` command: the first live
//! terminal carrying its label (`hc ls --json --label key=value`), written
//! to with `hc send`. Every call is bounded in time and ends when the
//! daemon stops.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::check_exec::{CheckError, capture};
use crate::config::expand_home;

const LS_TIMEOUT: Duration = Duration::from_secs(15);
/// `hc send` waits while someone has a draft typed in the terminal.
const SEND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REPLY: usize = 1 << 20;

/// Where yonderd sends envelopes to.
pub trait Lead {
    /// The lead agent's terminal address, if one is live.
    fn find(&mut self) -> Result<Option<String>, String>;
    /// Pastes `text` into terminal `to` and submits it.
    fn send(&mut self, to: &str, text: &str) -> Result<(), String>;
}

pub struct Hc {
    pub bin: PathBuf,
    /// `key=value`.
    pub label: String,
    pub stop: &'static AtomicBool,
}

impl Hc {
    /// `$YONDER_HC`, else the config's `hc.bin`, else `hc` from PATH.
    pub fn bin(configured: &str, home: &Path) -> PathBuf {
        match std::env::var_os("YONDER_HC").filter(|b| !b.is_empty()) {
            Some(bin) => bin.into(),
            None if !configured.is_empty() => expand_home(configured, home),
            None => "hc".into(),
        }
    }

    fn call(&self, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
        let mut command = Command::new(&self.bin);
        command.args(args).stderr(Stdio::null());
        let verb = args[0];
        let (status, out) = capture(command, timeout, MAX_REPLY, self.stop).map_err(|e| match e {
            CheckError::Spawn(e) => format!("hc {verb}: cannot run {}: {e}", self.bin.display()),
            e => format!("hc {verb}: {e}"),
        })?;
        if status.success() {
            return Ok(out);
        }
        let why = match status.code() {
            Some(code) => format!("exit:{code}"),
            None => "killed".into(),
        };
        Err(format!("hc {verb}: {why}: {}", error_message(&out)))
    }
}

impl Lead for Hc {
    fn find(&mut self) -> Result<Option<String>, String> {
        let out = self.call(&["ls", "--json", "--label", &self.label], LS_TIMEOUT)?;
        first_live(&out).map_err(|e| format!("hc ls: {e}"))
    }

    fn send(&mut self, to: &str, text: &str) -> Result<(), String> {
        self.call(&["send", to, text], SEND_TIMEOUT).map(drop)
    }
}

/// The address of the first live terminal in `hc ls --json` output:
/// `{"terminals": [{"address": "t2", "state": "live", ...}]}`.
fn first_live(out: &[u8]) -> Result<Option<String>, String> {
    let list: serde_json::Value = serde_json::from_slice(out).map_err(|e| format!("not JSON: {e}"))?;
    let terminals = list["terminals"].as_array().ok_or("no terminals list")?;
    let live = terminals.iter().find(|t| t["state"].as_str().is_none_or(|s| s == "live"));
    Ok(live.and_then(|t| t["address"].as_str()).map(String::from))
}

/// What hc said went wrong: its JSON `error.message`, else its first line.
fn error_message(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    let json = serde_json::from_str::<serde_json::Value>(&text).ok();
    match json.as_ref().and_then(|v| v["error"]["message"].as_str()) {
        Some(message) => message.to_string(),
        None => text.lines().next().unwrap_or("").chars().take(200).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEVER: AtomicBool = AtomicBool::new(false);

    #[test]
    fn finds_the_first_live_terminal() {
        let ls = br#"{"hidden_finished":0,"terminals":[{"address":"t1","state":"finished"},{"address":"t2","state":"live"},{"address":"t3","state":"live"}]}"#;
        assert_eq!(first_live(ls), Ok(Some("t2".into())));
        assert_eq!(first_live(br#"{"terminals":[]}"#), Ok(None));
        assert!(first_live(b"[]").is_err());
        assert!(first_live(b"oops").is_err());
    }

    #[test]
    fn says_what_hc_said() {
        let err = br#"{"error":{"hint":"see `hc ls --all`","kind":"not_found","message":"no terminal \"t99\""}}"#;
        assert_eq!(error_message(err), "no terminal \"t99\"");
        assert_eq!(error_message(b"boom\nmore"), "boom");
    }

    #[test]
    fn runs_the_program_it_is_given() {
        let dir = crate::test_dir("runs_the_program_it_is_given");
        let bin = dir.join("hc");
        let script = "#!/bin/sh\ncase $1 in\n ls) echo '{\"terminals\":[{\"address\":\"t9\",\"state\":\"live\"}]}' ;;\n send) printf '%s|%s' \"$2\" \"$3\" > \"$(dirname \"$0\")/sent\" ;;\n *) echo '{\"error\":{\"message\":\"no\"}}'; exit 3 ;;\nesac\n";
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut hc = Hc { bin: bin.clone(), label: "yonder.role=lead".into(), stop: &NEVER };
        assert_eq!(hc.find(), Ok(Some("t9".into())));
        hc.send("t9", "[yonder kind=remind]\nwater").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("sent")).unwrap(), "t9|[yonder kind=remind]\nwater");
        assert_eq!(hc.call(&["rm"], LS_TIMEOUT), Err("hc rm: exit:3: no".into()));
        let missing = Hc { bin: dir.join("nope"), ..hc };
        assert!(missing.call(&["ls"], LS_TIMEOUT).unwrap_err().contains("cannot run"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
