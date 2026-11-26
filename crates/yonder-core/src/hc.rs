//! The lead agent's terminal, through the `hc` command: the first live
//! terminal carrying its label (`hc ls --json --label key=value`), written
//! to with `hc send`. Every call is bounded in time and ends when the
//! daemon stops. A send that hc refused, or that never started, surely did
//! not arrive; one cut short may have.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
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
    fn send(&mut self, to: &str, text: &str) -> Result<(), SendError>;
}

/// Why a send did not surely go through.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// It did not arrive: try again.
    #[error("{0}")]
    NotSent(String),
    /// It may have arrived: never again.
    #[error("{0}")]
    Unknown(String),
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

    /// What hc printed on stdout and stderr, if it succeeded.
    fn call(&self, args: &[&str], timeout: Duration) -> Result<Vec<u8>, SendError> {
        let mut command = Command::new(&self.bin);
        command.args(args);
        let run = capture(command, timeout, MAX_REPLY, self.stop, true);
        outcome(args[0], &self.bin, run)
    }
}

/// How a run of hc went: refused (it says why, on stderr) or never started
/// is `NotSent`; cut short, killed, or failing without saying why is
/// `Unknown`.
fn outcome(verb: &str, bin: &Path, run: Result<(ExitStatus, Vec<u8>), CheckError>) -> Result<Vec<u8>, SendError> {
    let (status, out) = match run {
        Ok(done) => done,
        Err(CheckError::Spawn(e)) => return Err(SendError::NotSent(format!("hc {verb}: cannot run {}: {e}", bin.display()))),
        // Timeout, stopped, lost track of.
        Err(e) => return Err(SendError::Unknown(format!("hc {verb}: {e}"))),
    };
    if status.success() {
        return Ok(out);
    }
    match (status.code(), hc_error(&out)) {
        (Some(code), Some(why)) => Err(SendError::NotSent(format!("hc {verb}: exit:{code}: {why}"))),
        (Some(code), None) => Err(SendError::Unknown(format!("hc {verb}: exit:{code}, not saying why: {}", first_line(&out)))),
        (None, _) => Err(SendError::Unknown(format!("hc {verb}: killed by signal {}", status.signal().unwrap_or(0)))),
    }
}

impl Lead for Hc {
    fn find(&mut self) -> Result<Option<String>, String> {
        let out = self.call(&["ls", "--json", "--label", &self.label], LS_TIMEOUT).map_err(|e| e.to_string())?;
        first_live(&out).map_err(|e| format!("hc ls: {e}"))
    }

    fn send(&mut self, to: &str, text: &str) -> Result<(), SendError> {
        self.call(&["send", to, text], SEND_TIMEOUT).map(drop)
    }
}

/// The address of the first live terminal in `hc ls --json` output:
/// `{"terminals": [{"address": "t2", "state": "live", ...}]}`, on a line
/// of its own among whatever went to stderr.
fn first_live(out: &[u8]) -> Result<Option<String>, String> {
    let text = String::from_utf8_lossy(out);
    let json = text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|v| v.get("terminals").is_some());
    let list = json.ok_or_else(|| format!("no terminal list in {:?}", first_line(out)))?;
    let terminals = list["terminals"].as_array().ok_or("no terminals list")?;
    let live = terminals.iter().find(|t| t["state"].as_str().is_none_or(|s| s == "live"));
    Ok(live.and_then(|t| t["address"].as_str()).map(String::from))
}

/// What hc said went wrong: the `error.message` (or `error.kind`) of a
/// JSON line `{"error": {...}}`, if it said.
fn hc_error(out: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(out);
    text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find_map(|v| {
        let e = v.get("error")?;
        e["message"].as_str().or(e["kind"].as_str()).map(String::from)
    })
}

fn first_line(out: &[u8]) -> String {
    String::from_utf8_lossy(out).lines().next().unwrap_or("").chars().take(200).collect()
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
        let with_noise = b"warning: something\n{\"terminals\":[{\"address\":\"t4\",\"state\":\"live\"}]}\n";
        assert_eq!(first_live(with_noise), Ok(Some("t4".into())));
    }

    #[test]
    fn tells_refused_from_unknown() {
        let bin = Path::new("/tmp/example/hc");
        let exit = |code: i32| ExitStatus::from_raw(code << 8);
        let refused = br#"{"error":{"hint":"see `hc ls --all`","kind":"not_found","message":"no terminal \"t99\""}}"#.to_vec();
        let o = |run| outcome("send", bin, run);
        assert_eq!(o(Ok((exit(0), b"{}".to_vec()))), Ok(b"{}".to_vec()));
        assert_eq!(o(Ok((exit(3), refused))), Err(SendError::NotSent("hc send: exit:3: no terminal \"t99\"".into())));
        let kind_only = br#"{"error":{"kind":"blocked"}}"#.to_vec();
        assert!(matches!(o(Ok((exit(11), kind_only))), Err(SendError::NotSent(_))));
        let spawn = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(o(Err(CheckError::Spawn(spawn))), Err(SendError::NotSent(_))));
        // Maybe pasted: never again.
        assert!(matches!(o(Ok((exit(1), b"panicked".to_vec()))), Err(SendError::Unknown(_))));
        assert!(matches!(o(Ok((ExitStatus::from_raw(9), vec![]))), Err(SendError::Unknown(_))));
        for e in [CheckError::Timeout, CheckError::Stopped, CheckError::Wait(std::io::Error::other("x"))] {
            assert!(matches!(o(Err(e)), Err(SendError::Unknown(_))));
        }
    }

    #[test]
    fn runs_the_program_it_is_given() {
        let dir = crate::test_dir("runs_the_program_it_is_given");
        let bin = dir.join("hc");
        // Errors on stderr, as hc prints them.
        let script = "#!/bin/sh\ncase $1 in\n ls) echo '{\"terminals\":[{\"address\":\"t9\",\"state\":\"live\"}]}' ;;\n send) printf '%s|%s' \"$2\" \"$3\" > \"$(dirname \"$0\")/sent\" ;;\n *) echo '{\"error\":{\"message\":\"no\"}}' >&2; exit 3 ;;\nesac\n";
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut hc = Hc { bin: bin.clone(), label: "yonder.role=lead".into(), stop: &NEVER };
        assert_eq!(hc.find(), Ok(Some("t9".into())));
        hc.send("t9", "[yonder kind=remind]\nwater").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("sent")).unwrap(), "t9|[yonder kind=remind]\nwater");
        assert_eq!(hc.call(&["rm"], LS_TIMEOUT), Err(SendError::NotSent("hc rm: exit:3: no".into())));
        let missing = Hc { bin: dir.join("nope"), ..hc };
        assert!(matches!(missing.call(&["ls"], LS_TIMEOUT), Err(SendError::NotSent(e)) if e.contains("cannot run")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
