//! Whether a core can serve this client: the same rule Herdr applies
//! (herdr-app `src/hc.rs`, `check`), with the methods herdc calls.

use serde_json::Value;

/// The core API generation herdc speaks.
pub const GENERATION: u64 = 1;
/// The oldest core herdc works with (herdr-app `distribution/core-version`).
pub const CORE_MIN: &str = "0.1.11";
/// What herdc calls on the core.
pub const USES: &[&str] = &[
    "terminal.run",
    "terminal.list",
    "terminal.get",
    "terminal.kill",
    "terminal.label",
    "terminal.attach",
    "terminal.keep",
    "doc.get",
    "doc.set",
    "events.subscribe",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// Older than herdc needs: update herdr-core on the computer.
    TooOld { have: String, need: String },
    /// A newer generation than herdc speaks: update herdc.
    Newer { generation: u64 },
    /// It lacks methods herdc calls (an unusual build): update the core.
    Missing(Vec<String>),
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mismatch::TooOld { have, need } => write!(f, "herdr-core {have} is too old: herdc needs {need} or newer"),
            Mismatch::Newer { generation } => write!(f, "this herdr-core speaks generation {generation}: update herdc"),
            Mismatch::Missing(m) => write!(f, "this herdr-core lacks {}", m.join(", ")),
        }
    }
}

fn semver(v: &str) -> (u64, u64, u64) {
    let mut p = v.trim().trim_start_matches("hc ").split(['.', '-']).map(|n| n.parse::<u64>().unwrap_or(0));
    (p.next().unwrap_or(0), p.next().unwrap_or(0), p.next().unwrap_or(0))
}

pub fn check(hello: &Value) -> Result<(), Mismatch> {
    let generation = hello.get("generation").and_then(Value::as_u64).unwrap_or(0);
    if generation > GENERATION {
        return Err(Mismatch::Newer { generation });
    }
    let have = hello.get("version").and_then(Value::as_str).unwrap_or("0.0.0").to_owned();
    if generation < GENERATION || semver(&have) < semver(CORE_MIN) {
        return Err(Mismatch::TooOld { have, need: CORE_MIN.to_owned() });
    }
    let methods: Vec<&str> = hello.get("methods").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let missing: Vec<String> = USES.iter().filter(|m| !methods.contains(m)).map(|m| m.to_string()).collect();
    if missing.is_empty() { Ok(()) } else { Err(Mismatch::Missing(missing)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hello(generation: u64, version: &str, methods: &[&str]) -> Value {
        json!({ "generation": generation, "version": version, "methods": methods })
    }

    #[test]
    fn a_core_serves_by_generation_version_and_methods() {
        assert_eq!(check(&hello(1, "0.1.18", USES)), Ok(()));
        assert_eq!(check(&hello(2, "9.9.9", USES)), Err(Mismatch::Newer { generation: 2 }));
        assert!(matches!(check(&hello(1, "0.1.10", USES)), Err(Mismatch::TooOld { .. })));
        assert_eq!(check(&hello(1, "0.1.18", &USES[1..])), Err(Mismatch::Missing(vec!["terminal.run".into()])));
    }
}
