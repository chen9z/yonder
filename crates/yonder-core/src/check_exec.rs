//! Running a reminder's check: `/bin/sh -c` in a process group of its own,
//! with a short list of environment variables (no keys or tokens leak in),
//! a time limit after which the whole group is killed, and a cap on the
//! output. Only the trimmed stdout counts; stderr is dropped.

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Why a check failed; shown as `exit:N`, `signal:N`, `timeout`, `output_too_large`, `spawn_failed`, `wait_failed`.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("exit:{0}")]
    Exit(i32),
    #[error("signal:{0}")]
    Signal(i32),
    #[error("timeout")]
    Timeout,
    /// Given up because the daemon is stopping: says nothing of the check.
    #[error("stopped")]
    Stopped,
    #[error("output_too_large")]
    OutputTooLarge,
    #[error("spawn_failed: {0}")]
    Spawn(std::io::Error),
    /// Started, then lost track of: what it did is not known.
    #[error("wait_failed: {0}")]
    Wait(std::io::Error),
}

pub fn run(cmd: &str) -> Result<CheckOutput, CheckError> {
    run_with(cmd, TIMEOUT, MAX_OUTPUT)
}

pub fn run_with(cmd: &str, timeout: Duration, max_output: usize) -> Result<CheckOutput, CheckError> {
    run_until(cmd, timeout, max_output, &AtomicBool::new(false))
}

/// As `run_with`, but given up (`Stopped`) as soon as `stop` is set.
pub fn run_until(cmd: &str, timeout: Duration, max_output: usize, stop: &AtomicBool) -> Result<CheckOutput, CheckError> {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(cmd)
        .env_clear()
        .envs(std::env::vars_os().filter(|(k, _)| {
            k.to_str().is_some_and(|k| KEEP_ENV.contains(&k) || k.starts_with("LC_"))
        }))
        .stderr(Stdio::null());
    let (status, buf) = capture(command, timeout, max_output, stop, false)?;
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

/// Runs `command` in a process group of its own and reads its stdout (and
/// stderr, in the same stream, with `stderr_too`), at most `max_output`
/// bytes and one more to tell it went past. Ends when it has exited and
/// its output closed, or at the deadline or `stop`, which kill the group.
/// The environment, and stderr without `stderr_too`, are the caller's.
pub fn capture(
    command: Command,
    timeout: Duration,
    max_output: usize,
    stop: &AtomicBool,
    stderr_too: bool,
) -> Result<(ExitStatus, Vec<u8>), CheckError> {
    capture_input(command, None, timeout, max_output, stop, stderr_too)
}

/// As `capture`, with `input` (if any) on its stdin, else nothing there.
/// The input is written from a thread of its own, so a command that
/// never reads it still ends at the deadline.
pub fn capture_input(
    mut command: Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    max_output: usize,
    stop: &AtomicBool,
    stderr_too: bool,
) -> Result<(ExitStatus, Vec<u8>), CheckError> {
    let deadline = Instant::now() + timeout;
    let (mut stdout, writer) = std::io::pipe().map_err(CheckError::Spawn)?;
    if stderr_too {
        command.stderr(writer.try_clone().map_err(CheckError::Spawn)?);
    }
    let stdin = if input.is_some() { Stdio::piped() } else { Stdio::null() };
    command.stdin(stdin).stdout(writer).process_group(0);
    let spawned = command.spawn();
    // The command keeps its copies of the write end: without them gone,
    // the output would never close.
    drop(command);
    let mut child = spawned.map_err(CheckError::Spawn)?;
    if let (Some(input), Some(mut pipe)) = (input, child.stdin.take()) {
        // Ends when all is written, or with an error once the group is
        // gone and nothing reads; then the pipe closes.
        std::thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut pipe, &input);
        });
    }
    // The child leads its group, and its pid (so the group's id) stays
    // ours until it is reaped: exits are only watched, never reaped, until
    // the group has been killed.
    let group = child.id() as libc::pid_t;
    let kill_group = || {
        // SAFETY: signals the group the unreaped child leads.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    };
    let fd = stdout.as_raw_fd();
    // SAFETY: reads and sets the flags of the pipe end we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        let e = std::io::Error::last_os_error();
        kill_group();
        let _ = child.wait();
        return Err(CheckError::Wait(e));
    }

    let mut buf = Vec::new();
    let (mut exited, mut eof) = (false, false);
    // Until the child has exited and its output closed, or time is up.
    // Something that left the group (setsid) may hold the output open
    // forever: the deadline stops reading too.
    let ended = loop {
        if !exited {
            match has_exited(group) {
                Ok(true) => {
                    exited = true;
                    // What it left running in its group goes too.
                    kill_group();
                }
                Ok(false) => {}
                Err(e) => {
                    kill_group();
                    let _ = child.wait();
                    return Err(CheckError::Wait(e));
                }
            }
        }
        if exited && eof {
            break None;
        }
        if stop.load(Ordering::Relaxed) {
            break Some(CheckError::Stopped);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break Some(CheckError::Timeout);
        }
        let wait = left.min(Duration::from_millis(20));
        if eof {
            std::thread::sleep(wait);
            continue;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd we own, for at most 20 ms.
        unsafe { libc::poll(&mut pfd, 1, wait.as_millis().max(1) as libc::c_int) };
        eof = read_available(&mut stdout, &mut buf, max_output, deadline, stop);
    };
    // Stop reading: whatever still holds the pipe gets EPIPE.
    drop(stdout);
    if let Some(e) = ended {
        kill_group();
        let _ = child.wait();
        return Err(e);
    }
    // Exited already, so this only reaps it.
    let status = child.wait().map_err(CheckError::Wait)?;
    Ok((status, buf))
}

