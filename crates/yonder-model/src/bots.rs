//! What `yonder bot ls --json` and `yonder bot status --json` print, as
//! the app reads them (from the computer, through hc's `machine.exec`),
//! and the one list it shows of the two. Fields the app does not use are
//! left out; ones that may be missing (an invalid bot has only its name,
//! problems and files) default.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A problem with a bot's definition, as `bot ls --json` says it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotProblem {
    /// `error` or `warning`.
    pub level: String,
    pub message: String,
}

/// One bot of `yonder bot ls --json`: its definition's fields at the top
/// (none but `name` when it is invalid), whether it is valid, its problems.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotListed {
    pub name: String,
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub description: Option<String>,
    /// `persistent` or `oneoff`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub problems: Vec<BotProblem>,
}

/// One bot of `yonder bot status --json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotStatus {
    pub name: String,
    /// `stopped`, `starting`, `ready`, `failed`, `gone`, `paused`, or
    /// `unknown` when hc did not answer.
    pub state: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub hc_error: Option<String>,
}

/// A bot as the app lists it: its definition and its state together.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRow {
    pub name: String,
    pub valid: bool,
    pub description: String,
    pub machine: String,
    pub harness: String,
    pub kind: String,
    /// Why it is invalid: its first error.
    pub problem: Option<String>,
    /// As `BotStatus::state`; `stopped` if status did not say.
    pub state: String,
    pub address: Option<String>,
    pub session: Option<String>,
    pub reason: Option<String>,
}

/// The two lists made one, in `ls`'s order (by name).
pub fn bot_rows(listed: &[BotListed], statuses: &[BotStatus]) -> Vec<BotRow> {
    listed
        .iter()
        .map(|l| {
            let status = statuses.iter().find(|s| s.name == l.name);
            let problem = l.problems.iter().find(|p| p.level == "error").map(|p| p.message.clone());
            BotRow {
                name: l.name.clone(),
                valid: l.valid,
                description: l.description.clone().unwrap_or_default(),
                machine: l.machine.clone().unwrap_or_default(),
                harness: l.harness.clone().unwrap_or_default(),
                kind: l.kind.clone().unwrap_or_default(),
                problem: problem.or_else(|| (!l.valid).then(|| "invalid".to_owned())),
                state: status.map_or_else(|| "stopped".to_owned(), |s| s.state.clone()),
                address: status.and_then(|s| s.address.clone()),
                session: status.and_then(|s| s.session.clone()),
                reason: status.and_then(|s| s.reason.clone().or_else(|| s.hc_error.clone())),
            }
        })
        .collect()
}

/// The JSON array in what a command printed: the whole of it, or from
/// the first line that starts one (a login shell may print before it).
pub fn json_array<T: DeserializeOwned>(out: &str) -> Result<Vec<T>, String> {
    if let Ok(all) = serde_json::from_str(out) {
        return Ok(all);
    }
    let start = out.lines().scan(0, |at, line| {
        let here = *at;
        *at += line.len() + 1;
        Some((here, line))
    });
    let from = start.filter(|(_, line)| line.trim_start().starts_with('[')).map(|(at, _)| at).next();
    match from {
        Some(at) => serde_json::from_str(&out[at..]).map_err(|e| e.to_string()),
        None => Err("no JSON list in what it printed".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_are_made_one() {
        let listed: Vec<BotListed> = json_array(
            r#"[{"name":"ops","valid":true,"description":"homelab ops","kind":"persistent","machine":"local","harness":"pi","cwd":"/tmp/example","problems":[],"files":{}},
                {"name":"bad","valid":false,"dir":"/tmp/example/bad","files":{},"problems":[{"level":"warning","message":"w"},{"level":"error","message":"no bot.toml"}]}]"#,
        )
        .unwrap();
        let statuses: Vec<BotStatus> = json_array(
            "motd from a login shell\n[{\"name\":\"ops\",\"state\":\"ready\",\"paused\":false,\"address\":\"t70\",\"session\":\"s-1\",\"reason\":null,\"hc_error\":null}]",
        )
        .unwrap();
        let rows = bot_rows(&listed, &statuses);
        assert_eq!(rows[0].state, "ready");
        assert_eq!((rows[0].address.as_deref(), rows[0].session.as_deref(), rows[0].description.as_str()), (Some("t70"), Some("s-1"), "homelab ops"));
        assert_eq!((rows[1].valid, rows[1].problem.as_deref(), rows[1].state.as_str()), (false, Some("no bot.toml"), "stopped"));
        assert!(json_array::<BotStatus>("nothing").is_err());
    }
}
