//! The machine-add mod's work: the hosts in the core machine's
//! `~/.ssh/config` to pick from, adding one (`machine.add`), and the Mac
//! check after.
//!
//! Adapted from herdr-app `defaults/mods/machine-add/main.js`, Apache-2.0.

use super::{There, TIMEOUT};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// A named `Host` of an ssh config.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    pub alias: String,
    pub aliases: Vec<String>,
    pub hostname: Option<String>,
    pub user: Option<String>,
}

/// Host entries people can pick from the core machine's `~/.ssh/config`:
/// named ones, not patterns, from the file and the files it includes (ssh
/// stops 16 deep).
pub async fn hosts(there: &There, home: &str) -> Vec<Host> {
    let text = there.read_text(&format!("{home}/.ssh/config")).await.ok().flatten();
    let mut hosts = Vec::new();
    parse(there, home, text.unwrap_or_default(), &mut hosts, 0).await;
    hosts
}

fn parse<'a>(there: &'a There, home: &'a str, text: String, hosts: &'a mut Vec<Host>, depth: usize) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        // Index of the host the lines now set, if any.
        let mut current: Option<usize> = None;
        for raw in text.lines() {
            let Some((key, value)) = line(raw) else { continue };
            match key.as_str() {
                "host" | "match" => {
                    let names: Vec<String> = if key == "host" {
                        value
                            .split_whitespace()
                            .filter(|h| !h.contains(['*', '?', '!']) && !hosts.iter().any(|x| x.alias == *h || x.aliases.iter().any(|a| a == h)))
                            .map(str::to_owned)
                            .collect()
                    } else {
                        vec![]
                    };
                    current = names.split_first().map(|(alias, rest)| {
                        hosts.push(Host { alias: alias.clone(), aliases: rest.to_vec(), ..Host::default() });
                        hosts.len() - 1
                    });
                }
                "include" if depth < 16 => {
                    for file in include_files(there, home, &value).await {
                        let text = there.read_text(&file).await.ok().flatten();
                        parse(there, home, text.unwrap_or_default(), hosts, depth + 1).await;
                    }
                }
                "hostname" => current.into_iter().for_each(|i| hosts[i].hostname = Some(value.clone())),
                "user" => current.into_iter().for_each(|i| hosts[i].user = Some(value.clone())),
                _ => {}
            }
        }
    })
}

/// `Key value` or `Key=value`, the key lower-cased.
fn line(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    let end = raw.find(|c: char| c.is_whitespace() || c == '#' || c == '=').unwrap_or(raw.len());
    if end == 0 || raw[end..].starts_with('#') {
        return None;
    }
    let rest = raw[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    Some((raw[..end].to_lowercase(), rest.trim().to_owned()))
}

/// An Include's files, as ssh finds them: `~` is home, a relative path is
/// under `~/.ssh`, and a pattern matches files (through sh; none where
/// there's no sh).
async fn include_files(there: &There, home: &str, value: &str) -> Vec<String> {
    let mut files = Vec::new();
    for p in words(value) {
        let p = if let Some(rest) = p.strip_prefix("~/") {
            format!("{home}/{rest}")
        } else if p.starts_with('/') || drive(&p) {
            p
        } else {
            format!("{home}/.ssh/{p}")
        };
        if !p.contains(['*', '?', '[']) {
            files.push(p);
            continue;
        }
        let script = r#"for f in $1; do [ -f "$f" ] && printf "%s\n" "$f"; done"#;
        if let Ok(r) = there.run(&["sh", "-c", script, "sh", &p], None, TIMEOUT).await {
            files.extend(r.out().lines().filter(|l| !l.is_empty()).map(str::to_owned));
        }
    }
    files
}

/// Words, a quoted one kept whole.
fn words(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        let (word, after) = match rest.strip_prefix('"').and_then(|r| r.split_once('"')) {
            Some((quoted, after)) => (quoted, after),
            None => rest.split_once(char::is_whitespace).unwrap_or((rest, "")),
        };
        out.push(word.to_owned());
        rest = after.trim_start();
    }
    out
}

