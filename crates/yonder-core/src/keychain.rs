//! Bots' environment values in the login keychain, through
//! `/usr/bin/security`: service `dev.yonder.bots` (`YONDER_KEYCHAIN_SERVICE`
//! changes it, for tests), account `<bot>/<VARIABLE>`.
//!
//! A value never goes in a process's arguments. It is written on
//! `security -i`'s stdin as one line, `add-generic-password -U -s <service>
//! -a <account> -X <hex> "<keychain>"`: hex (`-X`), so nothing in it can end
//! the line or a field. Read back with `find-generic-password -w`, which
//! prints it as text only when every byte is printable ASCII (else as hex):
//! so values are printable ASCII only, and come back as they went in.
//! The service, account and keychain path go on that line too: none may
//! hold white space, quotes, backslashes or control characters.
//! What `security` says on failure is never passed on when setting: it may
//! repeat the line.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::check_exec::hex;

pub const DEFAULT_SERVICE: &str = "dev.yonder.bots";
const TOOL: &str = "/usr/bin/security";
/// `security`'s exit status for an item that isn't there.
const NOT_FOUND: i32 = 44;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeychainError {
    /// A value that cannot be kept as it is.
    #[error("{0}")]
    BadValue(&'static str),
    /// A service, account or keychain path that cannot go on the line.
    #[error("{0}")]
    BadName(String),
    #[error("{0}")]
    Failed(String),
}

impl KeychainError {
    pub fn code(&self) -> &'static str {
        match self {
            KeychainError::BadValue(_) => "bad_value",
            KeychainError::BadName(_) | KeychainError::Failed(_) => "keychain",
        }
    }
}

pub struct Keychain {
    pub service: String,
    pub tool: PathBuf,
}

/// `<bot>/<VARIABLE>`.
pub fn account(bot: &str, var: &str) -> String {
    format!("{bot}/{var}")
}

/// One word on a `security -i` line: something, with no white space,
/// quotes, backslashes or control characters.
pub fn check_word(what: &str, word: &str) -> Result<(), KeychainError> {
    if word.is_empty() || word.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'' || c == '\\') {
        return Err(KeychainError::BadName(format!("{what} {word:?} cannot be passed to security")));
    }
    Ok(())
}

/// The path in what `security login-keychain` says: `"<path>"`, one pair
/// of quotes around it, and nothing in it `check_word` refuses.
pub fn keychain_path(said: &str) -> Result<String, KeychainError> {
    let quoted = said.trim();
    let path = quoted
        .strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .ok_or_else(|| KeychainError::Failed(format!("no login keychain: {quoted:?}")))?;
    check_word("the login keychain", path)?;
    Ok(path.to_owned())
}

/// A value that can be kept: printable ASCII (space to `~`), not empty.
pub fn check_value(value: &str) -> Result<(), KeychainError> {
    if value.is_empty() {
        return Err(KeychainError::BadValue("the value is empty"));
    }
    if !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return Err(KeychainError::BadValue("the value must be printable ASCII: no line breaks, control characters or other letters"));
    }
    Ok(())
}

