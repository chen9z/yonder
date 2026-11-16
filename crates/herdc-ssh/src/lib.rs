//! The way in: SSH to the computer, then a pipe to its herdr-core, exactly as
//! herdr-core reaches its own machines (`machines.rs`, `pipe`). SSH is the
//! only authentication; the computer needs nothing installed beyond Herdr.
//!
//! Connecting has two steps so a person can check the host key before any
//! secret is sent: [`greet`] does the key exchange and reports the key,
//! [`Greeted::login`] authenticates.

use herdc_wire::Client;
use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{decode_secret_key, Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, MethodKind};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, DuplexStream};

/// What the pipe's shell line prints when the computer has no herdr-core.
pub const NOT_HERE: &str = "herdr-core is not installed here";
/// Where install.ps1 puts herdr-core on Windows, from the SSH home folder.
const WINDOWS_HC: &str = r".\AppData\Local\Programs\herdr-core\herdr-core.exe";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot reach {0}: {1}")]
    Unreachable(String, std::io::Error),
    #[error("ssh: {0}")]
    Ssh(#[from] russh::Error),
    #[error("the computer's key changed: expected {expected}, got {got}. If it was reinstalled, forget it and trust the new key")]
    HostKeyChanged { expected: String, got: String },
    #[error("the computer refused the login (it accepts: {0})")]
    Refused(String),
    #[error("Wrong user name or password.")]
    WrongPassword,
    #[error("The computer didn't answer the login.")]
    NoAnswer,
    #[error("cannot read the key: {0}")]
    Key(String),
    #[error("{NOT_HERE}: install Herdr on the computer first")]
    NotInstalled,
    #[error("herdr-core did not answer: {error}{}", if stderr.is_empty() { String::new() } else { format!("\n{stderr}") })]
    Core { error: herdc_wire::Error, stderr: String },
}

/// Where to log in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// A LAN name or address, or a Tailscale name (`pc`, `100.x.y.z`).
    pub host: String,
    pub port: u16,
    pub user: String,
}

#[derive(Clone)]
pub enum Credential {
    Password(String),
    /// An OpenSSH private key (`-----BEGIN OPENSSH PRIVATE KEY-----`).
    Key { openssh: String, passphrase: Option<String> },
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Credential::Password(_) => "Password(..)",
            Credential::Key { .. } => "Key(..)",
        })
    }
}

/// The computer's host key, as people compare it (`ssh-keygen -lf`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKey {
    pub algorithm: String,
    /// `SHA256:...`
    pub fingerprint: String,
}

impl HostKey {
    fn of(key: &PublicKey) -> Self {
        HostKey { algorithm: key.algorithm().to_string(), fingerprint: key.fingerprint(HashAlg::Sha256).to_string() }
    }
}

/// Records the host key; nothing secret is sent until the caller checked it.
struct Hand {
    seen: Arc<Mutex<Option<PublicKey>>>,
}

impl client::Handler for Hand {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        *self.seen.lock().unwrap() = Some(key.public_key());
        Ok(true)
    }
}

fn config() -> Arc<client::Config> {
    Arc::new(client::Config {
        // As herdr-core dials machines: ServerAliveInterval=15, ServerAliveCountMax=4.
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 4,
        inactivity_timeout: None,
        // Compressed when the computer offers it (OpenSSH does, from login
        // on): a terminal's output shrinks several times over.
        preferred: russh::Preferred {
            compression: std::borrow::Cow::Borrowed(&[russh::compression::ZLIB_LEGACY, russh::compression::ZLIB, russh::compression::NONE]),
            ..Default::default()
        },
        ..Default::default()
    })
}

/// A key exchange done, nobody logged in yet.
pub struct Greeted {
    handle: Handle<Hand>,
    pub host_key: HostKey,
}

/// How long reaching a computer and exchanging keys may take.
const GREET: Duration = Duration::from_secs(10);

/// Connects and exchanges keys. Pass the fingerprint trusted before (if any):
/// a different key fails here, before anything secret travels. Without
/// one, the caller shows `host_key` to be trusted before logging in. Something
/// that takes the connection and says nothing is unreachable too.
pub async fn greet(target: &Target, trusted: Option<&str>) -> Result<Greeted, Error> {
    let timed_out = || Error::Unreachable(target.host.clone(), std::io::ErrorKind::TimedOut.into());
    let seen = Arc::new(Mutex::new(None));
    let address = (target.host.as_str(), target.port);
    let stream = tokio::time::timeout(GREET, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| timed_out())?
        .map_err(|e| Error::Unreachable(target.host.clone(), e))?;
    let _ = stream.set_nodelay(true);
    let handle = tokio::time::timeout(GREET, client::connect_stream(config(), stream, Hand { seen: seen.clone() })).await.map_err(|_| timed_out())??;
    let key = seen.lock().unwrap().clone().ok_or(russh::Error::UnknownKey)?;
    let host_key = HostKey::of(&key);
    if let Some(expected) = trusted
        && expected != host_key.fingerprint
    {
        return Err(Error::HostKeyChanged { expected: expected.to_owned(), got: host_key.fingerprint });
    }
    Ok(Greeted { handle, host_key })
}

