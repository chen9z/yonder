//! A Pi session as a conversation. Pi keeps each session in a file of
//! its own (`~/.pi/agent/sessions/<folder>/<time>_<id>.jsonl`), one entry
//! a line, each naming the one before it: going back and compacting
//! branch it, so what is said now is the line from the last entry back.
//! The file is read as it grows (`file.read` gives lines from where it
//! left off), and told as what was said: the person's words, the
//! replies, the tools used between them. Going back to before one of
//! the person's messages is done as a person does it, by Pi's keys
//! ([`rewind`]).

use super::There;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Lines read first: the end of a long session, not all of it.
pub const TAIL: usize = 800;

/// How much of a tool's result is kept (it is shown on its own page).
const RESULT: usize = 6000;

/// How much of what a tool was given is kept (a file written, an edit).
const INPUT: usize = 20000;

/// One thing said, as the conversation shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// What the person (or another agent, through `hc send`) wrote.
    You { text: String, images: usize },
    /// The agent's words, Markdown.
    Reply(String),
    /// Tools used one after another, between words.
    Tools(Vec<Call>),
    /// What came before was summed up (Pi's compaction).
    Compacted,
    /// The model failed: why.
    Failed(String),
    /// Stopped before it was done (Esc).
    Stopped,
    /// The session itself changed, by a command (`/name`, `/model`).
    Changed(Change),
}

/// What a command changed: the session's name, the model, how hard it
/// thinks.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Name(String),
    Model(String),
    Thinking(String),
}

/// A tool used: its name, what it was given (`said` in a line, `input`
/// whole: the command, the file written, an edit as `- `/`+ ` lines when
/// `diff`), and what came back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub said: String,
    pub input: String,
    pub diff: bool,
    pub result: Option<String>,
    pub error: bool,
}

/// A piece of the conversation, keyed to stay the same row as it grows;
/// `at` when it was said (ms since the epoch, 0: not known).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub id: String,
    pub said: Said,
    pub at: u64,
}

#[derive(Clone, Debug)]
enum Kind {
    You { text: String, images: usize },
    /// Its words and tool calls in order (thinking left out); how it ended.
    Answer { parts: Vec<Part>, failed: Option<String>, stopped: bool },
    Result { call: String, text: String, error: bool },
    Shell { command: String, output: String, failed: bool },
    Compacted,
    Change(Change),
    Other,
}

#[derive(Clone, Debug)]
enum Part {
    Words(String),
    Call(Call),
}

#[derive(Clone, Debug)]
struct Entry {
    parent: Option<String>,
    kind: Kind,
    at: u64,
}

/// A session as read so far: from line `start` to line `read` of its
/// file (earlier lines can be read in later, before them).
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    entries: HashMap<String, Entry>,
    /// The entry on the last line read, and that line.
    last: Option<(usize, String)>,
    /// Lines of the file read (the next read starts after them).
    pub read: usize,
    /// The first line read (0: none yet).
    pub start: usize,
}

impl Transcript {
    /// It began before the lines read: older entries aren't known.
    pub fn cut(&self) -> bool {
        self.start > 1
    }

    /// Takes in the lines a read gave (`first`: the first one's number),
    /// after those read or before them.
    pub fn take(&mut self, read: &Read) {
        let count = read.text.lines().count();
        for (i, line) in read.text.lines().enumerate() {
            self.add(line, read.first + i);
        }
        if self.start == 0 || read.first < self.start {
            self.start = read.first.max(1);
        }
        self.read = self.read.max(read.first.saturating_sub(1) + count);
    }

