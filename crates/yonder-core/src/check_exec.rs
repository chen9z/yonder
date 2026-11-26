//! Running a reminder's check: `/bin/sh -c` in a process group of its own,
//! with a short list of environment variables (no keys or tokens leak in),
//! a time limit after which the whole group is killed, and a cap on the
//! output. Only the trimmed stdout counts; stderr is dropped.

use std::io::Read;
use std::os::fd::AsRawFd;
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
    let deadline = Instant::now() + timeout;
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
    // The shell leads its group, and its pid (so the group's id) stays
    // ours until it is reaped: exits are only watched, never reaped, until
    // the group has been killed.
    let group = child.id() as libc::pid_t;
    let kill_group = || {
        // SAFETY: signals the group the unreaped child leads.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    };
    let mut stdout = child.stdout.take().expect("piped");
    let fd = stdout.as_raw_fd();
    // SAFETY: changes the flags of the pipe end we own.
    unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };

    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let (mut exited, mut eof) = (false, false);
    // Until the shell has exited and its output closed, or time is up.
    // Something that left the group (setsid) may hold the output open
    // forever: the deadline stops reading too.
    let timed_out = loop {
        if !exited {
            match has_exited(group) {
                Ok(true) => {
                    exited = true;
                    // What the shell left running in its group goes too.
                    kill_group();
                }
                Ok(false) => {}
                Err(e) => {
                    kill_group();
                    let _ = child.wait();
                    return Err(CheckError::Spawn(e));
                }
            }
        }
        if exited && eof {
            break false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break true;
        }
        let wait = left.min(Duration::from_millis(20));
        if eof {
            std::thread::sleep(wait);
            continue;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd we own, for at most 20 ms.
        unsafe { libc::poll(&mut pfd, 1, wait.as_millis().max(1) as libc::c_int) };
        // What is there now, but not past the deadline for a check that
        // writes without end.
        while Instant::now() < deadline {
            match stdout.read(&mut chunk) {
                Ok(0) => eof = true,
                Ok(n) => {
                    // Past the cap only note that there was more.
                    let room = (max_output + 1).saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => eof = true,
            }
            break;
        }
    };
    // Stop reading: whatever still holds the pipe gets EPIPE.
    drop(stdout);
    if timed_out {
        kill_group();
        let _ = child.wait();
        return Err(CheckError::Timeout);
    }
    // Exited already, so this only reaps it.
    let status = child.wait().map_err(CheckError::Spawn)?;
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

/// Whether `pid` has exited, without reaping it.
fn has_exited(pid: libc::pid_t) -> std::io::Result<bool> {
    loop {
        // SAFETY: siginfo_t is plain data that waitid writes into.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let flags = libc::WEXITED | libc::WNOWAIT | libc::WNOHANG;
        // SAFETY: waits on our own child, leaving it waitable.
        let r = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) };
        if r == 0 {
            // SAFETY: waitid filled it in; si_pid is 0 if nothing exited.
            return Ok(unsafe { info.si_pid() } == pid);
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
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
        // Far past the cap, the rest is read and dropped.
        let huge = run_with("head -c 50000000 /dev/zero", TIMEOUT, 1000);
        assert!(matches!(huge, Err(CheckError::OutputTooLarge)));
        // Without end: the deadline stops it.
        let start = Instant::now();
        let endless = run_with("yes", Duration::from_millis(500), 1000);
        assert!(matches!(endless, Err(CheckError::Timeout)), "{endless:?}");
        assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
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
    fn kills_what_the_shell_leaves_behind() {
        // The shell exits at once; the sleep in its group holds stdout.
        let start = Instant::now();
        let out = run("sleep 30 & echo ok").unwrap();
        assert_eq!(out.stdout, "ok");
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    /// A check that puts a process in a session of its own, out of reach
    /// of the group kill, holding stdout open; its pid lands in `pid_file`.
    fn escaped(pid_file: &std::path::Path, then: &str) -> String {
        let perl = r#"use POSIX; setsid(); open my $f, ">", $ARGV[0]; print $f $$; close $f; sleep 30"#;
        format!("perl -e '{perl}' '{}' & {then}", pid_file.display())
    }

    fn kill_escaped(pid_file: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            if let Some(pid) = std::fs::read_to_string(pid_file).ok().and_then(|p| p.trim().parse().ok()) {
                break pid;
            }
            assert!(Instant::now() < deadline, "the escaped process never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        // SAFETY: a test's own stray process.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        std::fs::remove_dir_all(pid_file.parent().unwrap()).unwrap();
    }

    #[test]
    fn deadline_holds_against_an_escaped_reader() {
        let short = Duration::from_millis(500);
        for (name, then) in [("running", "sleep 30"), ("exited", "echo ok")] {
            let dir = crate::test_dir(&format!("escaped_{name}"));
            let pid_file = dir.join("pid");
            let start = Instant::now();
            let r = run_with(&escaped(&pid_file, then), short, MAX_OUTPUT);
            let took = start.elapsed();
            kill_escaped(&pid_file);
            assert!(matches!(r, Err(CheckError::Timeout)), "{name}: {r:?}");
            assert!(took < short + Duration::from_secs(2), "{name}: {took:?}");
        }
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