/// How long a login may take.
const LOGIN: Duration = Duration::from_secs(30);

impl Greeted {
    /// Logs in, within [`LOGIN`]. A password is offered as `password` and,
    /// where the computer doesn't take it that way, as
    /// `keyboard-interactive`. One `password` refused is wrong: it is not
    /// tried again that way (macOS's sshd then never answers).
    pub async fn login(self, user: &str, credential: &Credential) -> Result<Session, Error> {
        tokio::time::timeout(LOGIN, self.login_now(user, credential)).await.map_err(|_| Error::NoAnswer)?
    }

    async fn login_now(mut self, user: &str, credential: &Credential) -> Result<Session, Error> {
        let result = match credential {
            Credential::Password(password) => {
                let first = self.handle.authenticate_password(user, password).await?;
                match first {
                    russh::client::AuthResult::Success => true,
                    russh::client::AuthResult::Failure { remaining_methods, .. } => {
                        !remaining_methods.contains(&MethodKind::Password)
                            && remaining_methods.contains(&MethodKind::KeyboardInteractive)
                            && self.keyboard_interactive(user, password).await?
                    }
                }
            }
            Credential::Key { openssh, passphrase } => {
                let key = decode_secret_key(openssh, passphrase.as_deref()).map_err(|e| Error::Key(e.to_string()))?;
                let hash = self.handle.best_supported_rsa_hash().await?.flatten();
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
                self.handle.authenticate_publickey(user, key).await?.success()
            }
        };
        if !result {
            if matches!(credential, Credential::Password(_)) {
                return Err(Error::WrongPassword);
            }
            let methods = match self.handle.authenticate_none(user).await? {
                russh::client::AuthResult::Failure { remaining_methods, .. } => {
                    remaining_methods.iter().map(|m| <&str>::from(m).to_owned()).collect::<Vec<_>>().join(", ")
                }
                russh::client::AuthResult::Success => String::new(),
            };
            return Err(Error::Refused(methods));
        }
        Ok(Session { handle: Arc::new(self.handle) })
    }

    async fn keyboard_interactive(&mut self, user: &str, password: &str) -> Result<bool, Error> {
        let mut reply = self.handle.authenticate_keyboard_interactive_start(user, None::<String>).await?;
        // A server asks in rounds; a password login is one prompt (or none).
        for _ in 0..4 {
            match reply {
                KeyboardInteractiveAuthResponse::Success => return Ok(true),
                KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
                KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                    let answers = prompts.iter().map(|_| password.to_owned()).collect();
                    reply = self.handle.authenticate_keyboard_interactive_respond(answers).await?;
                }
            }
        }
        Ok(false)
    }
}

/// The operating system of the computer, for the pipe's shell line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Remote {
    #[default]
    Unix,
    Windows,
}

/// How to reach herdr-core once logged in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pipe {
    /// Herdr's own line: `herdr-core connect`, found on PATH, else in
    /// ~/.local/bin; `session` selects an `HC_SESSION`.
    Core { remote: Remote, session: Option<String> },
    /// A shell line of the person's own that carries the protocol on stdio.
    Custom(String),
}

impl Default for Pipe {
    fn default() -> Self {
        Pipe::Core { remote: Remote::Unix, session: None }
    }
}

impl Pipe {
    /// The command the SSH server runs (herdr-core `machines.rs`, `pipe`).
    pub fn line(&self) -> String {
        match self {
            Pipe::Custom(line) => line.clone(),
            Pipe::Core { remote: Remote::Windows, session } => {
                let session = session.as_deref().map(|s| format!(" --session {s}")).unwrap_or_default();
                format!("cmd /d /c \"if exist {WINDOWS_HC} ({WINDOWS_HC} connect{session}) else (echo {NOT_HERE} 1>&2 & exit /b 127)\"")
            }
            Pipe::Core { remote: Remote::Unix, session } => {
                let session = session.as_deref().map(|s| format!("HC_SESSION={s} ")).unwrap_or_default();
                format!(
                    "sh -c 'p=$(command -v herdr-core) || p=$HOME/.local/bin/herdr-core; [ -x \"$p\" ] || {{ echo \"{NOT_HERE}\" >&2; exit 127; }}; {session}exec \"$p\" connect'"
                )
            }
        }
    }
}

