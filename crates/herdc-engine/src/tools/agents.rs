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
