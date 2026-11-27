//! Yonder's home: `$YONDER_HOME`, else `~/.yonder`. Everything Yonder keeps
//! is under it: `config.toml`, `ledger.db`, yonderd's lock, `logs/` and
//! `bots/`. It must be an absolute path: yonderd, started by launchd from
//! elsewhere, has to find the same one as the yonder that set it up.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HomeError {
    #[error("no Yonder home: set YONDER_HOME or HOME")]
    None,
    #[error("Yonder's home {0:?} is not an absolute path")]
    Relative(PathBuf),
}

/// What to do about a relative home.
pub const ABSOLUTE: &str = "give YONDER_HOME as an absolute path, such as /tmp/example/yonder";

/// `$YONDER_HOME`, else `~/.yonder`; an absolute path.
pub fn dir() -> Result<PathBuf, HomeError> {
    dir_from(std::env::var_os("YONDER_HOME"), std::env::var_os("HOME"))
}

pub fn dir_from(yonder_home: Option<OsString>, home: Option<OsString>) -> Result<PathBuf, HomeError> {
    let dir: PathBuf = match (yonder_home.filter(|d| !d.is_empty()), home.filter(|h| !h.is_empty())) {
        (Some(dir), _) => dir.into(),
        (None, Some(home)) => Path::new(&home).join(".yonder"),
        (None, None) => return Err(HomeError::None),
    };
    if dir.is_absolute() { Ok(dir) } else { Err(HomeError::Relative(dir)) }
}

/// Makes `home` if it isn't there, only its owner may enter it (0700, as
/// the folders above it that are made too). One that is there is left as
/// it is.
pub fn ensure(home: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(home)
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
        assert_eq!(dir_from(some("/tmp/example/y"), some("/tmp/example")), Ok("/tmp/example/y".into()));
        assert_eq!(dir_from(some(""), some("/tmp/example")), Ok("/tmp/example/.yonder".into()));
        assert_eq!(dir_from(None, None), Err(HomeError::None));
        assert_eq!(dir_from(some("y"), some("/tmp/example")), Err(HomeError::Relative("y".into())));
        assert_eq!(dir_from(some("./y"), None), Err(HomeError::Relative("./y".into())));
        assert_eq!(dir_from(None, some("example")), Err(HomeError::Relative("example/.yonder".into())));
    }

    #[test]
    fn a_new_home_is_the_owners_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_dir("home-mode");
        let home = dir.join("a/b");
        ensure(&home).unwrap();
        for d in [dir.join("a"), home.clone()] {
            assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700, "{}", d.display());
        }
        // One that is there keeps its mode.
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o750)).unwrap();
        ensure(&home).unwrap();
        assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o750);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
