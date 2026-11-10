//! A scratch herdr-core for tests: its own `HC_SESSION`, HOME and XDG
//! folders under /var/tmp, stopped and removed when dropped. It never
//! touches the user's core (herdr-core `docs/security.md`, Sessions).

pub mod computer;

use herdc_wire::Client;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The herdr-core binary: `HERDC_HC`, else the installed one.
pub fn core_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("HERDC_HC") {
        return p.into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    for candidate in [home.join(".local/bin/herdr-core"), home.join(".local/bin/hc")] {
        if candidate.exists() {
            return candidate;
        }
    }
    "herdr-core".into()
}

pub struct Session {
    pub root: PathBuf,
    pub bin: PathBuf,
}

impl Session {
    /// A fresh session (no Default: every one is a core on disk).
    #[allow(clippy::new_without_default)]
    pub fn new() -> Session {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = PathBuf::from(format!("/var/tmp/herdc-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["run", "state", "config", "home"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        Session { root, bin: core_binary() }
    }

    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// The environment that selects this session (also for an SSH pipe's
    /// shell line: see [`Session::env_line`]).
    pub fn env(&self) -> Vec<(String, String)> {
        let p = |d: &str| self.root.join(d).display().to_string();
        vec![
            ("HOME".into(), p("home")),
            ("HC_SESSION".into(), "test".into()),
            ("XDG_RUNTIME_DIR".into(), p("run")),
            ("XDG_STATE_HOME".into(), p("state")),
            ("XDG_CONFIG_HOME".into(), p("config")),
            ("HC_SETTLE_MS".into(), "0".into()),
            ("SHELL".into(), "/bin/sh".into()),
        ]
    }

    /// `VAR=value ...` for a shell line that should reach this session.
    pub fn env_line(&self) -> String {
        self.env().iter().map(|(k, v)| format!("{k}='{v}'")).collect::<Vec<_>>().join(" ")
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear().env("PATH", std::env::var("PATH").unwrap_or_default()).envs(self.env());
        cmd
    }

    pub fn hc(&self, args: &[&str]) -> Output {
        self.command(&self.bin).args(args).stdin(Stdio::null()).output().expect("run herdr-core")
    }

    /// `hc --json ...`, which must succeed.
    pub fn json(&self, args: &[&str]) -> Value {
        let out = self.hc(&[&["--json"], args].concat());
        assert!(out.status.success(), "hc {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("hc {args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)))
    }

    /// A framed client over a local `herdr-core connect` (starting the core).
    pub async fn connect(&self) -> Client {
        let mut child = tokio::process::Command::from(self.command(&self.bin));
        child.arg("connect").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
        let mut child = child.spawn().expect("spawn herdr-core connect");
        let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        // The pipe lives as long as the runtime: the child is reaped when it ends.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Client::handshake(stdout, stdin).await.expect("hello")
    }

    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.home().join(rel)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Every session started here (a phone may ask for another), not only `test`.
        let mut names = std::collections::BTreeSet::from(["test".to_owned()]);
        for kept in std::fs::read_dir(self.root.join("run/herdr-core")).into_iter().flatten().flatten() {
            if let Some(name) = kept.file_name().to_str().and_then(|n| n.split('.').next()).filter(|n| !n.is_empty()) {
                names.insert(name.to_owned());
            }
        }
        for name in names {
            let _ = self.command(&self.bin).env("HC_SESSION", name).args(["server", "stop"]).stdin(Stdio::null()).output();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Polls until `ok`, failing the test after `timeout`.
pub async fn eventually(what: &str, timeout: Duration, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