    fn add(&mut self, line: &str, n: usize) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let Some(id) = v["id"].as_str() else { return };
        let parent = v["parentId"].as_str().map(str::to_owned);
        let kind = match v["type"].as_str() {
            Some("message") => message(&v["message"]),
            Some("compaction") => Kind::Compacted,
            Some("session_info") => v["name"].as_str().map_or(Kind::Other, |n| Kind::Change(Change::Name(n.to_owned()))),
            // The model without its provider's path.
            Some("model_change") => v["modelId"].as_str().map_or(Kind::Other, |m| Kind::Change(Change::Model(m.rsplit('/').next().unwrap_or(m).to_owned()))),
            Some("thinking_level_change") => v["thinkingLevel"].as_str().map_or(Kind::Other, |l| Kind::Change(Change::Thinking(l.to_owned()))),
            _ => Kind::Other,
        };
        let at = v["timestamp"].as_str().and_then(epoch_ms).unwrap_or(0);
        self.entries.insert(id.to_owned(), Entry { parent, kind, at });
        if self.last.as_ref().is_none_or(|(m, _)| n >= *m) {
            self.last = Some((n, id.to_owned()));
        }
    }

    /// What is said now: the branch the last entry is on, oldest first.
    pub fn lines(&self) -> Vec<Line> {
        let mut branch = Vec::new();
        let mut at = self.last.as_ref().map(|(_, id)| id.clone());
        while let Some(id) = at {
            let Some(entry) = self.entries.get(&id) else { break };
            branch.push((id, entry));
            if branch.len() > self.entries.len() {
                break;
            }
            at = entry.parent.clone();
        }
        branch.reverse();
        let results: HashMap<&str, (&str, bool)> = branch
            .iter()
            .filter_map(|(_, e)| match &e.kind {
                Kind::Result { call, text, error } => Some((call.as_str(), (text.as_str(), *error))),
                _ => None,
            })
            .collect();
        let mut out: Vec<Line> = Vec::new();
        // One run of tools between words, however many answers it spans.
        let tool = |out: &mut Vec<Line>, call: Call, at: u64| match out.last_mut() {
            Some(Line { said: Said::Tools(calls), .. }) => calls.push(call),
            _ => out.push(Line { id: call.id.clone(), said: Said::Tools(vec![call]), at }),
        };
        for (id, entry) in branch {
            let at = entry.at;
            match &entry.kind {
                Kind::You { text, images } => out.push(Line { id, said: Said::You { text: text.clone(), images: *images }, at }),
                Kind::Answer { parts, failed, stopped } => {
                    for (i, part) in parts.iter().enumerate() {
                        match part {
                            Part::Words(words) => out.push(Line { id: format!("{id}:{i}"), said: Said::Reply(words.clone()), at }),
                            Part::Call(call) => {
                                let result = results.get(call.id.as_str());
                                tool(&mut out, Call { result: result.map(|r| r.0.to_owned()), error: result.is_some_and(|r| r.1), ..call.clone() }, at);
                            }
                        }
                    }
                    if let Some(why) = failed {
                        out.push(Line { id: format!("{id}:failed"), said: Said::Failed(why.clone()), at });
                    } else if *stopped {
                        out.push(Line { id: format!("{id}:stopped"), said: Said::Stopped, at });
                    }
                }
                Kind::Shell { command, output, failed } => {
                    let call = Call { id: id.clone(), name: "!".into(), said: cut(command.lines().next().unwrap_or_default(), 160), input: command.clone(), result: Some(output.clone()), error: *failed, ..Call::default() };
                    tool(&mut out, call, at);
                }
                Kind::Compacted => out.push(Line { id, said: Said::Compacted, at }),
                // Once something was said: before, it is how it began.
                Kind::Change(change) if out.iter().any(|l| matches!(l.said, Said::You { .. })) => out.push(Line { id, said: Said::Changed(change.clone()), at }),
                Kind::Change(_) | Kind::Result { .. } | Kind::Other => {}
            }
        }
        out
    }
}

