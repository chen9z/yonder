//! A computer to log in to, for tests and for trying the phone app: an SSH
//! server in this process that takes one user with a password (also as
//! macOS asks for it, keyboard-interactive) or with a key in its
//! `authorized_keys`, and runs every command in a [`Session`]'s
//! environment, so Herdr's own pipe line reaches that session's core and
//! nothing touches the user's.

use crate::Session;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::ChildStdin;
use tokio::sync::Mutex;

pub struct Options {
    pub user: String,
    pub password: String,
    /// Passwords only through keyboard-interactive (as macOS by default).
    pub keyboard_only: bool,
    /// Where to listen: `127.0.0.1:0` for a test, `0.0.0.0:2222` for a phone.
    pub listen: String,
    /// Where its host key is kept (made there the first time), so a phone
    /// still trusts it once it starts again; None: a new one each time.
    pub host_key: Option<std::path::PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options { user: "me".into(), password: "open sesame".into(), keyboard_only: false, listen: "127.0.0.1:0".into(), host_key: None }
    }
}

/// A running computer.
pub struct Computer {
    pub port: u16,
    /// Its host key, as `ssh-keygen -lf` shows it.
    pub fingerprint: String,
}

#[derive(Clone)]
struct Server {
    options: Arc<Options>,
    env: Arc<Vec<(String, String)>>,
    home: PathBuf,
    /// This connection's programs' stdins, by channel (channels are numbered
    /// per connection). Dropped with the connection, as sshd closes them:
    /// a program reading its stdin ends when the line does.
    stdins: Arc<Mutex<HashMap<ChannelId, ChildStdin>>>,
}

impl server::Server for Server {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Server { stdins: Arc::default(), ..self.clone() }
    }
}

impl Server {
    fn methods(&self) -> MethodSet {
        let mut set = vec![MethodKind::PublicKey, MethodKind::KeyboardInteractive];
        if !self.options.keyboard_only {
            set.push(MethodKind::Password);
        }
        MethodSet::from(&set[..])
    }

    fn reject(&self) -> Auth {
        Auth::Reject { proceed_with_methods: Some(self.methods()), partial_success: false }
    }

    /// The keys in the session home's `~/.ssh/authorized_keys`.
    fn authorized(&self) -> Vec<PublicKey> {
        let text = std::fs::read_to_string(self.home.join(".ssh/authorized_keys")).unwrap_or_default();
        text.lines().filter_map(|l| PublicKey::from_openssh(l.trim()).ok()).collect()
    }
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(self.reject())
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let ok = !self.options.keyboard_only && user == self.options.user && password == self.options.password;
        Ok(if ok { Auth::Accept } else { self.reject() })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        match response {
            None => Ok(Auth::Partial {
                name: Cow::Borrowed(""),
                instructions: Cow::Borrowed(""),
                prompts: Cow::Owned(vec![(Cow::Borrowed("Password:"), false)]),
            }),
            Some(mut answers) => {
                let ok = user == self.options.user && answers.next().is_some_and(|a| &a[..] == self.options.password.as_bytes());
                Ok(if ok { Auth::Accept } else { Auth::reject() })
            }
        }
    }

    async fn auth_publickey(&mut self, user: &str, key: server::VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        let ours = self.authorized().iter().any(|k| k.key_data() == key.public_key.key_data());
        Ok(if user == self.options.user && ours { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(&mut self, _: Channel<Msg>, reply: server::ChannelOpenHandle, _: &mut server::Session) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, line: &[u8], session: &mut server::Session) -> Result<(), Self::Error> {
        let line = String::from_utf8_lossy(line).into_owned();
        let spawned = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&line)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(self.env.iter().cloned())
            .current_dir(&self.home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        self.stdins.lock().await.insert(channel, child.stdin.take().unwrap());
        let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let handle = session.handle();
        session.channel_success(channel)?;
        tokio::spawn(async move {
            let errors = {
                let handle = handle.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0; 4096];
                    while let Ok(n @ 1..) = err.read(&mut buf).await {
                        let _ = handle.extended_data(channel, 1, buf[..n].to_vec()).await;
                    }
                })
            };
            let mut buf = vec![0; 65536];
            while let Ok(n @ 1..) = out.read(&mut buf).await {
                if handle.data(channel, buf[..n].to_vec()).await.is_err() {
                    break;
                }
            }
            let _ = errors.await;
            let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(255) as u32;
            let _ = handle.exit_status_request(channel, code).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _: &mut server::Session) -> Result<(), Self::Error> {
        if let Some(stdin) = self.stdins.lock().await.get_mut(&channel) {
            let _ = stdin.write_all(data).await;
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _: &mut server::Session) -> Result<(), Self::Error> {
        self.stdins.lock().await.remove(&channel);
        Ok(())
    }
}

/// Serves `session` over SSH until the runtime ends.
pub async fn serve(session: &Session, options: Options) -> std::io::Result<Computer> {
    let kept = options.host_key.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|k| PrivateKey::from_openssh(k).ok());
    let host_key = kept.unwrap_or_else(|| {
        let made = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("a host key");
        if let Some(path) = &options.host_key {
            let _ = std::fs::write(path, made.to_openssh(russh::keys::ssh_key::LineEnding::LF).expect("an OpenSSH key").as_bytes());
        }
        made
    });
    let fingerprint = host_key.public_key().fingerprint(HashAlg::Sha256).to_string();
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        auth_rejection_time: std::time::Duration::from_millis(10),
        auth_rejection_time_initial: Some(std::time::Duration::ZERO),
        ..Default::default()
    });
    let socket = tokio::net::TcpListener::bind(&options.listen).await?;
    let port = socket.local_addr()?.port();
    // Herdr's pipe finds herdr-core on PATH or in ~/.local/bin: the binary
    // tests use, wherever it is, stands in the session's home.
    let bin = session.home().join(".local/bin");
    std::fs::create_dir_all(&bin)?;
    let link = bin.join("herdr-core");
    if !link.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(std::fs::canonicalize(&session.bin).unwrap_or(session.bin.clone()), &link)?;
    }
    let mut server = Server { options: Arc::new(options), env: Arc::new(session.env()), home: session.home(), stdins: Arc::default() };
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &socket).await;
    });
    Ok(Computer { port, fingerprint })
}
