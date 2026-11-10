//! The computers this phone reaches, and the line to each: SSH over the
//! LAN or Tailscale, then herdr-core's own pipe. The password is kept on the
//! phone, sealed; a computer that has the phone's key (put there by hand,
//! Settings shows it) is signed in to with that.

use herdc_engine::{Connector, Engine, Options, View};
use herdc_ssh::{Credential, Pipe, Remote, Session, Target};
use nsh_core::storage::Storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::vault;
use std::sync::{Arc, OnceLock};

const COMPUTERS: &str = "computers";
const OPEN: &str = "open";
const PHONE_KEY: &str = "phone.key";
const PHONE_PUB: &str = "phone.pub";
/// What a computer showed last: `seen.<id>`.
const SEEN: &str = "seen.";
/// A view kept is at most so big (the storage holds 5 MiB in all): a
/// bigger one is kept without the terminals' records.
const SEEN_MOST: usize = 256 * 1024;

/// How the phone's key is kept, said for a diagnosis.
pub fn key_kept() -> &'static str {
    Storage::get_item(PHONE_KEY).map_or("none yet", |kept| vault::how(&kept))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Computer {
    /// What to call it (default: the host).
    pub name: String,
    /// A LAN name or address, or a Tailscale name.
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The host key trusted the first time (`SHA256:...`).
    pub fingerprint: Option<String>,
    /// Sealed; None: the computer has the phone's key.
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "sealed", deserialize_with = "opened")]
    pub password: Option<String>,
    #[serde(default)]
    pub windows: bool,
    /// An `HC_SESSION` other than the default (tests).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

fn sealed<S: serde::Serializer>(words: &Option<String>, to: S) -> Result<S::Ok, S::Error> {
    words.as_deref().map(vault::seal).serialize(to)
}

/// What can't be opened here (sealed on another phone) is gone: the
/// password is asked again.
fn opened<'de, D: serde::Deserializer<'de>>(from: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(from)?.and_then(|kept| vault::open(&kept)))
}

impl Computer {
    /// `user@host:port`, and `/session` for a session other than the default.
    pub fn id(&self) -> String {
        let session = self.session.as_ref().map(|s| format!("/{s}")).unwrap_or_default();
        format!("{}@{}:{}{session}", self.user, self.host, self.port)
    }

    pub fn target(&self) -> Target {
        Target { host: self.host.clone(), port: self.port, user: self.user.clone() }
    }

    pub fn pipe(&self) -> Pipe {
        let remote = if self.windows { Remote::Windows } else { Remote::Unix };
        Pipe::Core { remote, session: self.session.clone() }
    }

    fn credential(&self, key: &str) -> Credential {
        match &self.password {
            Some(p) => Credential::Password(p.clone()),
            None => Credential::Key { openssh: key.to_owned(), passphrase: None },
        }
    }
}

pub fn computers() -> Vec<Computer> {
    Storage::get_item(COMPUTERS).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Keeps `computer` first (the one opened last), replacing its old entry.
pub fn remember(computer: &Computer) {
    let mut all = computers();
    all.retain(|c| c.id() != computer.id());
    all.insert(0, computer.clone());
    keep(&all);
}

/// The computer the app was on when it stopped, to open again (none once
/// the person left it).
pub fn reopen() -> Option<Computer> {
    let id = Storage::get_item(OPEN)?;
    computers().into_iter().find(|c| c.id() == id)
}

pub fn set_open(id: Option<&str>) {
    match id {
        Some(id) => drop(Storage::set_item(OPEN, id)),
        None => Storage::remove_item(OPEN),
    }
}

pub fn forget(id: &str) {
    let mut all = computers();
    all.retain(|c| c.id() != id);
    keep(&all);
    Storage::remove_item(&format!("{SEEN}{id}"));
}

/// What a computer showed when it was last connected, and when (seconds
/// since 1970): drawn while it is reached again, and listed with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub at: u64,
    pub view: View,
    /// The terminals this phone started there: still the phone's, at its
    /// grid, once the app starts again.
    #[serde(default)]
    pub started: Vec<String>,
}

pub fn seen(id: &str) -> Option<Seen> {
    serde_json::from_str(&Storage::get_item(&format!("{SEEN}{id}"))?).ok()
}