impl Keychain {
    /// `$YONDER_KEYCHAIN_SERVICE`, else `dev.yonder.bots`.
    pub fn from_env() -> Keychain {
        let service = std::env::var("YONDER_KEYCHAIN_SERVICE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_SERVICE.into());
        Keychain { service, tool: TOOL.into() }
    }

    fn login_keychain(&self) -> Result<String, KeychainError> {
        let out = Command::new(&self.tool)
            .arg("login-keychain")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| KeychainError::Failed(format!("{}: {e}", self.tool.display())))?;
        if !out.status.success() {
            return Err(KeychainError::Failed(format!("no login keychain: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        keychain_path(&String::from_utf8_lossy(&out.stdout))
    }

    /// Keeps `value` at `account`, over what was there.
    pub fn set(&self, account: &str, value: &str) -> Result<(), KeychainError> {
        check_value(value)?;
        check_word("the keychain service", &self.service)?;
        check_word("the keychain account", account)?;
        let login = self.login_keychain()?;
        let line = format!("add-generic-password -U -s {} -a {account} -X {} \"{login}\"\n", self.service, hex(value.as_bytes()));
        let mut child = Command::new(&self.tool)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| KeychainError::Failed(format!("{}: {e}", self.tool.display())))?;
        let wrote = child.stdin.take().map(|mut stdin| stdin.write_all(line.as_bytes()));
        let status = child.wait().map_err(|e| KeychainError::Failed(e.to_string()))?;
        match (wrote, status.success()) {
            (Some(Ok(())), true) => Ok(()),
            _ => Err(KeychainError::Failed(format!("security did not keep it (exit {})", status.code().unwrap_or(-1)))),
        }
    }

    /// The value at `account`, or None if none is kept.
    pub fn get(&self, account: &str) -> Result<Option<String>, KeychainError> {
        check_word("the keychain service", &self.service)?;
        check_word("the keychain account", account)?;
        let login = self.login_keychain()?;
        let out = Command::new(&self.tool)
            .args(["find-generic-password", "-s", &self.service, "-a", account, "-w", &login])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| KeychainError::Failed(format!("{}: {e}", self.tool.display())))?;
        match out.status.code() {
            Some(0) => {
                let text = String::from_utf8(out.stdout).map_err(|_| KeychainError::Failed(format!("{account}: not text")))?;
                Ok(Some(text.strip_suffix('\n').unwrap_or(&text).to_string()))
            }
            Some(NOT_FOUND) => Ok(None),
            code => Err(KeychainError::Failed(format!("reading {account}: exit {}: {}", code.unwrap_or(-1), String::from_utf8_lossy(&out.stderr).trim()))),
        }
    }

    /// Forgets `account`; for tests.
    #[cfg(test)]
    fn delete(&self, account: &str) {
        let _ = Command::new(&self.tool).args(["delete-generic-password", "-s", &self.service, "-a", account]).output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_safe_words_go_on_the_line() {
        assert_eq!(keychain_path("    \"/tmp/example/login.keychain-db\"\n").as_deref(), Ok("/tmp/example/login.keychain-db"));
        for said in [
            "\"/tmp/example\\login.keychain-db\"",
            "\"/tmp/example/a\"b.keychain-db\"",
            "\"/tmp/example/a\nlist-keychains\"",
            "\"/tmp/example/a b.keychain-db\"",
            "\"/tmp/example/a\tb\"",
            "/tmp/example/login.keychain-db",
            "\"\"",
            "",
        ] {
            assert!(keychain_path(said).is_err(), "{said:?}");
        }
        assert!(check_word("service", "dev.yonder.bots").is_ok());
        assert!(check_word("account", &account("ops", "EXAMPLE_API_KEY")).is_ok());
        for bad in ["", "a b", "a\"b", "a'b", "a\\b", "a\nb", "a\rb", "a\0b"] {
            assert!(check_word("service", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn values_are_printable_ascii() {
        assert!(check_value("s3cret-b3 with \"quotes\" \\ and $x").is_ok());
        for bad in ["", "a\nb", "a\rb", "a\tb", "a\0b", "h\u{e9}llo", "a\u{7f}"] {
            assert_eq!(check_value(bad).unwrap_err().code(), "bad_value", "{bad:?}");
        }
    }

    /// The person's keychain, under a test service only, deleted after.
    #[cfg(target_os = "macos")]
    #[test]
    fn keeps_and_gives_back_values() {
        let k = Keychain { service: "dev.yonder.bots.test".into(), tool: TOOL.into() };
        let acct = format!("unit-{}/EXAMPLE_API_KEY", std::process::id());
        k.delete(&acct);
        assert_eq!(k.get(&acct), Ok(None));
        for value in ["synthetic-1", "with \"quotes\", a \\ and -X 00 \"x\""] {
            k.set(&acct, value).unwrap();
            assert_eq!(k.get(&acct).unwrap().as_deref(), Some(value));
        }
        assert_eq!(k.set(&acct, "a\nb").unwrap_err().code(), "bad_value");
        let bad = Keychain { service: "dev.yonder.bots.test x".into(), tool: TOOL.into() };
        assert!(bad.set(&acct, "x").is_err());
        k.delete(&acct);
        assert_eq!(k.get(&acct), Ok(None));
    }
}
