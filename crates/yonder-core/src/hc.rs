//! The lead agent's terminal, through the `hc` command: the first live
//! terminal carrying its label (`hc ls --json --label key=value`), written
//! to with `hc send`. Every call is bounded in time and ends when the
//! daemon stops. A send that never started, or that hc refused before
//! typing anything (`REFUSED_BEFORE_TYPING`), surely did not arrive; any
//! other failure may have.
//!
//! Files on other machines go through `hc file read --json --on M <path>`
//! (`{"first":1,"last":2,"lines":2,"text":"..."}` on stdout) and `hc file
//! write --on M <path>` (the text on stdin; it makes missing folders and
//! prints `{"bytes":N,"path":...}`). A missing file is `not_found`, exit 3.
//! hc 0.1.22, as tried: `read` gives at most 8 MB, cutting the rest off
//! with exit 0 and `last` short of `lines`, so that is taken as failing;
//! a file not UTF-8 is `not_text`, a directory `invalid_params`.
//!
//! Bots' terminals (`HcBots`), as hc 0.1.22 was seen to do them:
//! - `hc run --bg --json ... -- <argv>` prints `{"address":"t5c",...}`
//!   (`machine/t3p` elsewhere). With `--on`, a relative `--cwd` is from
//!   that machine's home, `~` is not expanded, and a missing directory
//!   fails the run itself (`spawn_failed`).
//! - `hc get --json <t>`: `state` is `live` or `finished` (with
//!   `exit_code`, `signal`, `reason`), `status` is null or
//!   `{"app","state","kind","msg",...}` with `state` one of idle, working,
//!   done, blocked (`kind` permission, question or auth) or error. Its
//!   `screen` is how the screen is kept (`exact`), not the text: that is
//!   `hc read --json <t>`'s `text`.
//! - `hc interrupt --json <t>` presses ctrl+c and prints
//!   `{"key":"ctrl+c","outcome":"unknown"}`; a finished terminal is
//!   `finished`, exit 11.
//! - `hc exec` without `--json` exits with the program's code and prints
//!   what it wrote; hc's own failures are its error line (`--json` would
//!   exit 0 with the code inside).

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::check_exec::{CheckError, capture, capture_input};
use crate::config::expand_home;

const LS_TIMEOUT: Duration = Duration::from_secs(15);
/// `hc send` waits while someone has a draft typed in the terminal.
const SEND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REPLY: usize = 1 << 20;
/// Each `hc file` call, and `hc run`.
const FILE_TIMEOUT: Duration = Duration::from_secs(30);
/// `hc get`, `hc read`, `hc ls`.
const LOOK_TIMEOUT: Duration = Duration::from_secs(15);
/// `hc interrupt` watches 10 seconds for an effect.
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(25);
/// `hc exec` of a bot's startup check elsewhere.
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);
/// 8 MB of text, escaped in JSON, with room.
const MAX_FILE_REPLY: usize = 24 << 20;

/// Error kinds of `terminal.send` (`hc schema`) raised before anything is
/// typed: no such terminal or machine (checked on an isolated core),
/// bad parameters, and `--if` not matching (its hint says "nothing was
/// typed"). Others, `finished` and `blocked` among them, are not known to
/// come before typing, so they count as maybe arrived.
pub const REFUSED_BEFORE_TYPING: &[&str] = &["not_found", "machine_unknown", "invalid_params", "status_mismatch"];

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

