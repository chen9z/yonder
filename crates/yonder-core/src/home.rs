//! Yonder's home: `$YONDER_HOME`, else `~/.yonder`. Everything Yonder keeps
//! is under it: `config.toml`, `ledger.db`, yonderd's lock, `logs/` and
//! `bots/`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// `$YONDER_HOME`, else `~/.yonder`; None without either.
pub fn dir() -> Option<PathBuf> {
    dir_from(std::env::var_os("YONDER_HOME"), std::env::var_os("HOME"))
}

pub fn dir_from(yonder_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    match (yonder_home.filter(|d| !d.is_empty()), home.filter(|h| !h.is_empty())) {
        (Some(dir), _) => Some(dir.into()),
        (None, Some(home)) => Some(Path::new(&home).join(".yonder")),
        (None, None) => None,
    }
}

/// The config file in `home`.
pub fn config_path(home: &Path) -> PathBuf {
    home.join("config.toml")
}

/// Where the bots are defined in `home`, one directory each.
pub fn bots_dir(home: &Path) -> PathBuf {
    home.join("bots")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_home() {
        let some = |s: &str| Some(OsString::from(s));
        assert_eq!(dir_from(some("/tmp/example/y"), some("/tmp/example")), Some("/tmp/example/y".into()));
        assert_eq!(dir_from(some(""), some("/tmp/example")), Some("/tmp/example/.yonder".into()));
        assert_eq!(dir_from(None, None), None);
    }
}
