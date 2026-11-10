//! Programs run without a terminal, on a core's machine or one it reaches
//! (`machine.exec`): git for folders and changes, checks for agents.

use base64::Engine as _;
use herdc_model::model::FolderInfo;
use herdc_wire::Client;
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct Ran {
    /// None: a signal ended it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Runs `argv` there (`on`: another machine; `cwd`: else the core's own,
/// that machine's home with `on`). A nonzero exit is a result, not an error.
pub async fn exec(core: &Client, on: Option<&str>, argv: &[String], cwd: Option<&str>, timeout: Duration) -> Result<Ran, String> {
    let params = json!({"argv": argv, "on": on, "cwd": cwd, "timeout": timeout.as_secs_f64()});
    let v = core
        .call_timeout("machine.exec", params, timeout + Duration::from_secs(5))
        .await
        .map_err(|e| e.to_string())?;
    let bytes = |k: &str| base64::engine::general_purpose::STANDARD.decode(v[k].as_str().unwrap_or_default()).unwrap_or_default();
    Ok(Ran {
        code: v["code"].as_i64().map(|c| c as i32),
        stdout: bytes("stdout"),
        stderr: bytes("stderr"),
        timed_out: v["timed_out"] == true,
    })
}

/// The repo a folder is in (a worktree's own top), else the folder itself,
/// and its branch; and whether it is a repo (or git couldn't say).
pub async fn folder_info(core: &Client, cwd: &str, home: &str, on: Option<&str>) -> (FolderInfo, bool) {
    let none = FolderInfo { top: cwd.into(), branch: None };
    if on.is_none() && cwd == home {
        return (none, false);
    }
    let argv = ["git", "-C", cwd, "rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"].map(str::to_owned);
    let ran = exec(core, on, &argv, None, Duration::from_secs(10)).await.ok();
    let Some(ran) = ran.clone().filter(Ran::ok) else {
        // Only git's own "no" means no repo; a failed run, a repo without
        // commits or one git won't read are asked again.
        let no = ran.is_some_and(|r| String::from_utf8_lossy(&r.stderr).to_lowercase().contains("not a git repository"));
        return (none, !no);
    };
    let out: Vec<String> = ran.out().lines().map(str::to_owned).collect();
    let [top, head] = out.as_slice() else { return (none, true) };
    let branch = (head != "HEAD").then(|| head.clone());
    (FolderInfo { top: top.clone(), branch }, true)
}

/// The home folder of the core's own machine.
/// The grid of a terminal nobody types in: the core's `headless` setting,
/// else its default 200x120.
pub async fn headless(core: &Client) -> (u16, u16) {
    let read = r#"cat "${XDG_CONFIG_HOME:-$HOME/.config}/herdr-core/config.toml""#;
    let argv = ["sh", "-c", read].map(str::to_owned);
    let config = exec(core, None, &argv, None, Duration::from_secs(10)).await.ok().filter(Ran::ok).map(|r| r.out());
    config.as_deref().and_then(headless_in).unwrap_or((200, 120))
}

/// `headless = "COLSxROWS"` among a config's top-level keys.
fn headless_in(config: &str) -> Option<(u16, u16)> {
    let line = config.lines().map(str::trim).take_while(|l| !l.starts_with('[')).find(|l| l.split('=').next().is_some_and(|k| k.trim() == "headless"))?;
    let (cols, rows) = line.split_once('=')?.1.trim().trim_matches(|c| c == '"' || c == '\'').split_once('x')?;
    Some((cols.trim().parse().ok()?, rows.trim().parse().ok()?)).filter(|&(c, r)| c > 0 && r > 0)
}

pub async fn home(core: &Client) -> Option<String> {
    let argv = ["sh", "-c", "printf %s \"$HOME\""].map(str::to_owned);
    let ran = exec(core, None, &argv, None, Duration::from_secs(10)).await.ok().filter(Ran::ok);
    match ran {
        Some(r) if !r.stdout.is_empty() => Some(r.out()),
        // Windows has no sh: its shell knows the profile folder.
        _ => {
            let argv = ["cmd", "/d", "/c", "echo %USERPROFILE%"].map(str::to_owned);
            let r = exec(core, None, &argv, None, Duration::from_secs(10)).await.ok().filter(Ran::ok)?;
            Some(r.out().trim().to_owned()).filter(|h| !h.is_empty())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn headless_is_a_top_level_key() {
        assert_eq!(super::headless_in("notes = false\nheadless = \"120x40\"\n"), Some((120, 40)));
        assert_eq!(super::headless_in("[other]\nheadless = \"120x40\""), None);
        assert_eq!(super::headless_in("headless = \"0x40\""), None);
        assert_eq!(super::headless_in(""), None);
    }
}