/// How a run of hc went: never started, or refused with an error kind of
/// `REFUSED_BEFORE_TYPING` (on stderr), is `NotSent`; cut short, killed,
/// any other kind, or failing without a kind is `Unknown`.
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
    let Some(code) = status.code() else {
        return Err(SendError::Unknown(format!("hc {verb}: killed by signal {}", status.signal().unwrap_or(0))));
    };
    match hc_error(&out) {
        Some(HcError { kind: Some(kind), message }) if REFUSED_BEFORE_TYPING.contains(&kind.as_str()) => {
            Err(SendError::NotSent(format!("hc {verb}: exit:{code}: {kind}: {message}")))
        }
        Some(HcError { kind, message }) => {
            let kind = kind.as_deref().unwrap_or("no kind");
            Err(SendError::Unknown(format!("hc {verb}: exit:{code}: {kind}: {message}")))
        }
        None => Err(SendError::Unknown(format!("hc {verb}: exit:{code}, not saying why: {}", first_line(&out)))),
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

/// Files in a directory on another machine, through `hc file`.
pub struct HcFiles {
    pub bin: PathBuf,
    pub machine: String,
    /// Absolute, or from that machine's home (hc's `--on` takes a relative
    /// path from there); empty for the home itself.
    pub dir: String,
    pub stop: &'static AtomicBool,
}

impl HcFiles {
    /// `name` in `dir`, never taken for an option.
    pub fn path(&self, name: &str) -> String {
        let path = match self.dir.trim_end_matches('/') {
            "" if self.dir.starts_with('/') => format!("/{name}"),
            "" => name.to_string(),
            dir => format!("{dir}/{name}"),
        };
        if path.starts_with('-') { format!("./{path}") } else { path }
    }

    fn file(&self, verb: &str, args: &[&str], input: Option<Vec<u8>>) -> Result<Vec<u8>, FileError> {
        let mut command = Command::new(&self.bin);
        command.arg("file").arg(verb).args(args);
        let run = capture_input(command, input, FILE_TIMEOUT, MAX_FILE_REPLY, self.stop, true);
        file_outcome(verb, &self.bin, run)
    }
}

impl crate::sync::Files for HcFiles {
    fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let path = self.path(name);
        match self.file("read", &["--json", "--on", &self.machine, &path], None) {
            Ok(out) => whole_text(&out).map(|t| Some(t.into_bytes())).map_err(|e| format!("hc file read {path}: {e}")),
            Err(FileError::NotFound) => Ok(None),
            Err(FileError::Failed(e)) => Err(e),
        }
    }

    fn write(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let path = self.path(name);
        match self.file("write", &["--on", &self.machine, &path], Some(data.to_vec())) {
            Ok(_) => Ok(()),
            Err(FileError::NotFound) => Err(format!("hc file write {path}: not_found")),
            Err(FileError::Failed(e)) => Err(e),
        }
    }
}

#[derive(Debug, PartialEq)]
enum FileError {
    NotFound,
    Failed(String),
}

/// How a run of `hc file` went: `not_found` is told apart, anything else
/// that is not success is a failure with what hc said.
fn file_outcome(verb: &str, bin: &Path, run: Result<(ExitStatus, Vec<u8>), CheckError>) -> Result<Vec<u8>, FileError> {
    let failed = |e: String| Err(FileError::Failed(format!("hc file {verb}: {e}")));
    let (status, out) = match run {
        Ok(done) => done,
        Err(CheckError::Spawn(e)) => return failed(format!("cannot run {}: {e}", bin.display())),
        Err(e) => return failed(e.to_string()),
    };
    if out.len() > MAX_FILE_REPLY {
        return failed(format!("its reply is over {MAX_FILE_REPLY} bytes"));
    }
    if status.success() {
        return Ok(out);
    }
    let Some(code) = status.code() else {
        return failed(format!("killed by signal {}", status.signal().unwrap_or(0)));
    };
    match hc_error(&out) {
        Some(HcError { kind: Some(kind), .. }) if kind == "not_found" => Err(FileError::NotFound),
        Some(HcError { kind, message }) => failed(format!("exit:{code}: {}: {message}", kind.as_deref().unwrap_or("no kind"))),
        None => failed(format!("exit:{code}, not saying why: {}", first_line(&out))),
    }
}

/// The `text` of `hc file read --json`, if it is the whole file.
fn whole_text(out: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(out);
    let reply = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v.get("text").is_some_and(|t| t.is_string()))
        .ok_or_else(|| format!("no text in {:?}", first_line(out)))?;
    let (last, lines) = (reply["last"].as_u64(), reply["lines"].as_u64());
    if let (Some(last), Some(lines)) = (last, lines)
        && last < lines
    {
        return Err(format!("only lines 1-{last} of {lines} came back: too large to read whole"));
    }
    Ok(reply["text"].as_str().unwrap_or_default().to_string())
}