/// Keeps what computer `id` shows now, and which terminals the phone started.
pub fn keep_seen(id: &str, view: &View, started: Vec<String>) {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut seen = Seen { at, view: view.clone(), started };
    let mut kept = serde_json::to_string(&seen).unwrap_or_default();
    if kept.len() > SEEN_MOST {
        seen.view.terminals.clear();
        kept = serde_json::to_string(&seen).unwrap_or_default();
    }
    if let Err(full) = Storage::set_item(format!("{SEEN}{id}"), kept) {
        log::warn!("{full}");
    }
}

fn keep(all: &[Computer]) {
    if let Err(full) = Storage::set_item(COMPUTERS, serde_json::to_string(all).unwrap_or_default()) {
        log::warn!("{full}");
    }
}

/// This phone's own SSH key: (private OpenSSH, public line), made once and
/// kept sealed. One that can't be opened here (the app's data came from
/// another phone, or from before it was installed again) is replaced: the
/// computers then ask for the password once more.
pub fn phone_key() -> (String, String) {
    static KEY: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);
    let mut known = KEY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(key) = known.as_ref() {
        return key.clone();
    }
    let kept = Storage::get_item(PHONE_KEY).zip(Storage::get_item(PHONE_PUB));
    let key = match kept.and_then(|(kept, public)| Some((vault::open(&kept)?, public, vault::sealed(&kept)))) {
        Some((private, public, sealed)) => {
            // Kept before keys were sealed: sealed now.
            if !sealed {
                let _ = Storage::set_item(PHONE_KEY, vault::seal(&private));
            }
            (private, public)
        }
        None => {
            let (private, public) = herdc_ssh::generate_key("herdc").expect("an ed25519 key");
            let _ = Storage::set_item(PHONE_KEY, vault::seal(&private));
            let _ = Storage::set_item(PHONE_PUB, public.clone());
            (private, public)
        }
    };
    *known = Some(key.clone());
    key
}

/// Where the engine, viewers and SSH live: a runtime of their own, there
/// whenever a page asks (NSH enters its runtime only in tasks).
pub fn runtime() -> tokio::runtime::Handle {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("herdc").enable_all().build().expect("a runtime")
    });
    runtime.handle().clone()
}

/// Logs in with what the phone keeps for `computer`.
async fn login(computer: &Computer, key: &str) -> Result<Session, String> {
    let greeted = herdc_ssh::greet(&computer.target(), computer.fingerprint.as_deref()).await.map_err(|e| e.to_string())?;
    greeted.login(&computer.user, &computer.credential(key)).await.map_err(|e| e.to_string())
}

/// The password a line logs in with: typed again once it was refused.
pub type Password = Arc<std::sync::Mutex<Option<String>>>;

/// Dials `computer`'s core, logging in again only once the SSH session is
/// gone. `first`: a session the person just logged in with. A password
/// refused is not tried again (the computer would count each try against
/// the account) until another is put in `password`.
pub fn connector(computer: Computer, key: String, first: Option<Session>, password: Password) -> Connector {
    let held = Arc::new(tokio::sync::Mutex::new((first, None::<String>)));
    Arc::new(move || {
        let (held, mut computer, key, password) = (held.clone(), computer.clone(), key.clone(), password.clone());
        Box::pin(async move {
            let mut held = held.lock().await;
            let (held, refused) = &mut *held;
            if held.as_ref().is_none_or(Session::is_closed) {
                computer.password = password.lock().unwrap().clone();
                let wrong = herdc_ssh::Error::WrongPassword.to_string();
                if computer.password.is_some() && *refused == computer.password {
                    return Err(wrong);
                }
                let session = login(&computer, &key).await;
                if session.as_ref().err() == Some(&wrong) {
                    refused.clone_from(&computer.password);
                }
                *held = Some(session?);
            }
            let session = held.as_ref().expect("a session");
            let core = session.connect_core(&computer.pipe()).await;
            if core.is_err() && session.is_closed() {
                *held = None;
            }
            core.map_err(|e| e.to_string())
        })
    })
}

/// Whether `computer`'s SSH port takes a connection now.
pub async fn reachable(computer: &Computer) -> bool {
    let dial = tokio::net::TcpStream::connect((computer.host.as_str(), computer.port));
    matches!(tokio::time::timeout(std::time::Duration::from_secs(1), dial).await, Ok(Ok(_)))
}