/// A logged-in SSH connection. Keep it while its pipes are in use.
#[derive(Clone)]
pub struct Session {
    handle: Arc<Handle<Hand>>,
}

/// One running command's stdio.
pub struct Exec {
    pub stdout: DuplexStream,
    pub stdin: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    /// What it wrote to stderr (the first 64 KiB).
    pub stderr: Arc<Mutex<Vec<u8>>>,
    pub exit: tokio::sync::watch::Receiver<Option<u32>>,
}

impl Session {
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Runs `line` on the computer with stdio as bytes (no PTY).
    pub async fn exec(&self, line: &str) -> Result<Exec, Error> {
        let channel = self.handle.channel_open_session().await?;
        channel.exec(true, line).await?;
        let (mut read, write) = channel.split();
        let stdin = Box::new(write.make_writer());
        let (mut tx, stdout) = tokio::io::duplex(256 * 1024);
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let (exit_tx, exit) = tokio::sync::watch::channel(None);
        let errors = stderr.clone();
        tokio::spawn(async move {
            // Keep the write half's channel open as long as output flows.
            let _write = write;
            while let Some(message) = read.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        if tx.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    ChannelMsg::ExtendedData { data, ext: 1 } => {
                        let mut e = errors.lock().unwrap();
                        let room = 65536usize.saturating_sub(e.len());
                        e.extend_from_slice(&data[..data.len().min(room)]);
                    }
                    ChannelMsg::ExitStatus { exit_status } => {
                        let _ = exit_tx.send(Some(exit_status));
                    }
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
            let _ = tx.shutdown().await;
        });
        Ok(Exec { stdout, stdin, stderr, exit })
    }

    /// Opens the pipe to herdr-core and says hello.
    pub async fn connect_core(&self, pipe: &Pipe) -> Result<Client, Error> {
        let exec = self.exec(&pipe.line()).await?;
        let Exec { stdout, stdin, stderr, mut exit } = exec;
        match Client::handshake(stdout, stdin).await {
            Ok(client) => Ok(client),
            Err(error) => {
                // Let the exit status and stderr arrive before explaining.
                let _ = tokio::time::timeout(Duration::from_secs(2), exit.wait_for(Option::is_some)).await;
                let text = String::from_utf8_lossy(&stderr.lock().unwrap()).trim().to_owned();
                if text.contains(NOT_HERE) {
                    return Err(Error::NotInstalled);
                }
                Err(Error::Core { error, stderr: text })
            }
        }
    }

    pub async fn close(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "", "en").await;
    }
}

/// A new ed25519 key for this phone: (private OpenSSH PEM, public line to
/// add to the computer's `~/.ssh/authorized_keys`).
pub fn generate_key(comment: &str) -> Result<(String, String), Error> {
    let mut key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).map_err(|e| Error::Key(e.to_string()))?;
    key.set_comment(comment);
    let private = key.to_openssh(russh::keys::ssh_key::LineEnding::LF).map_err(|e| Error::Key(e.to_string()))?;
    let public = key.public_key().to_openssh().map_err(|e| Error::Key(e.to_string()))?;
    Ok((private.to_string(), public))
}

/// The public line of a private key the person pasted.
pub fn public_line(openssh: &str, passphrase: Option<&str>) -> Result<String, Error> {
    let key = decode_secret_key(openssh, passphrase).map_err(|e| Error::Key(e.to_string()))?;
    key.public_key().to_openssh().map_err(|e| Error::Key(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unix_line_is_herdr_cores_own() {
        assert_eq!(
            Pipe::default().line(),
            "sh -c 'p=$(command -v herdr-core) || p=$HOME/.local/bin/herdr-core; [ -x \"$p\" ] || { echo \"herdr-core is not installed here\" >&2; exit 127; }; exec \"$p\" connect'"
        );
        let line = Pipe::Core { remote: Remote::Unix, session: Some("scratch".into()) }.line();
        assert!(line.contains("HC_SESSION=scratch exec"));
        let line = Pipe::Core { remote: Remote::Windows, session: Some("s".into()) }.line();
        assert!(line.starts_with("cmd /d /c") && line.contains("connect --session s"));
    }

    #[test]
    fn generated_keys_round_trip() {
        let (private, public) = generate_key("herdc@phone").unwrap();
        assert!(public.starts_with("ssh-ed25519 ") && public.ends_with("herdc@phone"));
        assert_eq!(public_line(&private, None).unwrap(), public);
        assert!(public_line("not a key", None).is_err());
    }
}