/// What went wrong with an hc call: hc's own error, or anything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HcFail {
    Said { verb: String, code: i32, kind: String, message: String },
    Other(String),
}

impl HcFail {
    pub fn kind(&self) -> Option<&str> {
        match self {
            HcFail::Said { kind, .. } => Some(kind),
            HcFail::Other(_) => None,
        }
    }
}

impl std::fmt::Display for HcFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HcFail::Said { verb, code, kind, message } => write!(f, "hc {verb}: exit:{code}: {kind}: {message}"),
            HcFail::Other(e) => f.write_str(e),
        }
    }
}

/// How a run of hc went: its output if it succeeded, else what it said
/// (only its error's `kind` and `message`: its `hint` may list machines).
fn said(verb: &str, bin: &Path, run: Result<(ExitStatus, Vec<u8>), CheckError>, max: usize) -> Result<Vec<u8>, HcFail> {
    let other = |e: String| Err(HcFail::Other(format!("hc {verb}: {e}")));
    let (status, out) = match run {
        Ok(done) => done,
        Err(CheckError::Spawn(e)) => return other(format!("cannot run {}: {e}", bin.display())),
        Err(e) => return other(e.to_string()),
    };
    if out.len() > max {
        return other(format!("its reply is over {max} bytes"));
    }
    if status.success() {
        return Ok(out);
    }
    let Some(code) = status.code() else {
        return other(format!("killed by signal {}", status.signal().unwrap_or(0)));
    };
    match hc_error(&out) {
        Some(HcError { kind, message }) => {
            Err(HcFail::Said { verb: verb.into(), code, kind: kind.unwrap_or_else(|| "no kind".into()), message })
        }
        None => other(format!("exit:{code}, not saying why: {}", first_line(&out))),
    }
}

/// Bots' terminals, through hc.
pub struct HcBots {
    pub bin: PathBuf,
    pub stop: &'static AtomicBool,
}

/// What a bot's startup check found wrong on another machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThereFail {
    /// Not reached: hc's word for why.
    Machine(String),
    Cwd,
    Harness,
    Other(String),
}

/// The JSON object on a line of its own in `out` that has `key`.
fn json_with(out: &[u8], key: &str) -> Option<serde_json::Value> {
    String::from_utf8_lossy(out).lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|v| v.get(key).is_some())
}

impl HcBots {
    fn call(&self, args: &[&str], input: Option<Vec<u8>>, timeout: Duration) -> Result<Vec<u8>, HcFail> {
        let mut command = Command::new(&self.bin);
        command.args(args);
        let run = capture_input(command, input, timeout, MAX_REPLY, self.stop, true);
        said(args[0], &self.bin, run, MAX_REPLY)
    }

    /// Starts `argv` in a new terminal named `name`, carrying `label`, on
    /// `machine` (None: here), in `cwd`; kept when its parent ends, and
    /// its program replaced by a shell if lost. Its address.
    pub fn run(&self, name: &str, label: &str, machine: Option<&str>, cwd: &str, argv: &[String]) -> Result<String, HcFail> {
        let mut args = vec!["run", "--bg", "--json", "--name", name, "--label", label, "--outlive-parent", "--keep"];
        if let Some(machine) = machine {
            args.extend(["--on", machine]);
        }
        args.extend(["--cwd", cwd, "--"]);
        args.extend(argv.iter().map(String::as_str));
        let out = self.call(&args, None, FILE_TIMEOUT)?;
        let reply = json_with(&out, "address").ok_or_else(|| HcFail::Other(format!("hc run: no address in {:?}", first_line(&out))))?;
        Ok(reply["address"].as_str().unwrap_or_default().to_string())
    }