/// Reads what `r` has now into `buf`, keeping `max_output` bytes and one
/// more to tell it went past, but stops at the deadline or `stop` for a
/// command that writes without end. Whether the output ended.
fn read_available(r: &mut impl Read, buf: &mut Vec<u8>, max_output: usize, deadline: Instant, stop: &AtomicBool) -> bool {
    let mut chunk = [0u8; 8192];
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match r.read(&mut chunk) {
            Ok(0) => return true,
            Ok(n) => {
                let room = (max_output + 1).saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(room)]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return false,
            Err(_) => return true,
        }
    }
    false
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

pub(crate) fn hex(bytes: &[u8]) -> String {
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
    fn gives_input_on_stdin() {
        let sh = |cmd: &str| {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(cmd);
            c
        };
        let never = AtomicBool::new(false);
        let (status, out) = capture_input(sh("cat"), Some(b"a\nb".to_vec()), TIMEOUT, MAX_OUTPUT, &never, false).unwrap();
        assert!(status.success());
        assert_eq!(out, b"a\nb");
        // Without input, stdin is empty.
        assert_eq!(capture(sh("cat"), TIMEOUT, MAX_OUTPUT, &never, false).unwrap().1, b"");
        // Never read, more than a pipe holds: still ends at the deadline.
        let start = Instant::now();
        let r = capture_input(sh("sleep 10"), Some(vec![b'x'; 1 << 20]), Duration::from_millis(300), MAX_OUTPUT, &never, false);
        assert!(matches!(r, Err(CheckError::Timeout)));
        assert!(start.elapsed() < Duration::from_secs(5));
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

    /// A process in a session of its own, out of reach of the group kill,
    /// holding the check's stdout open. Killed when this is dropped, even
    /// if the test fails.
    struct Escaped {
        dir: std::path::PathBuf,
    }

    impl Escaped {
        fn new(name: &str) -> Escaped {
            Escaped { dir: crate::test_dir(&format!("escaped_{name}")) }
        }

        /// A check that starts it, waits until it is in its own session
        /// (it writes its pid after setsid), then runs `then`.
        fn check(&self, then: &str) -> String {
            let pid = self.dir.join("pid");
            let perl = r#"use POSIX; setsid(); open my $f, ">", "$ARGV[0].tmp"; print $f $$; close $f; rename "$ARGV[0].tmp", $ARGV[0]; sleep 30"#;
            let wait = format!("n=0; while [ ! -s '{p}' ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done", p = pid.display());
            format!("perl -e '{perl}' '{}' & {wait}; {then}", pid.display())
        }
    }

    impl Drop for Escaped {
        fn drop(&mut self) {
            let pid = std::fs::read_to_string(self.dir.join("pid")).ok().and_then(|p| p.trim().parse().ok());
            if let Some(pid) = pid {
                // SAFETY: the test's own stray process.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            let _ = std::fs::remove_dir_all(&self.dir);
            if pid.is_none() && !std::thread::panicking() {
                panic!("the escaped process never started");
            }
        }
    }

    #[test]
    fn deadline_holds_against_an_escaped_reader() {
        // The shell still running, and the shell gone with only the
        // escaped process left holding stdout.
        let short = Duration::from_secs(1);
        for (name, then) in [("running", "sleep 30"), ("exited", "echo ok")] {
            let escaped = Escaped::new(name);
            let start = Instant::now();
            let r = run_with(&escaped.check(then), short, MAX_OUTPUT);
            let took = start.elapsed();
            assert!(matches!(r, Err(CheckError::Timeout)), "{name}: {r:?}");
            assert!(took < short + Duration::from_secs(2), "{name}: {took:?}");
        }
    }

    #[test]
    fn stops_when_told() {
        let stop = AtomicBool::new(false);
        let start = Instant::now();
        let r = std::thread::scope(|s| {
            let run = s.spawn(|| run_until("sleep 30", TIMEOUT, MAX_OUTPUT, &stop));
            std::thread::sleep(Duration::from_millis(200));
            stop.store(true, Ordering::Relaxed);
            run.join().unwrap()
        });
        assert!(matches!(r, Err(CheckError::Stopped)), "{r:?}");
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    }

    #[test]
    fn reading_stops_on_stop_even_with_output_left() {
        // io::repeat never runs dry: only the stop ends the reading.
        let stop = AtomicBool::new(false);
        let mut buf = Vec::new();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                stop.store(true, Ordering::Relaxed);
            });
            let start = Instant::now();
            let eof = read_available(&mut std::io::repeat(b'y'), &mut buf, 1000, start + TIMEOUT, &stop);
            assert!(!eof);
            assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
        });
        assert_eq!(buf.len(), 1001);
        // And at the deadline.
        let start = Instant::now();
        let never = AtomicBool::new(false);
        read_available(&mut std::io::repeat(b'y'), &mut buf, 1000, start + Duration::from_millis(200), &never);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(read_available(&mut std::io::empty(), &mut buf, 1000, start + TIMEOUT, &never));
    }

    #[test]
    fn stops_a_command_that_writes_without_end() {
        // Past the output cap the reading goes on, dropping what comes:
        // the stop must be seen in there too.
        let stop = AtomicBool::new(false);
        let r = std::thread::scope(|s| {
            let run = s.spawn(|| run_until("yes", TIMEOUT, MAX_OUTPUT, &stop));
            std::thread::sleep(Duration::from_millis(300));
            stop.store(true, Ordering::Relaxed);
            let stopped = Instant::now();
            let r = run.join().unwrap();
            assert!(stopped.elapsed() < Duration::from_secs(1), "{:?}", stopped.elapsed());
            r
        });
        assert!(matches!(r, Err(CheckError::Stopped)), "{r:?}");
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
