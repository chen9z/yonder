//! The agents herdr can start, and which of them a machine has: the
//! new-menu's `knownAgents({on})` (herdr-app `src/programs.rs`).

use super::{There, TIMEOUT};
use herdc_model::{programs, shell};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Known {
    /// Its programs.json entry: key, name, icon, run.
    pub program: Value,
    pub installed: bool,
}

impl Known {
    pub fn key(&self) -> &str {
        self.program["key"].as_str().unwrap_or_default()
    }

    pub fn name(&self) -> &str {
        self.program["name"].as_str().unwrap_or_default()
    }

    /// The line that starts it.
    pub fn run(&self) -> Value {
        self.program["run"].clone()
    }
}

/// Every startable agent, in herdr's order, each asked for on `there` in
/// the shell its terminals start in.
pub async fn known(there: &There) -> Result<Vec<Known>, String> {
    let all = programs::startable();
    let commands: Vec<String> = all.iter().filter_map(|p| p["run"][0].as_str().map(str::to_owned)).collect();
    let argv = shell::check(&commands);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = match there.run(&argv, None, TIMEOUT).await {
        Ok(ran) => ran.out(),
        Err(e) if e.contains("cannot start sh: program not found") => {
            let names = commands.iter().map(|c| format!("'{}'", c.replace('\'', "''"))).collect::<Vec<_>>().join(",");
            let script = format!(
                "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); foreach($c in @({names})){{ if(Get-Command $c -ErrorAction SilentlyContinue){{ \"herdr-has:$c\" }} }}"
            );
            there.run(&["powershell.exe", "-NoLogo", "-Command", &script], None, TIMEOUT).await?.out()
        }
        Err(e) => return Err(e),
    };
    let has: Vec<&str> = out.split("herdr-has:").skip(1).filter_map(|s| s.split_whitespace().next()).collect();
    Ok(all
        .into_iter()
        .map(|program| {
            let installed = program["run"][0].as_str().is_some_and(|c| has.contains(&c));
            Known { program, installed }
        })
        .collect())
}

/// What a blocked agent shows: its question's last lines, and the choices
/// it offers (Codex's and Claude Code's numbered menus).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Question {
    pub lines: Vec<String>,
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    /// The key that picks it: its own one-key shortcut (`y`, `esc`), else
    /// its number.
    pub key: String,
}

/// How many of the question's lines are kept.
const ASKED: usize = 6;

/// Reads a question off a screen's text: a numbered menu (`› 1. Yes,
/// proceed (y)`) and the lines above it. A screen without one is asked in
/// its last lines.
pub fn question(screen: &str) -> Question {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let choice = |l: &str| -> Option<Choice> {
        let l = l.trim_start().trim_start_matches(['›', '❯', '>', '→']).trim_start();
        let (n, rest) = l.split_once(". ")?;
        if n.len() != 1 || !n.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let rest = rest.trim();
        let (label, key) = match rest.rsplit_once(" (") {
            Some((label, key)) if key.ends_with(')') => {
                let key = &key[..key.len() - 1];
                (label, if key.chars().count() == 1 || key == "esc" { key } else { n })
            }
            _ => (rest, n),
        };
        Some(Choice { label: label.trim().to_owned(), key: key.to_owned() })
    };
    // The last run of numbered lines (gaps of wrapped lines allowed).
    let numbered: Vec<usize> = (0..lines.len()).filter(|&i| choice(lines[i]).is_some()).collect();
    let menu = numbered.last().map(|&end| {
        let mut start = end;
        for &i in numbered.iter().rev() {
            if start - i > 3 {
                break;
            }
            start = i;
        }
        (start, end)
    });
    let (choices, above) = match menu {
        Some((start, end)) => ((start..=end).filter_map(|i| choice(lines[i])).collect(), start),
        None => (Vec::new(), lines.len()),
    };
    let lines = lines[..above]
        .iter()
        .rev()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(ASKED)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(str::to_owned)
        .collect();
    Question { lines, choices }
}

/// The question `terminal` shows now.
pub async fn asked(core: &herdc_wire::Client, terminal: &str) -> Result<Question, String> {
    let read = core.call("terminal.read", serde_json::json!({ "terminal": terminal })).await.map_err(|e| e.to_string())?;
    Ok(question(read["text"].as_str().unwrap_or_default()))
}

/// Presses `key` (a key name: `y`, `esc`, `enter`) in `terminal`.
pub async fn press(core: &herdc_wire::Client, terminal: &str, key: &str) -> Result<(), String> {
    core.call("terminal.keys", serde_json::json!({ "terminal": terminal, "keys": [key] })).await.map(drop).map_err(|e| e.to_string())
}

/// Says `words` to `terminal`, blocked or not. A menu is left with Esc
/// first (Codex's and Claude Code's "No, and tell it what to do"): Enter
/// on it would pick its highlighted answer, often a yes.
pub async fn reply(core: &herdc_wire::Client, terminal: &str, words: &str, menu: bool) -> Result<(), String> {
    if menu {
        press(core, terminal, "esc").await?;
    }
    core.call("terminal.send", serde_json::json!({ "terminal": terminal, "text": words, "force": true })).await.map(drop).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codex_approval_is_its_lines_and_choices() {
        let screen = "• Running curl -sI https://example.com | head -1\n\n  Would you like to run the following command?\n\n  Environment: local\n  Reason: Allow network access?\n\n  $ curl -sI https://example.com | head -1\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `curl -sI\n     https://example.com` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        let q = question(screen);
        let keys: Vec<_> = q.choices.iter().map(|c| (c.label.as_str(), c.key.as_str())).collect();
        assert_eq!(keys, [("Yes, proceed", "y"), ("Yes, and don't ask again for commands that start with `curl -sI", "2"), ("No, and tell Codex what to do differently", "esc")]);
        assert_eq!(q.lines.last().map(String::as_str), Some("$ curl -sI https://example.com | head -1"));
        assert_eq!(q.lines.len(), 5);
    }

    #[test]
    fn a_claude_menu_picks_by_number() {
        let q = question("Do you want to create a.txt?\n❯ 1. Yes\n  2. Yes, allow all edits during this session (shift+tab)\n  3. No, and tell Claude what to do differently (esc)\n");
        assert_eq!(q.choices[0], Choice { label: "Yes".into(), key: "1".into() });
        assert_eq!((q.choices[1].label.as_str(), q.choices[1].key.as_str()), ("Yes, allow all edits during this session", "2"));
        assert_eq!(q.choices[2].key, "esc");
        assert_eq!(q.lines, ["Do you want to create a.txt?"]);
    }

    #[test]
    fn without_a_menu_the_last_lines_are_the_question() {
        let q = question("a\nb\n\nProceed? [y/N]\n");
        assert!(q.choices.is_empty());
        assert_eq!(q.lines, ["a", "b", "Proceed? [y/N]"]);
    }
}