    /// `hc get --json`, or None if there is no such terminal.
    pub fn get(&self, address: &str) -> Result<Option<serde_json::Value>, HcFail> {
        match self.call(&["get", "--json", address], None, LOOK_TIMEOUT) {
            Ok(out) => json_with(&out, "state").map(Some).ok_or_else(|| HcFail::Other(format!("hc get: no state in {:?}", first_line(&out)))),
            Err(e) if e.kind() == Some("not_found") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The text on its screen, if hc gives it.
    pub fn screen(&self, address: &str) -> Option<String> {
        let out = self.call(&["read", "--json", address], None, LOOK_TIMEOUT).ok()?;
        json_with(&out, "text")?["text"].as_str().map(String::from)
    }

    /// The first live terminal carrying `label`, if any.
    pub fn live_labelled(&self, label: &str) -> Result<Option<String>, HcFail> {
        let out = self.call(&["ls", "--json", "--label", label], None, LOOK_TIMEOUT)?;
        first_live(&out).map_err(|e| HcFail::Other(format!("hc ls: {e}")))
    }

    /// Presses its stop key: what hc saw happen, or None if it has
    /// finished or is not there.
    pub fn interrupt(&self, address: &str) -> Result<Option<String>, HcFail> {
        match self.call(&["interrupt", "--json", address], None, INTERRUPT_TIMEOUT) {
            Ok(out) => Ok(Some(json_with(&out, "outcome").and_then(|v| v["outcome"].as_str().map(String::from)).unwrap_or_else(|| "done".into()))),
            Err(e) if matches!(e.kind(), Some("finished" | "not_found")) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn send(&self, address: &str, text: &str) -> Result<(), HcFail> {
        self.call(&["send", address, text], None, SEND_TIMEOUT).map(drop)
    }

    /// On `machine`, in one call: `cwd` (from its home if relative; None:
    /// not looked at) is there, and `harness` is a program it can run.
    pub fn check_there(&self, machine: &str, cwd: Option<&str>, harness: &str) -> Result<(), ThereFail> {
        const SCRIPT: &str = r#"cd -- "$1" || exit 10; command -v "$2" > /dev/null 2>&1 || exit 11"#;
        let args = ["exec", "--on", machine, "--", "sh", "-c", SCRIPT, "sh", cwd.unwrap_or("."), harness];
        let mut command = Command::new(&self.bin);
        command.args(args);
        let run = capture_input(command, None, EXEC_TIMEOUT, MAX_REPLY, self.stop, true);
        match run {
            Ok((status, _)) if status.success() => Ok(()),
            Ok((status, out)) => match (status.code(), hc_error(&out)) {
                (_, Some(HcError { kind, message })) => Err(ThereFail::Machine(format!("{}: {message}", kind.as_deref().unwrap_or("no kind")))),
                (Some(10), None) => Err(ThereFail::Cwd),
                (Some(11), None) => Err(ThereFail::Harness),
                (code, None) => Err(ThereFail::Other(format!("hc exec: exit:{}: {}", code.unwrap_or(-1), first_line(&out)))),
            },
            Err(e) => Err(ThereFail::Machine(format!("hc exec: {e}"))),
        }
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

/// What hc said went wrong, in a JSON line `{"error": {"kind": ..., "message": ...}}`.
#[derive(Debug, PartialEq)]
struct HcError {
    kind: Option<String>,
    message: String,
}

fn hc_error(out: &[u8]) -> Option<HcError> {
    let text = String::from_utf8_lossy(out);
    text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find_map(|v| {
        let e = v.get("error")?.as_object()?;
        let kind = e.get("kind").and_then(|k| k.as_str()).map(String::from);
        let message = e.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
        Some(HcError { kind, message })
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
        assert_eq!(o(Ok((exit(3), refused))), Err(SendError::NotSent("hc send: exit:3: not_found: no terminal \"t99\"".into())));
        let kind = |k: &str| format!(r#"{{"error":{{"kind":"{k}","message":"m"}}}}"#).into_bytes();
        for k in REFUSED_BEFORE_TYPING {
            assert!(matches!(o(Ok((exit(4), kind(k)))), Err(SendError::NotSent(_))), "{k}");
        }
        // Not known to come before typing, or no kind at all: maybe arrived.
        for k in ["timeout", "blocked", "finished", "cancelled", "unavailable", "config", "internal", "something_new"] {
            assert!(matches!(o(Ok((exit(11), kind(k)))), Err(SendError::Unknown(_))), "{k}");
        }
        let no_kind = br#"{"error":{"message":"no terminal"}}"#.to_vec();
        assert!(matches!(o(Ok((exit(3), no_kind))), Err(SendError::Unknown(_))));
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
    fn reads_files_whole_or_not_at_all() {
        let read = br#"{"first":1,"last":2,"lines":2,"text":"line one\nline \"two\"\n"}"#;
        assert_eq!(whole_text(read), Ok("line one\nline \"two\"\n".into()));
        assert_eq!(whole_text(br#"{"first":1,"last":0,"lines":0,"text":""}"#), Ok(String::new()));
        let cut = br#"{"first":1,"last":79137,"lines":90000,"text":"..."}"#;
        assert!(whole_text(cut).unwrap_err().contains("too large"));
        assert!(whole_text(b"{\"bytes\":3}").is_err());
        let exit = |code: i32| ExitStatus::from_raw(code << 8);
        let o = |run| file_outcome("read", Path::new("/tmp/example/hc"), run);
        let missing = br#"{"error":{"hint":"check the path","kind":"not_found","message":"no file /tmp/example/x"}}"#.to_vec();
        assert_eq!(o(Ok((exit(3), missing))), Err(FileError::NotFound));
        let unknown = br#"{"error":{"hint":"machines: a, b","kind":"machine_unknown","message":"unknown machine \"x\""}}"#.to_vec();
        // What hc hints (the machines it knows) is left out.
        assert_eq!(o(Ok((exit(4), unknown))), Err(FileError::Failed("hc file read: exit:4: machine_unknown: unknown machine \"x\"".into())));
        assert!(matches!(o(Err(CheckError::Timeout)), Err(FileError::Failed(e)) if e.ends_with("timeout")));
        assert!(matches!(o(Ok((exit(1), b"panicked".to_vec()))), Err(FileError::Failed(_))));
    }

    #[test]
    fn paths_on_another_machine() {
        let files = |dir: &str| HcFiles { bin: "hc".into(), machine: "gpu-box".into(), dir: dir.into(), stop: &NEVER };
        assert_eq!(files("bots/trainer").path("AGENTS.md"), "bots/trainer/AGENTS.md");
        assert_eq!(files("/srv/ops/").path("AGENTS.md"), "/srv/ops/AGENTS.md");
        assert_eq!(files("").path("MEMORY.md"), "MEMORY.md");
        assert_eq!(files("/").path("MEMORY.md"), "/MEMORY.md");
        assert_eq!(files("-x").path("MEMORY.md"), "./-x/MEMORY.md");
    }

    #[test]
    fn runs_the_program_it_is_given() {
        let dir = crate::test_dir("runs_the_program_it_is_given");
        let bin = dir.join("hc");
        // Errors on stderr, as hc prints them.
        let script = "#!/bin/sh\ncase $1 in\n ls) echo '{\"terminals\":[{\"address\":\"t9\",\"state\":\"live\"}]}' ;;\n send) printf '%s|%s' \"$2\" \"$3\" > \"$(dirname \"$0\")/sent\" ;;\n *) echo '{\"error\":{\"kind\":\"not_found\",\"message\":\"no\"}}' >&2; exit 3 ;;\nesac\n";
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut hc = Hc { bin: bin.clone(), label: "yonder.role=lead".into(), stop: &NEVER };
        assert_eq!(hc.find(), Ok(Some("t9".into())));
        hc.send("t9", "[yonder kind=remind]\nwater").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("sent")).unwrap(), "t9|[yonder kind=remind]\nwater");
        assert_eq!(hc.call(&["rm"], LS_TIMEOUT), Err(SendError::NotSent("hc rm: exit:3: not_found: no".into())));
        let missing = Hc { bin: dir.join("nope"), ..hc };
        assert!(matches!(missing.call(&["ls"], LS_TIMEOUT), Err(SendError::NotSent(e)) if e.contains("cannot run")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
