//! What a screen draws: the world as one value, published whenever it
//! changes. The same facts herdr-app gives its mods (`spaces()`, `panes()`,
//! `agents()`, `terminals()`, `focused()`, `machines()`), typed.

use herdc_model::layout::Node;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub link: Link,
    /// The core machine's home folder (`~` in names).
    pub home: String,
    /// The core's own machine first (`name: None`), then those it added.
    pub machines: Vec<Machine>,
    pub spaces: Vec<SpaceView>,
    /// Agents in panes, in model order.
    pub agents: Vec<AgentView>,
    /// Every live terminal's record, as the core lists them (sub-agents,
    /// terminals without a pane, other apps' too), with `status` its word,
    /// `program` how it shows and `report` the agent's own.
    pub terminals: Vec<Value>,
    pub focused: Option<Focused>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Link {
    #[default]
    Connecting,
    Connected {
        /// What the core calls its machine.
        core: String,
        version: String,
    },
    /// Trying again soon.
    Lost { error: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    /// None: the core's own machine.
    pub name: Option<String>,
    /// connected, connecting, needs_login, needs_update or unreachable.
    pub state: String,
    /// The layout document: ready, loading or unavailable.
    #[serde(deserialize_with = "word::one")]
    pub layout: Word,
    pub error: Option<String>,
    /// What that machine calls itself.
    pub host: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpaceView {
    pub id: String,
    pub index: usize,
    /// What to call it: its own name, else its first terminal's folder.
    pub name: String,
    /// Someone gave it its name.
    pub named: bool,
    pub folder: String,
    pub machine: Option<String>,
    /// The branch of the terminal it shows first.
    pub branch: Option<String>,
    pub active: bool,
    /// The most urgent agent status in it.
    #[serde(deserialize_with = "word::maybe")]
    pub status: Option<Word>,
    pub tabs: Vec<TabView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabView {
    pub id: String,
    pub index: usize,
    pub name: String,
    pub named: bool,
    pub active: bool,
    #[serde(deserialize_with = "word::maybe")]
    pub status: Option<Word>,
    /// Put away on this screen (still running).
    pub settled: bool,
    pub settled_at: Option<u64>,
    pub changed_at: Option<u64>,
    pub working_since: Option<u64>,
    pub focused: Option<String>,
    pub zoomed: Option<String>,
    /// Its split tree, as the layout document has it.
    pub root: Node,
    pub panes: Vec<PaneView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneView {
    pub pane: String,
    pub terminal: Option<String>,
    /// A mod pane: the mod it shows.
    #[serde(rename = "mod")]
    pub mod_name: Option<String>,
    pub props: Value,
    pub title: String,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    #[serde(deserialize_with = "word::maybe")]
    pub status: Option<Word>,
    pub app: Option<String>,
    pub program: Value,
    pub focused: bool,
    pub zoomed: bool,
    /// Another terminal shown in its place for now (`terminal.show`).
    pub shown: Option<String>,
    /// Its terminal runs (not just ended, not lost with its machine).
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    pub address: String,
    pub pane: String,
    pub space: String,
    pub space_name: String,
    pub machine: Option<String>,
    pub tab: String,
    pub tab_index: usize,
    pub tab_name: String,
    #[serde(deserialize_with = "word::one")]
    pub status: Word,
    pub app: Option<String>,
    pub program: Value,
    pub title: String,
    /// Bumped on every status change: newest first sorts by it.
    pub seq: u64,
    /// When the status last changed, ms since the epoch: the core's
    /// `status_ms`, not when this phone first saw it (0: unknown).
    #[serde(default)]
    pub since: u64,
    pub pinned: bool,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Focused {
    pub space: String,
    pub tab: String,
    pub pane: Option<String>,
    pub terminal: Option<String>,
    pub shown: Option<String>,
    /// The agent an agents page shows.
    pub agent: Option<String>,
    pub cwd: Option<String>,
}

/// Something to tell the person: a toast, and for agents a sound.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    pub title: String,
    pub body: Option<String>,
    /// info or error.
    pub level: &'static str,
    /// What a tap on it runs.
    pub action: Option<Action>,
    /// An agent needs the person (`blocked`) or finished (`done`).
    pub alert: Option<&'static str>,
    /// Show it: false when the person is looking at that agent already.
    pub toast: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Action {
    pub name: String,
    pub args: Value,
}

impl Note {
    pub fn info(title: impl Into<String>, body: impl Into<String>) -> Note {
        Note { title: title.into(), body: Some(body.into()), level: "info", action: None, alert: None, toast: true }
    }

    pub fn error(title: impl Into<String>, body: impl Into<String>) -> Note {
        Note { level: "error", ..Note::info(title, body) }
    }
}

impl View {
    pub fn space(&self, id: &str) -> Option<&SpaceView> {
        self.spaces.iter().find(|s| s.id == id)
    }

    pub fn active_space(&self) -> Option<&SpaceView> {
        self.spaces.iter().find(|s| s.active)
    }

    pub fn active_tab(&self) -> Option<&TabView> {
        self.active_space()?.tabs.iter().find(|t| t.active)
    }

    pub fn tab(&self, id: &str) -> Option<&TabView> {
        self.spaces.iter().flat_map(|s| &s.tabs).find(|t| t.id == id)
    }

    pub fn pane(&self, id: &str) -> Option<&PaneView> {
        self.spaces.iter().flat_map(|s| &s.tabs).flat_map(|t| &t.panes).find(|p| p.pane == id)
    }

    pub fn terminal(&self, address: &str) -> Option<&Value> {
        self.terminals.iter().find(|t| t["address"] == address)
    }

    pub fn connected(&self) -> bool {
        matches!(self.link, Link::Connected { .. })
    }
}

/// A word of a few (a status): as itself, not borrowed from what a view
/// is read from (serde takes a `&str` field to be).
pub type Word = &'static str;

/// The words a view keeps as `&'static str` (statuses, a layout's
/// state), read back (a view kept from before): each word once, so the
/// few there are cost nothing more however often they are read.
mod word {
    use serde::{Deserialize, Deserializer};
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn kept(word: String) -> &'static str {
        static WORDS: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
        let mut words = WORDS.lock().unwrap_or_else(|e| e.into_inner());
        let words = words.get_or_insert_with(HashSet::new);
        match words.get(word.as_str()) {
            Some(known) => known,
            None => {
                let known: &'static str = Box::leak(word.into_boxed_str());
                words.insert(known);
                known
            }
        }
    }

    pub fn one<'de, D: Deserializer<'de>>(from: D) -> Result<&'static str, D::Error> {
        Ok(kept(String::deserialize(from)?))
    }

    pub fn maybe<'de, D: Deserializer<'de>>(from: D) -> Result<Option<&'static str>, D::Error> {
        Ok(Option::<String>::deserialize(from)?.map(kept))
    }
}

/// A title as panes show it: without the spinner agents animate in front
/// (a spinning title would change every listing).
///
/// From herdr-app `src/terminal.rs`, Apache-2.0.
pub fn shown_title(title: &str) -> &str {
    let spinner = |c: char| {
        matches!(c, '\u{2800}'..='\u{28ff}' | '\u{00b7}' | '\u{2722}' | '\u{2733}' | '\u{2736}' | '\u{273b}' | '\u{273d}' | '\u{25d0}'..='\u{25d3}')
    };
    let rest = title.trim_start_matches(|c: char| spinner(c) || c.is_whitespace());
    let led = title.len() - rest.len();
    if rest.is_empty() || !title[..led].chars().any(spinner) || !title[..led].ends_with(char::is_whitespace) {
        title
    } else {
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_kept_reads_back_as_it_was() {
        let mut tab = herdc_model::model::Tab::new("tab", "1", "p1", "t1");
        tab.root.size = None;
        let space = SpaceView {
            id: "s1".into(),
            index: 1,
            name: "proj".into(),
            named: true,
            folder: "~/proj".into(),
            machine: None,
            branch: Some("main".into()),
            active: true,
            status: Some("blocked"),
            tabs: vec![TabView {
                id: "tab".into(),
                index: 1,
                name: "1".into(),
                named: false,
                active: true,
                status: Some("blocked"),
                settled: false,
                settled_at: None,
                changed_at: Some(7),
                working_since: None,
                focused: Some("p1".into()),
                zoomed: None,
                root: tab.root,
                panes: Vec::new(),
            }],
        };
        let machine = Machine { name: None, state: "connected".into(), layout: "ready", error: None, host: Some("mac".into()) };
        let view = View { link: Link::Connected { core: "mac".into(), version: "0.1.18".into() }, home: "/home/me".into(), machines: vec![machine], spaces: vec![space], ..View::default() };
        let kept = serde_json::to_string(&view).unwrap();
        assert_eq!(serde_json::from_str::<View>(&kept).unwrap(), view);
    }

    #[test]
    fn spinners_leave_titles_but_status_marks_stay() {
        assert_eq!(shown_title("\u{2819} claude"), "claude");
        assert_eq!(shown_title("\u{2733} Fixing tests"), "Fixing tests");
        assert_eq!(shown_title("\u{2713} build"), "\u{2713} build");
        assert_eq!(shown_title("\u{2819}"), "\u{2819}");
    }
}