fn drive(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

/// The machines the core added, short-lived ones left out.
pub async fn added(there: &There) -> Vec<Value> {
    let list = there.core.call("machine.list", json!({})).await.unwrap_or_default();
    let all = list["machines"].as_array().or(list.as_array()).cloned().unwrap_or_default();
    all.into_iter().filter(|m| m["temporary"] != true).collect()
}

/// Whether `target` names one of `added`.
pub fn is_added(added: &[Value], target: &str) -> bool {
    added.iter().any(|m| ["target", "name", "host"].iter().any(|k| m[*k] == target))
}

/// What adding a machine came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Added {
    Yes,
    /// It needs a person to answer something there first: what to run.
    NeedsYou(String),
    No(String),
}

/// Adds `target` (`machine.add`): how ssh reaches it, as `vps`,
/// `can@10.0.0.7` or a Host of the ssh config.
pub async fn add(there: &There, target: &str) -> Added {
    let r = there.core.call_timeout("machine.add", json!({ "target": target }), Duration::from_secs(120)).await;
    let Err(e) = r else { return Added::Yes };
    let message = match &e {
        herdc_wire::Error::Rpc { message, .. } => message.clone(),
        other => other.to_string(),
    };
    let asks = ["password", "passphrase", "host key", "permission denied", "login"];
    let kinds = ["hc_missing", "incompatible", "login_needed", "needs_login", "auth_failed", "host_key"];
    if e.kind().is_some_and(|k| kinds.contains(&k)) || asks.iter().any(|a| message.to_lowercase().contains(a)) {
        let plain = target.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
        let quoted = if plain { target.to_owned() } else { format!("'{}'", target.replace('\'', r"'\''")) };
        Added::NeedsYou(format!(
            "Couldn't add {target} here: it needs you to answer something (a password, a new host key, or installing herdr-core there). Run `hc machine add {quoted}` in a terminal. `hc machine --help` shows the options."
        ))
    } else {
        Added::No(if message.is_empty() { "it didn't work".into() } else { message })
    }
}

/// An added Mac asks on its own screen before anything reads Desktop,
/// Documents or Downloads: Some(where its herdr-core is) for a Mac.
pub async fn mac_core(there: &There, name: &str) -> Option<String> {
    let mac = There::new(there.core.clone(), Some(name.to_owned()));
    let r = mac.run(&["sh", "-c", r#"uname; command -v herdr-core || echo "$HOME/.local/bin/herdr-core""#], None, TIMEOUT).await.ok()?;
    let out = r.out();
    let mut lines = out.trim().lines();
    (lines.next() == Some("Darwin")).then(|| lines.next().unwrap_or("~/.local/bin/herdr-core").to_owned())
}

/// Opens the Full Disk Access setting on that Mac.
pub async fn open_mac_setting(there: &There, name: &str) -> Result<(), String> {
    let mac = There::new(there.core.clone(), Some(name.to_owned()));
    mac.run(&["open", "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"], None, TIMEOUT).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_words_read_as_ssh_reads_them() {
        assert_eq!(line("  HostName = 10.0.0.7"), Some(("hostname".into(), "10.0.0.7".into())));
        assert_eq!(line("User\tcan"), Some(("user".into(), "can".into())));
        assert_eq!(line("# a comment"), None);
        assert_eq!(words(r#"a "b c" d"#), ["a", "b c", "d"]);
        assert!(drive("C:/x") && !drive("/x"));
    }

    #[test]
    fn added_is_by_target_name_or_host() {
        let added = [json!({"name": "vps", "target": "can@vps", "host": "vps.lan"})];
        assert!(is_added(&added, "can@vps") && is_added(&added, "vps.lan") && !is_added(&added, "other"));
    }
}
