//! `~/.config/yonder/config.toml`: who the lead is, the workers, where to
//! push. Everything personal lives there, never in this repository. A
//! missing file, or a missing section or key, means the local default: one
//! machine, no workers, no state file, no pushes.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub lead: Lead,
    pub state_file: StateFile,
    pub startup_guards: StartupGuards,
    pub workers: Vec<Worker>,
    pub notify: Notify,
    pub backup: Backup,
    pub privacy: Privacy,
    pub hc: Hc,
    pub daemon: Daemon,
    pub service: Service,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Lead {
    pub name: String,
    pub workspace: String,
    /// Empty: never started by Yonder.
    pub launch_cmd: Vec<String>,
    pub herdr_space: String,
    pub extra_labels: Vec<String>,
    /// The `key=value` label its terminal carries.
    pub label: String,
}

impl Default for Lead {
    fn default() -> Lead {
        Lead {
            name: "lead".into(),
            workspace: String::new(),
            launch_cmd: Vec::new(),
            herdr_space: String::new(),
            extra_labels: Vec::new(),
            label: "yonder.role=lead".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StateFile {
    pub enabled: bool,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StartupGuards {
    pub trusted_hook_sources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Worker {
    pub name: String,
    /// `local`, or an hc machine name.
    pub machine: String,
    pub cwd: String,
    pub template: String,
    pub session_id: String,
    pub foreground_match: String,
    pub extra_labels: Vec<String>,
    pub notify_owner_on_rebind: bool,
}

impl Default for Worker {
    fn default() -> Worker {
        Worker {
            name: String::new(),
            machine: "local".into(),
            cwd: String::new(),
            template: String::new(),
            session_id: String::new(),
            foreground_match: String::new(),
            extra_labels: Vec::new(),
            notify_owner_on_rebind: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Notify {
    /// A notification on this computer too (macOS only).
    pub local: bool,
    pub ntfy: Ntfy,
}

impl Default for Notify {
    fn default() -> Notify {
        Notify { local: true, ntfy: Ntfy::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Ntfy {
    pub enabled: bool,
    pub url: String,
    /// The secret store key of the token; the token itself is never here.
    pub token_secret: String,
    pub click_package: String,
}

impl Default for Ntfy {
    fn default() -> Ntfy {
        Ntfy {
            enabled: false,
            url: String::new(),
            token_secret: "ntfy_token".into(),
            click_package: "dev.yonder.app".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Backup {
    pub local_keep_days: u32,
    /// Empty: no copy on another machine.
    pub remote_machine: String,
    pub remote_dir: String,
}

impl Default for Backup {
    fn default() -> Backup {
        Backup {
            local_keep_days: 30,
            remote_machine: String::new(),
            remote_dir: "~/backups/yonder-ledger".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Privacy {
    pub default_redacted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Hc {
    /// The hc program; empty: `hc` from PATH. `YONDER_HC` wins over both.
    pub bin: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Daemon {
    /// Seconds between yonderd's rounds.
    pub tick_secs: u64,
}

impl Default for Daemon {
    fn default() -> Daemon {
        Daemon { tick_secs: 15 }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Service {
    /// The launchd label yonderd runs under.
    pub label: String,
}

impl Default for Service {
    fn default() -> Service {
        Service { label: "dev.yonder.daemon".into() }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {0}: {1}")]
    Read(PathBuf, std::io::Error),
    #[error("parsing {0}: {1}")]
    Parse(PathBuf, toml::de::Error),
    #[error("{0}: two workers are named {1:?}")]
    DuplicateWorker(PathBuf, String),
    #[error("{0}: {1}")]
    Invalid(PathBuf, &'static str),
}

impl Config {
    /// `$YONDER_CONFIG`, else `~/.config/yonder/config.toml`, else nothing.
    pub fn default_path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("YONDER_CONFIG").filter(|p| !p.is_empty()) {
            return Some(path.into());
        }
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
        Some(Path::new(&home).join(".config/yonder/config.toml"))
    }

    /// The config at `default_path`, or the defaults.
    pub fn load_default() -> Result<Config, ConfigError> {
        Config::default_path().map_or(Ok(Config::default()), |p| Config::load(&p))
    }

    /// The config at `path`, or the defaults if there is no file there.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(ConfigError::Read(path.into(), e)),
        };
        let config: Config =
            toml::from_str(&text).map_err(|e| ConfigError::Parse(path.into(), e))?;
        for (i, w) in config.workers.iter().enumerate() {
            if config.workers[..i].iter().any(|o| o.name == w.name) {
                return Err(ConfigError::DuplicateWorker(path.into(), w.name.clone()));
            }
        }
        if !(1..=3600).contains(&config.daemon.tick_secs) {
            return Err(ConfigError::Invalid(path.into(), "daemon.tick_secs must be 1 to 3600"));
        }
        if !config.lead.label.contains('=') {
            return Err(ConfigError::Invalid(path.into(), "lead.label must be key=value"));
        }
        Ok(config)
    }
}

/// `path` with a leading `~/` put in `home`.
pub fn expand_home(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.into(),
        None => path.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../config.example.toml");

    fn write_temp(name: &str, text: &str) -> PathBuf {
        let dir = crate::test_dir(name);
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn parses_example_config() {
        let path = write_temp("parses_example_config", EXAMPLE);
        let c = Config::load(&path).unwrap();
        assert_eq!(c.lead.name, "nova");
        assert_eq!(c.lead.launch_cmd, ["zsh", "-lic", "exec pi"]);
        assert!(c.state_file.enabled);
        assert_eq!(c.state_file.path, "/tmp/example/state.md");
        assert_eq!(c.workers.len(), 2);
        let trainer = &c.workers[0];
        assert_eq!((trainer.name.as_str(), trainer.machine.as_str()), ("trainer", "local"));
        assert_eq!(trainer.foreground_match, "codex");
        let ops = &c.workers[1];
        assert_eq!((ops.name.as_str(), ops.machine.as_str()), ("ops", "gpu-box"));
        // Keys left out take their defaults.
        assert!(ops.notify_owner_on_rebind);
        assert!(ops.extra_labels.is_empty());
        assert!(c.notify.ntfy.enabled);
        assert_eq!(c.notify.ntfy.token_secret, "ntfy_token");
        assert_eq!(c.backup.remote_machine, "homelab");
        assert_eq!(c.backup.local_keep_days, 14);
        assert!(!c.privacy.default_redacted);
        assert_eq!(c.lead.label, "yonder.role=lead");
        assert_eq!(c.hc.bin, "/tmp/example/bin/hc");
        assert_eq!(c.daemon.tick_secs, 30);
        assert!(!c.notify.local);
        assert_eq!(c.service.label, "dev.yonder.daemon");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_config_uses_defaults() {
        let dir = crate::test_dir("missing_config_uses_defaults");
        let c = Config::load(&dir.join("config.toml")).unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.lead.name, "lead");
        assert!(c.lead.launch_cmd.is_empty());
        assert!(c.workers.is_empty());
        assert!(!c.state_file.enabled);
        assert!(!c.notify.ntfy.enabled);
        assert_eq!(c.notify.ntfy.click_package, "dev.yonder.app");
        assert_eq!(c.backup.local_keep_days, 30);
        assert!(c.backup.remote_machine.is_empty());
        assert_eq!(c.lead.label, "yonder.role=lead");
        assert!(c.hc.bin.is_empty());
        assert_eq!(c.daemon.tick_secs, 15);
        assert!(c.notify.local);
        assert_eq!(c.service.label, "dev.yonder.daemon");
        // So does an empty file, and one with only some sections.
        let path = write_temp("missing_config_uses_defaults_empty", "");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let path2 = write_temp("missing_config_uses_defaults_part", "[privacy]\ndefault_redacted = true\n");
        let c = Config::load(&path2).unwrap();
        assert!(c.privacy.default_redacted);
        assert_eq!(c.backup, Backup::default());
        for p in [&dir, &path, &path2] {
            let _ = std::fs::remove_dir_all(if p.is_dir() { p } else { p.parent().unwrap() });
        }
    }

    #[test]
    fn rejects_bad_config() {
        let path = write_temp("rejects_bad_config", "[lead]\nname = 3\n");
        assert!(matches!(Config::load(&path), Err(ConfigError::Parse(..))));
        let twice = "[[workers]]\nname = \"ops\"\n[[workers]]\nname = \"ops\"\n";
        std::fs::write(&path, twice).unwrap();
        assert!(matches!(Config::load(&path), Err(ConfigError::DuplicateWorker(_, n)) if n == "ops"));
        for bad in ["[daemon]\ntick_secs = 0\n", "[lead]\nlabel = \"lead\"\n"] {
            std::fs::write(&path, bad).unwrap();
            assert!(matches!(Config::load(&path), Err(ConfigError::Invalid(..))), "{bad}");
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn expands_home() {
        let home = Path::new("/tmp/example");
        assert_eq!(expand_home("~/bots/ops", home), Path::new("/tmp/example/bots/ops"));
        assert_eq!(expand_home("~", home), home);
        assert_eq!(expand_home("/srv/x", home), Path::new("/srv/x"));
    }
}
