//! Running a reminder's check: `/bin/sh -c` in a process group of its own,
//! with a short list of environment variables (no keys or tokens leak in),
//! a time limit after which the whole group is killed, and a cap on the
//! output. Only the trimmed stdout counts; stderr is dropped.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

pub const TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_OUTPUT: usize = 1 << 20;

/// Variables a check sees, if set; `LC_*` too.
const KEEP_ENV: &[&str] = &["PATH", "HOME", "USER", "SHELL", "LANG", "TZ", "TMPDIR"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOutput {
    /// Stdout without leading and trailing white space.
    pub stdout: String,
    /// SHA-256 hex of `stdout`.
    pub hash: String,
}

/// Why a check failed; shown as `exit:N`, `timeout`, `output_too_large`, `spawn_failed`.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("exit:{0}")]
    Exit(i32),
    #[error("signal:{0}")]
    Signal(i32),
    #[error("timeout")]
    Timeout,
    #[error("output_too_large")]
    OutputTooLarge,
    #[error("spawn_failed: {0}")]
    Spawn(std::io::Error),
}

pub fn run(cmd: &str) -> Result<CheckOutput, CheckError> {
    run_with(cmd, TIMEOUT, MAX_OUTPUT)
}

pub fn run_with(cmd: &str, timeout: Duration, max_output: usize) -> Result<CheckOutput, CheckError> {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(cmd)
        .env_clear()
        .envs(std::env::vars_os().filter(|(k, _)| {
            k.to_str().is_some_and(|k| KEEP_ENV.contains(&k) || k.starts_with("LC_"))
        }))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().map_err(CheckError::Spawn)?;
    let group = child.id() as libc::pid_t;
    let kill_group = || {
        // SAFETY: signals the group the child leads, which nothing else uses.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    };
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let read = (&mut stdout).take(max_output as u64 + 1).read_to_end(&mut buf);
        // Drain the rest so a chatty check does not block on a full pipe.
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        read.map(|_| buf)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                kill_group();
                let _ = child.wait();
                return Err(CheckError::Spawn(e));
            }
        }
    };
    // Whatever the shell left running in its group goes too, so its
    // output pipe closes.
    kill_group();
    let Some(status) = status else {
        let _ = child.wait();
        let _ = reader.join();
        return Err(CheckError::Timeout);
    };
    let buf = reader.join().ok().and_then(Result::ok).unwrap_or_default();
    if let Some(code) = status.code().filter(|c| *c != 0) {
        return Err(CheckError::Exit(code));
    }
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Err(CheckError::Signal(sig));
    }
    if buf.len() > max_output {
        return Err(CheckError::OutputTooLarge);
    }
    let stdout = String::from_utf8_lossy(&buf).trim().to_string();
    let hash = hex(&Sha256::digest(stdout.as_bytes()));
    Ok(CheckOutput { stdout, hash })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_trimmed_stdout() {
        let out = run("printf '  ok\\n\\n'; echo noise >&2").unwrap();
        assert_eq!(out.stdout, "ok");
        // sha256("ok")
        assert_eq!(out.hash, "2689367b205c16ce32ed4200942b8b8b1e262dfc70d9bc9fbc77c49699a4f1df");
        assert_eq!(run("echo ok").unwrap().hash, out.hash);
    }

    #[test]
    fn reports_failures() {
        assert!(matches!(run("exit 3"), Err(CheckError::Exit(3))));
        assert_eq!(run("exit 3").unwrap_err().to_string(), "exit:3");
        assert!(matches!(run("kill -9 $$"), Err(CheckError::Signal(9))));
        let big = run_with("head -c 2000 /dev/zero", TIMEOUT, 1000);
        assert!(matches!(big, Err(CheckError::OutputTooLarge)));
    }

    #[test]
    fn keeps_only_safe_env() {
        // cargo runs tests with CARGO_* and more set; the check sees none of it.
        assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
        let out = run("env").unwrap();
        assert!(!out.stdout.contains("CARGO_"), "{}", out.stdout);
        for line in out.stdout.lines() {
            let key = line.split('=').next().unwrap();
            // sh sets PWD, SHLVL and _ itself.
            let own = ["PWD", "SHLVL", "_", "OLDPWD"].contains(&key);
            assert!(KEEP_ENV.contains(&key) || key.starts_with("LC_") || own, "{line}");
        }
        assert_eq!(run("echo $PATH").unwrap().stdout, std::env::var("PATH").unwrap());
    }

    #[test]
    fn kills_the_whole_group_on_timeout() {
        let start = Instant::now();
        let r = run_with("sleep 30 & sleep 30; echo late", Duration::from_millis(500), MAX_OUTPUT);
        assert!(matches!(r, Err(CheckError::Timeout)));
        // The backgrounded sleep held stdout open; killing the group closed it.
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }
}