fn message(m: &Value) -> Kind {
    let parts = || m["content"].as_array().map(Vec::as_slice).unwrap_or_default();
    match m["role"].as_str() {
        Some("user") => {
            let text = match &m["content"] {
                Value::String(s) => s.clone(),
                _ => parts().iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
            };
            Kind::You { text: text.trim().to_owned(), images: parts().iter().filter(|p| p["type"] == "image").count() }
        }
        Some("assistant") => {
            let parts = parts()
                .iter()
                .filter_map(|p| match p["type"].as_str() {
                    Some("text") => p["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(|t| Part::Words(t.to_owned())),
                    Some("toolCall") => {
                        let name = p["name"].as_str().unwrap_or_default();
                        let (input, diff) = input(name, &p["arguments"]);
                        Some(Part::Call(Call {
                            id: p["id"].as_str().unwrap_or_default().to_owned(),
                            name: name.to_owned(),
                            said: given(name, &p["arguments"]),
                            input,
                            diff,
                            ..Call::default()
                        }))
                    }
                    _ => None,
                })
                .collect();
            let failed = (m["stopReason"] == "error").then(|| m["errorMessage"].as_str().unwrap_or_default().to_owned());
            Kind::Answer { parts, failed, stopped: m["stopReason"] == "aborted" }
        }
        Some("toolResult") => {
            let text = parts().iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n");
            Kind::Result { call: m["toolCallId"].as_str().unwrap_or_default().to_owned(), text: cut(&text, RESULT), error: m["isError"] == true }
        }
        Some("bashExecution") => Kind::Shell {
            command: m["command"].as_str().unwrap_or_default().to_owned(),
            output: cut(m["output"].as_str().unwrap_or_default(), RESULT),
            failed: m["exitCode"].as_i64().is_some_and(|c| c != 0),
        },
        _ => Kind::Other,
    }
}

/// What a tool was given, in a line: the command, the file, the search.
fn given(name: &str, args: &Value) -> String {
    let first = |keys: &[&str]| keys.iter().find_map(|k| args[*k].as_str().or_else(|| args[*k][0].as_str()));
    let said = match name {
        "bash" => first(&["command"]),
        "read" | "write" | "edit" => first(&["path", "file_path"]),
        _ => first(&["description", "command", "path", "pattern", "query", "queries", "url", "urls", "prompt"]),
    };
    let said = said.or_else(|| args.as_object()?.values().find_map(Value::as_str)).unwrap_or_default();
    cut(said.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim(), 160)
}

/// What a tool was given, whole: the command, the file and what was
/// written to it, an edit as the lines it took out (`- `) and put in
/// (`+ `: a diff, then); else its arguments as they were.
fn input(name: &str, args: &Value) -> (String, bool) {
    let s = |k: &str| args[k].as_str().unwrap_or_default();
    let (text, diff) = match name {
        "bash" => (s("command").to_owned(), false),
        "read" => (s("path").to_owned(), false),
        "write" => (format!("{}\n\n{}", s("path"), s("content")), false),
        "edit" => {
            let edits = args["edits"].as_array().cloned().unwrap_or_else(|| vec![args.clone()]);
            let mut out = s("path").to_owned();
            for e in edits {
                out.push('\n');
                for (sign, key) in [("- ", "oldText"), ("+ ", "newText")] {
                    for line in e[key].as_str().unwrap_or_default().lines() {
                        out.push_str(&format!("\n{sign}{line}"));
                    }
                }
            }
            (out, true)
        }
        _ => (serde_json::to_string_pretty(args).unwrap_or_default(), false),
    };
    (cut(&text, INPUT), diff)
}

/// An entry's time (`2026-11-05T17:59:34.063Z`), in ms since the epoch.
fn epoch_ms(iso: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| iso.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, min, s) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // Its fraction of a second, in ms: `.5` is 500.
    let fraction: String = iso.get(19..).and_then(|f| f.strip_prefix('.')).unwrap_or_default().chars().take_while(char::is_ascii_digit).take(3).collect();
    let ms = format!("{fraction:0<3}").parse::<i64>().unwrap_or(0);
    // Days since 1970-01-01 of a civil date (Howard Hinnant's).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(((days * 24 + h) * 60 + min) * 60 * 1000 + s * 1000 + ms).ok()
}

fn cut(text: &str, at: usize) -> String {
    match text.char_indices().nth(at) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Lines a read gave: from `first` (from 1), and how many the file has.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub text: String,
    pub first: usize,
    pub lines: usize,
}

/// The session file of a Pi running in `cwd`: the one its terminal's
/// label names (`alma.session`: `pi:id:<id>`; none yet, none: not
/// another), else the newest there.
pub async fn find(there: &There, label: Option<&str>, cwd: &str) -> Result<Option<String>, String> {
    let id = label.and_then(|l| l.strip_prefix("pi:id:")).filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).unwrap_or_default();
    let script = r#"d="$HOME/.pi/agent/sessions"