/// The display facts new terminals start with (herdr-core `Display`).
pub fn display(palette: &herdc_term::Palette, cell: (f32, f32)) -> Value {
    json!({
        "fg": palette.foreground.hex(),
        "bg": palette.background.hex(),
        "cursor": palette.cursor.hex(),
        "cell": [cell.0.round() as u32, cell.1.round() as u32],
    })
}

/// Starts the engine for `computer` on the app's runtime.
/// `size`: the grid new terminals start at until a pane says its own.
pub fn open(computer: &Computer, first: Option<Session>, display: Value, size: (u16, u16), password: Password) -> Engine {
    let _entered = runtime().enter();
    let (key, _) = phone_key();
    Engine::start(connector(computer.clone(), key, first, password), Options { store: None, size: Some(size), display })
}

#[cfg(test)]
mod tests {
    use super::Lost;

    #[test]
    fn why_a_line_went() {
        assert_eq!(Lost::of("cannot reach 10.0.0.2: Network is unreachable (os error 101)"), Lost::NoNetwork);
        assert_eq!(Lost::of("cannot reach 10.0.0.2: Connection refused (os error 61)"), Lost::Refused);
        assert!(Lost::of("lost the connection to the core").network());
        let login = Lost::of("the computer refused the login (it accepts: publickey)");
        assert!(!login.network());
        assert_eq!(login.said("pc"), "The computer refused the login (it accepts: publickey)");
    }
}

/// Keeps `computer` as it is now, where it is in the list.
pub fn update(computer: &Computer) {
    let all: Vec<Computer> = computers().into_iter().map(|c| if c.id() == computer.id() { computer.clone() } else { c }).collect();
    keep(&all);
}


/// Why the line is down.
#[derive(Clone, Debug, PartialEq)]
pub enum Lost {
    NoNetwork,
    Refused,
    Silent,
    Unknown,
    Unreachable,
    Dropped,
    WrongPassword,
    /// Not the network: the login, the key, Herdr itself.
    Other(String),
}

impl Lost {
    pub fn of(error: &str) -> Lost {
        if let Some(io) = error.strip_prefix("cannot reach ").and_then(|e| e.split_once(": ")).map(|(_, io)| io.to_ascii_lowercase()) {
            return if io.contains("unreachable") || io.contains("no route") {
                Lost::NoNetwork
            } else if io.contains("refused") {
                Lost::Refused
            } else if io.contains("timed out") {
                Lost::Silent
            } else if io.contains("lookup") || io.contains("resolve") || io.contains("not known") {
                Lost::Unknown
            } else {
                Lost::Unreachable
            };
        }
        if error == herdc_ssh::Error::WrongPassword.to_string() { return Lost::WrongPassword; }
        if error == "lost the connection to the core" || error.contains("no reply from the core") {
            return Lost::Dropped;
        }
        Lost::Other(error.to_owned())
    }

    /// Whether it is the network (then the computer is looked for until it
    /// answers again).
    pub fn network(&self) -> bool {
        !matches!(self, Lost::Other(_) | Lost::WrongPassword)
    }

    /// As the person reads it, when nothing tries again (adding a computer).
    pub fn why(&self, host: &str) -> String {
        match self {
            Lost::NoNetwork => t!("No network.").to_owned(),
            Lost::Refused => t!("{} isn't taking SSH connections.", host),
            Lost::Silent => t!("{} doesn't answer.", host),
            Lost::Unknown => t!("Can't find {} on this network.", host),
            Lost::Unreachable => t!("Can't reach {}.", host),
            _ => crate::i18n::tr_string(&self.said(host)),
        }
    }

    /// As the person reads it.
    pub fn said(&self, host: &str) -> String {
        match self {
            Lost::NoNetwork => t!("No network. Reconnecting when it's back…").to_owned(),
            Lost::Refused => t!("{} isn't taking SSH connections. Trying again…", host),
            Lost::Silent => t!("{} doesn't answer. Trying again…", host),
            Lost::Unknown => t!("Can't find {} on this network. Trying again…", host),
            Lost::Unreachable => t!("Can't reach {}. Trying again…", host),
            Lost::Dropped => t!("Connection lost. Reconnecting…").to_owned(),
            Lost::WrongPassword => t!("{} didn't take the password kept for it.", host),
            Lost::Other(error) => {
                let mut chars = error.chars();
                chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
            }
        }
    }
}