if [ -n "$1" ]; then for f in "$d"/*/*_"$1".jsonl; do [ -f "$f" ] && printf %s "$f"; done; exit 0; fi
ls -t "$d/$2"/*.jsonl 2>/dev/null | head -n 1 | tr -d '\n'"#;
    let ran = there.run(&["sh", "-c", script, "sh", id, &folder(cwd)], None, Duration::from_secs(15)).await?;
    Ok(Some(ran.out()).filter(|p| ran.ok() && p.ends_with(".jsonl")))
}

/// Pi's folder for the sessions started in `cwd`.
fn folder(cwd: &str) -> String {
    format!("--{}--", cwd.trim_start_matches(['/', '\\']).replace(['/', '\\', ':'], "-"))
}

/// The lines after the first `from`; the first time (`from` 0), only
/// the last [`TAIL`]. A line still being written waits for the next read.
pub async fn read(there: &There, path: &str, from: usize) -> Result<Read, String> {
    let call = |params: Value| async move {
        there.core.call_timeout("file.read", params, Duration::from_secs(30)).await.map_err(|e| e.to_string())
    };
    let on = there.on.clone();
    let mut start = from + 1;
    if from == 0 {
        let size = call(json!({ "path": path, "on": on, "offset": 1, "limit": 0 })).await?;
        start = (size["lines"].as_u64().unwrap_or(0) as usize).saturating_sub(TAIL) + 1;
    }
    let got = call(json!({ "path": path, "on": on, "offset": start })).await?;
    let mut text = got["text"].as_str().unwrap_or_default().to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.truncate(text.rfind('\n').map_or(0, |i| i + 1));
    }
    Ok(Read { text, first: start, lines: got["lines"].as_u64().unwrap_or(0) as usize })
}

/// The lines before line `before` (from 1), at most [`TAIL`] of them:
/// what came before what was read.
pub async fn earlier(there: &There, path: &str, before: usize) -> Result<Read, String> {
    let first = before.saturating_sub(TAIL).max(1);
    if before <= first {
        return Ok(Read { first, ..Read::default() });
    }
    let got = there.core.call_timeout("file.read", json!({ "path": path, "on": there.on, "offset": first, "limit": before - first }), Duration::from_secs(30)).await.map_err(|e| e.to_string())?;
    Ok(Read { text: got["text"].as_str().unwrap_or_default().to_owned(), first, lines: got["lines"].as_u64().unwrap_or(0) as usize })
}

/// Stops the agent at `terminal` (Esc, as the core's interrupt presses
/// it): what came of it (`stopped`, `still_working`, ...).
pub async fn interrupt(there: &There, terminal: &str) -> Result<String, String> {
    let got = there.core.call_timeout("terminal.interrupt", json!({ "terminal": terminal, "timeout": 5 }), Duration::from_secs(20)).await.map_err(|e| e.to_string())?;
    Ok(got["outcome"].as_str().unwrap_or_default().to_owned())
}

/// Puts a file (a picture) beside the agent at `terminal`, on its
/// machine: where it landed, for the message to say.
pub async fn put(there: &There, terminal: &str, name: &str, bytes: &[u8]) -> Result<String, String> {
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let got = there.core.call_timeout("terminal.put", json!({ "terminal": terminal, "name": name, "data": data }), Duration::from_secs(120)).await.map_err(|e| e.to_string())?;
    got["path"].as_str().map(str::to_owned).ok_or_else(|| "terminal.put: no path".to_owned())
}

/// Says `text` to the agent at `terminal` as `hc send` does: pasted,
/// then Enter, once nobody is typing to it.
pub async fn send(there: &There, terminal: &str, text: &str) -> Result<(), String> {
    there.core.call_timeout("terminal.send", json!({ "terminal": terminal, "text": text }), Duration::from_secs(90)).await.map(|_| ()).map_err(|e| match e {
        herdc_wire::Error::Rpc { kind: Some(kind), .. } if kind == "blocked" => "blocked".to_owned(),
        other => other.to_string(),
    })
}

/// How long Pi is given to show what a key did.
const SHOWN: Duration = Duration::from_secs(4);

/// Pi takes a second Ctrl+C within this as "quit".
const TWICE: Duration = Duration::from_millis(500);

/// Why going back didn't happen: nothing changed in Pi, or it went back
/// and the words weren't sent after (its field emptied again).
#[derive(Clone, Debug, PartialEq)]
pub enum Rewound {
    Unchanged(String),
    Moved(String),
}

/// Goes back in the session of the Pi at `terminal` to just before one
/// of the person's messages, `said`, with `later` of theirs after it on
/// the branch shown. Done by its keys as a person would: its field
/// emptied (and seen empty: nothing else open), `/tree`, its own messages
/// only, up to that one, no summary; the screen read at each step, and
/// nothing chosen unless it shows the message. Its field is left empty
/// for the words written instead, whatever came of it.
pub async fn rewind(there: &There, terminal: &str, said: &str, later: usize) -> Result<(), Rewound> {
    let unchanged = |why: String| Rewound::Unchanged(why);
    let emptied = Instant::now();
    keys(there, terminal, &["ctrl+c"]).await.map_err(unchanged)?;
    let free = |s: &str| tree(s).is_none() && !s.contains("Summarize branch?") && editor(s).is_some_and(|e| e.is_empty());
    until(there, terminal, free).await.map_err(unchanged)?.ok_or_else(|| unchanged("Something is open in its terminal; nothing changed.".into()))?;
    keys(there, terminal, &["/", "t", "r", "e", "e", "enter"]).await.map_err(unchanged)?;
    let went = choose(there, terminal, said, later).await;
    let moved = match went {
        Ok(()) => None,
        Err((entered, why)) => {
            // Out of the tree (and its question): nothing chosen, then.
            let mut out = false;
            for _ in 0..3 {
                match screen(there, terminal).await {
                    Ok(s) if tree(&s).is_some() || s.contains("Summarize branch?") => {
                        keys(there, terminal, &["esc"]).await.map_err(unchanged)?;
                        out = true;
                    }
                    _ => break,
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            if !entered || out {
                return Err(Rewound::Unchanged(why));
            }
            Some(why)
        }
    };
    // The message, put back in its field: out again, for the new words.
    tokio::time::sleep((TWICE + Duration::from_millis(200)).saturating_sub(emptied.elapsed())).await;
    let cleared = async {
        keys(there, terminal, &["ctrl+c"]).await?;
        until(there, terminal, |s| editor(s).is_some_and(|e| e.is_empty())).await?.ok_or_else(|| "Its field didn't empty.".to_owned())
    };
    match (moved, cleared.await) {
        (None, Ok(_)) => Ok(()),
        (Some(why), _) | (None, Err(why)) => Err(Rewound::Moved(why)),
    }
}

/// In the tree: the person's messages only, up to `said`, chosen,
/// without a summary of what is left. Failing, whether its Enter went.
async fn choose(there: &There, terminal: &str, said: &str, later: usize) -> Result<(), (bool, String)> {
    let before = |why: String| (false, why);
    let after = |why: String| (true, why);
    until(there, terminal, |s| tree(s).is_some()).await.map_err(before)?.ok_or_else(|| before("Its session tree didn't open.".into()))?;
    let only = |s: &str| tree(s).is_some_and(|c| c.contains("[user]"));
    // Ctrl+U turns "theirs only" on and off: on, once.
    if !screen(there, terminal).await.is_ok_and(|s| only(&s)) {
        keys(there, terminal, &["ctrl+u"]).await.map_err(before)?;
        if until(there, terminal, only).await.map_err(before)?.is_none() {
            keys(there, terminal, &["ctrl+u"]).await.map_err(before)?;
            until(there, terminal, only).await.map_err(before)?.ok_or_else(|| before("Its tree didn't show only your messages.".into()))?;
        }
    }
    if later > 0 {
        keys(there, terminal, &vec!["up"; later]).await.map_err(before)?;
    }
    until(there, terminal, |s| chosen(s).is_some_and(|r| is(r, said))).await.map_err(before)?.ok_or_else(|| before("That message isn't where it was in its tree; nothing changed.".into()))?;
    keys(there, terminal, &["enter"]).await.map_err(after)?;
    // Back there, the message in its field (which was emptied for it); a
    // long one shows its end there: Pi's word for it, then.
    let back = |s: &str| tree(s).is_none() && !s.contains("Summarize branch?") && editor(s).is_some_and(|e| starts(&e, said));
    let asked = until(there, terminal, |s| s.contains("Summarize branch?") || back(s)).await.map_err(after)?.ok_or_else(|| after("It didn't go back.".into()))?;
    if asked.contains("Summarize branch?") {
        if !asked.lines().any(|l| l.contains("→ No summary")) {
            return Err(after("It asked about a summary in a way not known.".into()));
        }
        keys(there, terminal, &["enter"]).await.map_err(after)?;
        let done = |s: &str| back(s) || (tree(s).is_none() && !s.contains("Summarize branch?") && s.contains("Navigated to selected point"));
        until(there, terminal, done).await.map_err(after)?.ok_or_else(|| after("It didn't go back.".into()))?;
    }
    Ok(())
}

async fn keys(there: &There, terminal: &str, keys: &[&str]) -> Result<(), String> {
    there.core.call_timeout("terminal.keys", json!({ "terminal": terminal, "keys": keys }), Duration::from_secs(30)).await.map(|_| ()).map_err(|e| e.to_string())
}

async fn screen(there: &There, terminal: &str) -> Result<String, String> {
    let got = there.core.call_timeout("terminal.read", json!({ "terminal": terminal }), Duration::from_secs(15)).await.map_err(|e| e.to_string())?;
    Ok(got["text"].as_str().unwrap_or_default().to_owned())
}

/// The screen once it shows `what`, read again until [`SHOWN`] passed.
async fn until(there: &There, terminal: &str, what: impl Fn(&str) -> bool) -> Result<Option<String>, String> {
    let end = Instant::now() + SHOWN;
    loop {
        let s = screen(there, terminal).await?;
        if what(&s) {
            return Ok(Some(s));
        }
        if Instant::now() > end {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Pi's tree, open: what its count (`(3/4) [user]`) says after itself.
fn tree(screen: &str) -> Option<&str> {
    let lines: Vec<&str> = screen.lines().collect();
    count_at(&lines).and_then(|i| counted(lines[i]))
}

/// Where the tree's count is: a `(i/n)` line with a rule next under it,
/// its search above (not words of the conversation that look so).
fn count_at(lines: &[&str]) -> Option<usize> {
    let ruled = |i: usize| lines[i + 1..].iter().find(|l| !l.trim().is_empty()).is_some_and(|l| rule(l));
    let at = (0..lines.len()).rev().find(|&i| counted(lines[i]).is_some() && ruled(i))?;
    lines[..at].iter().any(|l| l.contains("Type to search")).then_some(at)
}

/// A line's count (`(3/4) [user]`): what it says after itself.
fn counted(line: &str) -> Option<&str> {
    let (count, rest) = line.trim_start().strip_prefix('(')?.split_once(')')?;
    let (at, of) = count.split_once('/')?;
    (!at.is_empty() && !of.is_empty() && count.chars().all(|c| c.is_ascii_digit() || c == '/')).then(|| rest.trim())
}

/// A rule across the screen.
fn rule(line: &str) -> bool {
    line.chars().count() > 3 && line.chars().all(|c| c == '─')
}

/// The row of the tree chosen (`› `), above its count.
fn chosen(screen: &str) -> Option<&str> {
    let lines: Vec<&str> = screen.lines().collect();
    let count = count_at(&lines)?;
    lines[..count].iter().rev().find(|l| l.starts_with('›')).copied()
}

/// Whether a row of the tree is the person's message `said`, on the
/// branch shown (`•`): as much of it as the row shows.
fn is(row: &str, said: &str) -> bool {
    row.split_once("user: ").is_some_and(|(before, shown)| before.contains('•') && starts(shown.trim_end_matches('…'), said))
}

/// Whether `shown` is `said`, spaces and lines aside (a field wraps
/// words, Chinese anywhere), or how it starts (cut where the screen
/// ends): at least its first few letters, and not longer.
fn starts(shown: &str, said: &str) -> bool {
    let flat = |s: &str| s.split_whitespace().collect::<String>();
    let (shown, said) = (flat(shown), flat(said));
    let (n, of) = (shown.chars().count(), said.chars().count());
    n > 0 && n <= of && n >= of.min(6) && said.starts_with(&shown)
}

/// What Pi's field holds: the lines between the last two rules across
/// the screen.
fn editor(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let last = lines.iter().rposition(|l| rule(l))?;
    let first = lines[..last].iter().rposition(|l| rule(l))?;
    Some(lines[first + 1..last].iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n").trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[Value], first: usize) -> Read {
        Read { text: lines.iter().map(|l| format!("{l}\n")).collect(), first, lines: first + lines.len() - 1 }
    }

    #[test]
    fn a_session_is_told_as_words_and_the_tools_between_them() {
        let lines = [
            json!({"type": "session", "id": "s"}),
            json!({"type": "message", "id": "1", "parentId": null, "message": {"role": "user", "content": [{"type": "text", "text": "check the disk"}, {"type": "image"}]}}),
            json!({"type": "message", "id": "2", "parentId": "1", "message": {"role": "assistant", "stopReason": "toolUse", "content": [
                {"type": "thinking", "thinking": "hm"}, {"type": "text", "text": "Looking."},
                {"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"command": "df -h\nexit"}}]}}),
            json!({"type": "message", "id": "3", "parentId": "2", "message": {"role": "toolResult", "toolCallId": "c1", "content": [{"type": "text", "text": "90% used"}]}}),
            json!({"type": "custom", "id": "4", "parentId": "3"}),
            json!({"type": "message", "id": "5", "parentId": "4", "message": {"role": "assistant", "content": [
                {"type": "toolCall", "id": "c2", "name": "read", "arguments": {"path": "/etc/fstab"}}]}}),
            json!({"type": "message", "id": "6", "parentId": "5", "message": {"role": "toolResult", "toolCallId": "c2", "isError": true, "content": [{"type": "text", "text": "denied"}]}}),
            json!({"type": "message", "id": "7", "parentId": "6", "message": {"role": "assistant", "stopReason": "stop", "content": [{"type": "text", "text": "**90%** used."}]}}),
        ];
        let mut t = Transcript::default();
        t.take(&read(&lines, 1));
        assert_eq!(t.read, 8);
        assert!(!t.cut());
        let said: Vec<Said> = t.lines().into_iter().map(|l| l.said).collect();
        assert_eq!(said, vec![
            Said::You { text: "check the disk".into(), images: 1 },
            Said::Reply("Looking.".into()),
            Said::Tools(vec![
                Call { id: "c1".into(), name: "bash".into(), said: "df -h".into(), input: "df -h\nexit".into(), result: Some("90% used".into()), ..Call::default() },
                Call { id: "c2".into(), name: "read".into(), said: "/etc/fstab".into(), input: "/etc/fstab".into(), result: Some("denied".into()), error: true, ..Call::default() },
            ]),
            Said::Reply("**90%** used.".into()),
        ]);
    }

    #[test]
    fn what_is_said_now_is_the_branch_the_last_entry_is_on_read_as_it_grows() {
        let mut t = Transcript::default();
        // Read from line 10: what came before isn't known.
        t.take(&read(&[
            json!({"type": "message", "id": "a", "parentId": "x", "message": {"role": "user", "content": "first"}}),
            json!({"type": "message", "id": "b", "parentId": "a", "message": {"role": "assistant", "stopReason": "error", "errorMessage": "502", "content": []}}),
        ], 10));
        assert!(t.cut());
        assert_eq!(t.read, 11);
        // Gone back to `a`'s parent: a new branch; then compacted, stopped.
        t.take(&read(&[
            json!({"type": "message", "id": "c", "parentId": "x", "message": {"role": "user", "content": "again"}}),
            json!({"type": "compaction", "id": "d", "parentId": "c", "summary": "…"}),
            json!({"type": "message", "id": "e", "parentId": "d", "message": {"role": "assistant", "stopReason": "aborted", "content": [{"type": "thinking"}]}}),
        ], 12));
        assert_eq!(t.read, 14);
        let said: Vec<Said> = t.lines().into_iter().map(|l| l.said).collect();
        assert_eq!(said, vec![Said::You { text: "again".into(), images: 0 }, Said::Compacted, Said::Stopped]);
        t.take(&read(&[json!({"type": "message", "id": "f", "parentId": "a", "message": {"role": "assistant", "stopReason": "error", "errorMessage": "502", "content": []}})], 15));
        let said: Vec<Said> = t.lines().into_iter().map(|l| l.said).collect();
        assert_eq!(said, vec![Said::You { text: "first".into(), images: 0 }, Said::Failed("502".into())]);
    }

    #[test]
    fn earlier_lines_read_in_later_go_before_and_the_last_stays_last() {
        let user = |id: &str, parent: &str, text: &str| json!({"type": "message", "id": id, "parentId": parent, "timestamp": "2026-11-05T17:59:34.063Z", "message": {"role": "user", "content": text}});
        let mut t = Transcript::default();
        t.take(&read(&[user("c", "b", "three")], 3));
        assert!(t.cut());
        t.take(&read(&[user("a", "", "one"), user("b", "a", "two")], 1));
        assert!(!t.cut());
        assert_eq!(t.read, 3);
        let said: Vec<String> = t.lines().into_iter().map(|l| match l.said { Said::You { text, .. } => text, _ => String::new() }).collect();
        assert_eq!(said, ["one", "two", "three"]);
        assert_eq!(t.lines()[0].at, 1_793_901_574_063);
    }

    #[test]
    fn an_edit_is_given_as_the_lines_it_took_out_and_put_in() {
        let (input, diff) = input("edit", &json!({"path": "a.md", "edits": [{"oldText": "x\ny", "newText": "z"}]}));
        assert!(diff);
        assert_eq!(input, "a.md\n\n- x\n- y\n+ z");
        assert_eq!(input_of("bash", json!({"command": "ls\npwd", "timeout": 5})), "ls\npwd");
        assert_eq!(epoch_ms("1970-01-02T00:00:01.5Z"), Some(86_401_500));
    }

    fn input_of(name: &str, args: Value) -> String {
        input(name, &args).0
    }

    #[test]
    fn what_a_command_changed_shows_once_something_was_said() {
        let lines = [
            json!({"type": "model_change", "id": "m", "parentId": null, "modelId": "remote/claude/claude-opus-5-5"}),
            json!({"type": "message", "id": "1", "parentId": "m", "message": {"role": "user", "content": "hi"}}),
            json!({"type": "session_info", "id": "2", "parentId": "1", "name": "探针"}),
            json!({"type": "thinking_level_change", "id": "3", "parentId": "2", "thinkingLevel": "low"}),
            json!({"type": "model_change", "id": "4", "parentId": "3", "modelId": "gpt-6"}),
        ];
        let mut t = Transcript::default();
        t.take(&read(&lines, 1));
        let said: Vec<Said> = t.lines().into_iter().map(|l| l.said).collect();
        assert_eq!(said, vec![
            Said::You { text: "hi".into(), images: 0 },
            Said::Changed(Change::Name("探针".into())),
            Said::Changed(Change::Thinking("low".into())),
            Said::Changed(Change::Model("gpt-6".into())),
        ]);
    }

    #[test]
    fn pi_s_tree_and_field_are_read_off_its_screen() {
        // As Pi 1.5 drew them: its tree, only the person's messages.
        let rule = "─".repeat(40);
        let open = format!("  Type to search:\n{rule}\n  • user: alpha one\n  ├⊟ • user: beta two\n› │     • user: gamma three\n  └─ user: beta two\n  (3/4) [user]\n{rule}\n/tmp/p\n0.0%/400k (auto)      (magpie) claude-opus-5-5 • high\n");
        assert_eq!(tree(&open), Some("[user]"));
        assert_eq!(counted("  (6/8)"), Some(""));
        assert_eq!(counted("(magpie) x"), None);
        assert_eq!(tree("(magpie) x\n 0.0%/400k (auto)\n"), None);
        // Words of the conversation shaped like its count, over the field:
        // not the tree (no search above, or no rule under).
        assert_eq!(tree(&format!(" done (1/2)\n (1/2)\n{rule}\n\n{rule}\n")), None);
        assert_eq!(tree(&format!("  Type to search:\n{rule}\n› • user: a\n  (1/1)\n\n{rule}\n")), Some(""));
        assert_eq!(tree(&format!(" Type to search the docs\n (1/2)\n reply\n{rule}\n\n{rule}\n")), None);
        let row = chosen(&open).unwrap();
        assert_eq!(row, "› │     • user: gamma three");
        assert!(is(row, "gamma\nthree"));
        assert!(!is(row, "gamma"));
        assert!(is("› ├⊟ • user: a long message cut wh…", "a long message cut where the screen ends"));
        assert!(!is("› └─ user: beta two", "beta two"), "not on the branch shown");
        // Gone back: the message in its field, between the last two rules.
        let back = format!(" alpha one\n reply one\n Navigated to selected point\n{rule}\nbeta two\n{rule}\n/tmp/p\n");
        assert_eq!(editor(&back).as_deref(), Some("beta two"));
        assert!(starts(&editor(&back).unwrap(), "beta two"));
        // A field wraps where it must: Chinese anywhere.
        assert!(starts("把这个改成\n中文的", "把这个改成中文的说明"));
        assert!(!starts("把这个改成中文的说明吧", "把这个改成中文的说明"));
        assert_eq!(editor(&format!("{rule}\n\n{rule}\n")).as_deref(), Some(""));
    }

    #[test]
    fn pi_keeps_a_folder_s_sessions_under_its_path_with_dashes() {
        assert_eq!(folder("/Users/me/workspace/nova"), "--Users-me-workspace-nova--");
        assert_eq!(folder("/Users/me/.config/x"), "--Users-me-.config-x--");
        assert_eq!(given("web_search", &json!({"queries": ["a b", "c"]})), "a b");
        assert_eq!(given("mystery", &json!({"n": 1, "what": "this"})), "this");
    }
}
