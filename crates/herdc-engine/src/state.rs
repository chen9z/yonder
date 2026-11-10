//! The world one screen keeps with its core, as herdr-app's window keeps it
//! (`src/window.rs`): the model, the live terminals, each machine's layout
//! document, folders, what was seen. One task owns it; core calls run
//! beside it and come back as jobs.
//!
//! Ported from herdr-app `src/window.rs`, Apache-2.0.

mod documents;
mod projection;

use crate::docs::{self, Outcome, Slot};
use crate::exec;
use crate::view::{self, AgentView, Focused, Link, Machine, Note, PaneView, SpaceView, TabView, View};
use herdc_model::layout::{Axis, Content};
use herdc_model::mirror;
use herdc_model::model::{self, Alert, Model, Tab};
use herdc_model::programs;
use herdc_wire::{Client, Terminals};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

pub(crate) type Job = Box<dyn FnOnce(&mut State) + Send>;

/// Changes made outside Herdr (a commit in another app): looked for this often.
const FOLDER_FALLBACK: Duration = Duration::from_secs(60);

/// How long a labeled terminal goes without a pane before it is adopted:
/// the screen that started it is writing its layout meanwhile, and two
/// screens racing to place it would lose that screen's names.
const ADOPT_AFTER: Duration = Duration::from_secs(1);

struct FolderCheck {
    at: Instant,
    cwd: String,
    busy: bool,
    /// A repo, or git couldn't say (a machine unreachable): worth a look again.
    repo: bool,
}

/// A terminal to start for a space's tab.
pub(crate) struct Spawn {
    pub argv: Option<Vec<String>>,
    /// Empty: the machine's home.
    pub cwd: String,
    /// `cwd` only follows the focused terminal: the home will do instead.
    pub follow: bool,
    pub space: String,
    pub tab: String,
    /// Another machine; None: the core's own.
    pub on: Option<String>,
}

pub(crate) enum TerminalChange {
    Keep { space: String, tab: String, pane: String, name: String, target: Option<String>, axis: Axis },
    Park,
}

/// Where the published world goes.
pub(crate) struct Outlets {
    pub view: watch::Sender<Arc<View>>,
    pub notes: broadcast::Sender<Note>,
    pub clients: watch::Sender<Option<Client>>,
}

/// What the engine shares with the screens on its side: the grid new
/// terminals start at, the terminals it started, and the core's grid for
/// a terminal nobody types in (once connected).
#[derive(Default)]
pub(crate) struct Here {
    pub size: Option<(u16, u16)>,
    pub started: HashSet<String>,
    pub headless: Option<(u16, u16)>,
}

pub(crate) struct State {
    jobs: mpsc::UnboundedSender<Job>,
    pub(crate) tasks: crate::tasks::Tasks,
    out: Outlets,
    pub(crate) core: Option<Client>,
    /// Bumped on every (re)connect and loss: replies to an older line are dropped.
    generation: u64,
    pub(crate) model: Model,
    /// Live terminals by address, as `normalize` makes them.
    pub(crate) live: HashMap<String, Value>,
    records: Vec<Value>,
    events: Option<Terminals>,
    documents: BTreeMap<String, Slot>,
    watching: bool,
    initial_view: Option<mirror::View>,
    pub(crate) cwds: HashMap<String, String>,
    folders_checked: HashMap<String, FolderCheck>,
    written_seen: HashMap<String, u64>,
    looked: HashMap<String, u64>,
    written_where: HashMap<String, String>,
    /// A terminal shown in a pane in place of its own, for now.
    pub(crate) swapped: HashMap<String, String>,
    /// The agent an agents page shows, chosen.
    pub(crate) agent: Option<String>,
    pub(crate) pane_history: Vec<String>,
    /// Terminals whose pane is being given or taken (labels on their way).
    pub(crate) changing: HashSet<String>,
    /// Terminals being killed: true once the core said so.
    closing: HashMap<String, bool>,
    /// Terminals asked for but not answered yet: nobody adopts meanwhile.
    pub(crate) spawning: usize,
    born: HashMap<String, Instant>,
    /// Since when a labeled terminal has been without a pane here.
    orphaned: HashMap<String, Instant>,
    checking_closed: bool,
    listed_once: bool,
    /// The person has the app in front.
    pub(crate) active: bool,
    /// What the screen shows, as it says (else the shown tab's panes).
    pub(crate) looking: Option<Vec<String>>,
    pub(crate) here: Arc<Mutex<Here>>,
    pub(crate) display: Value,
    pub(crate) home: String,
    link: Link,
    machines: Vec<Machine>,
    ticks: u64,
    store: Option<PathBuf>,
    written_view: String,
    /// Told once the next settle has published.
    pub(crate) settled: Vec<oneshot::Sender<()>>,
}

impl Drop for State {
    fn drop(&mut self) {
        // Receivers retain the last value after a watch publisher is gone.
        // Clear the client so retained UI handles cannot keep SSH alive.
        self.out.clients.send_replace(None);
    }
}

impl State {
    pub(crate) fn new(jobs: mpsc::UnboundedSender<Job>, out: Outlets, store: Option<PathBuf>, tasks: crate::tasks::Tasks) -> State {
        let initial_view = store.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok());
        State {
            jobs,
            tasks,
            out,
            core: None,
            generation: 0,
            model: Model::default(),
            live: HashMap::new(),
            records: Vec::new(),
            events: None,
            documents: BTreeMap::from([(String::new(), Slot::default())]),
            watching: false,
            initial_view,
            cwds: HashMap::new(),
            folders_checked: HashMap::new(),
            written_seen: HashMap::new(),
            looked: HashMap::new(),
            written_where: HashMap::new(),
            swapped: HashMap::new(),
            agent: None,
            pane_history: Vec::new(),
            changing: HashSet::new(),
            closing: HashMap::new(),
            spawning: 0,
            born: HashMap::new(),
            orphaned: HashMap::new(),
            checking_closed: false,
            listed_once: false,
            active: true,
            looking: None,
            here: Arc::default(),
            display: Value::Null,
            home: String::new(),
            link: Link::Connecting,
            machines: Vec::new(),
            ticks: 0,
            store,
            written_view: String::new(),
            settled: Vec::new(),
        }
    }

    // ---- running beside ----------------------------------------------------

    /// Runs `work` off this task; `then` gets its result here, unless the
    /// line to the core changed meanwhile.
    pub(crate) fn later<T: Send + 'static>(
        &self,
        work: impl Future<Output = T> + Send + 'static,
        then: impl FnOnce(&mut State, T) + Send + 'static,
    ) {
        let (jobs, generation) = (self.jobs.clone(), self.generation);
        self.tasks.spawn(async move {
            let result = work.await;
            let _ = jobs.send(Box::new(move |s: &mut State| {
                if s.generation == generation {
                    then(s, result)
                }
            }));
        });
    }

    pub(crate) fn client(&self) -> Result<Client, String> {
        self.core.clone().ok_or_else(|| "not connected to the core".into())
    }

    pub(crate) fn note(&self, note: Note) {
        let _ = self.out.notes.send(note);
    }

    /// A label written off this task; `failed` runs here if the core refused.
    fn label(&self, terminal: &str, labels: Value, failed: impl FnOnce(&mut State) + Send + 'static) {
        let Ok(core) = self.client() else { return };
        let params = json!({"terminal": terminal, "labels": labels});
        self.later(async move { core.call("terminal.label", params).await }, move |s, r| {
            if let Err(e) = r {
                tracing::warn!("label: {e}");
                failed(s);
            }
        });
    }

    // ---- the line to the core ----------------------------------------------

    pub(crate) fn connected(&mut self, core: Client, home: Option<String>) {
        self.generation += 1;
        let hello = core.hello();
        self.link = Link::Connected {
            core: hello.pointer("/core/name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            version: hello["version"].as_str().unwrap_or_default().to_owned(),
        };
        if let Some(home) = home {
            self.home = home;
        }
        for (machine, slot) in &mut self.documents {
            slot.restart(machine.is_empty());
        }
        self.documents.entry(String::new()).or_default().connection(true, None, None);
        self.watching = false;
        self.events = None;
        (self.spawning, self.checking_closed) = (0, false);
        self.changing.clear();
        self.core = Some(core.clone());
        let _ = self.out.clients.send(Some(core));
    }

    pub(crate) fn lost(&mut self, error: String) {
        self.generation += 1;
        self.core = None;
        let _ = self.out.clients.send(None);
        self.link = Link::Lost { error };
        self.watching = false;
        self.events = None;
        for slot in self.documents.values_mut() {
            slot.lost();
        }
    }

    pub(crate) fn subscribed(&mut self, snapshot: Value) {
        self.events = Some(Terminals::new(&snapshot));
        self.watching = true;
        self.documents.entry(String::new()).or_default().connection(true, None, None);
        self.flush_documents(true);
        self.events_changed();
        self.ask_machines();
    }

    pub(crate) fn unsubscribed(&mut self) {
        self.watching = false;
        self.events = None;
    }

    pub(crate) fn pushed(&mut self, bytes: &[u8]) {
        let Some(events) = &mut self.events else { return };
        let pushed = events.push(bytes);
        if pushed.docs {
            // A `missed` names no machine: everything is read again.
            let all = pushed.doc_machines.is_empty();
            for (machine, slot) in &mut self.documents {
                let named = pushed.doc_machines.iter().any(|m| m.as_deref().unwrap_or("") == machine);
                if all || named { slot.invalidate(); }
            }
        }
        if pushed.terminals {
            self.events_changed();
            self.ask_machines();
        }
    }

    fn events_changed(&mut self) {
        let Some(events) = &mut self.events else { return };
        // Our seen writes the core has applied (or, after a fresh snapshot,
        // can't tell): its labels decide again.
        let (seen, resynced) = events.take_seen();
        if resynced {
            self.written_seen.clear();
        } else {
            self.written_seen.retain(|a, w| seen.get(a).is_none_or(|s| s < w));
        }
        let live = events.live();
        self.terminals_listed(live);
    }

    pub(crate) fn tick(&mut self) {
        if self.core.is_none() {
            return;
        }
        self.ticks += 1;
        // Machines send no events: asked now and then.
        if self.ticks.is_multiple_of(5) {
            self.ask_machines();
        }
        self.tend_terminals(false);
    }

    fn ask_machines(&mut self) {
        let Ok(core) = self.client() else { return };
        self.later(async move { core.call("machine.list", json!({})).await }, |s, r| match r {
            Ok(machines) => {
                s.documents_updated(Some(machines), vec![]);
            }
            Err(e) => tracing::warn!("machine.list: {e}"),
        });
    }

    // ---- what the screen tells ---------------------------------------------

    /// After every job: what's seen, where terminals live, the documents and
    /// the published view are made true again.
    pub(crate) fn settle(&mut self) {
        self.model.order_spaces();
        let (model, live) = (&self.model, &self.live);
        self.swapped.retain(|p, a| model.address_of(p).is_some() && live.contains_key(a));
        if let Some(p) = self.model.focused_pane() {
            crate::geometry::remember(&mut self.pane_history, &p);
        }
        let model = &self.model;
        self.pane_history.retain(|id| model.tab_of(id).is_some());
        if self.active {
            let visible = self.visible_addresses();
            self.model.mark_seen(&visible);
        }
        self.share_seen();
        self.write_where();
        self.flush_documents(false);
        self.publish();
        self.save_view();
        for tx in self.settled.drain(..) {
            let _ = tx.send(());
        }
    }

    /// What's on screen now, for done-vs-seen.
    pub(crate) fn visible_addresses(&self) -> Vec<String> {
        if !self.active {
            return vec![];
        }
        if let Some(looking) = &self.looking {
            return looking.clone();
        }
        let Some(tab) = self.model.active_tab() else { return vec![] };
        tab.shown()
            .visible_blocks()
            .into_iter()
            .filter_map(|n| self.swapped.get(&n.id).cloned().or_else(|| n.terminal().map(str::to_owned)))
            .collect()
    }

    /// Shares what the person looks at as `herdr.seen` (herdr-app
    /// docs/labels.md): an agent on screen, once per turn while it stays
    /// there, so another screen marking it unread isn't undone by this one.
    /// Only ever raises it.
    fn share_seen(&mut self) -> bool {
        let looking: HashSet<String> = self.visible_addresses().into_iter().collect();
        self.looked.retain(|a, _| looking.contains(a));
        let mut changed = false;
        for addr in looking {
            let Some(a) = self.model.agents.get(&addr) else { continue };
            let turn = a.turn;
            if a.word.is_none() || !self.live.contains_key(&addr) || self.looked.get(&addr) == Some(&turn) {
                continue;
            }
            self.looked.insert(addr.clone(), turn);
            if a.seen_turn.is_some_and(|s| s >= turn) {
                continue;
            }
            // Shown as seen now, not after the core's round trip.
            changed |= self.model.set_turn(&addr, turn, Some(turn));
            self.written_seen.insert(addr.clone(), turn);
            let again = addr.clone();
            self.label(&addr, json!({"herdr.seen": turn.to_string()}), move |s| {
                if s.written_seen.get(&again) == Some(&turn) {
                    s.written_seen.remove(&again);
                    s.looked.remove(&again);
                }
            });
        }
        changed
    }

    /// Tells the core where each terminal lives, in words: the `where` label,
    /// `<space> / <tab>`, which hc shows in its picker and `ls`. Sub-agents
    /// get their parent's. Written only when it changes.
    fn write_where(&mut self) {
        let m = &self.model;
        let mut want: HashMap<String, String> = HashMap::new();
        for p in &m.spaces {
            let name = m.space_name(p, &self.home);
            for c in &p.tabs {
                for a in model::addresses(&c.root) {
                    want.insert(a, format!("{name} / {}", Model::tab_name(p, c)));
                }
            }
        }
        for addr in self.live.keys() {
            if want.contains_key(addr) {
                continue;
            }
            let mut cur = addr.clone();
            for _ in 0..32 {
                let Some(p) = self.live.get(&cur).and_then(|r| r["parent"].as_str()) else { break };
                if let Some(place) = want.get(p).cloned() {
                    want.insert(addr.clone(), place);
                    break;
                }
                cur = p.to_owned();
            }
        }
        let live = &self.live;
        self.written_where.retain(|a, _| live.contains_key(a));
        for (addr, place) in want {
            if self.written_where.get(&addr) == Some(&place) || !self.live.contains_key(&addr) {
                continue;
            }
            // A failed write is forgotten so the next settle tries again.
            self.written_where.insert(addr.clone(), place.clone());
            let (again, wrote) = (addr.clone(), place.clone());
            self.label(&addr, json!({"where": place}), move |s| {
                if s.written_where.get(&again) == Some(&wrote) {
                    s.written_where.remove(&again);
                }
            });
        }
    }

    fn publish(&mut self) {
        let view = self.view();
        if **self.out.view.borrow() != view {
            let _ = self.out.view.send(Arc::new(view));
        }
    }

    // ---- core facts --------------------------------------------------------

    fn terminals_listed(&mut self, list: Vec<Value>) {
        let mut live = HashMap::new();
        self.records = list.into_iter().map(normalize).collect();
        for r in &self.records {
            if let Some(a) = r["address"].as_str() {
                live.insert(a.to_owned(), r.clone());
            }
        }
        self.live = live;
        let live = &self.live;
        self.closing.retain(|addr, completed| !*completed || live.contains_key(addr));
        self.tend_terminals(true);
    }

    /// With or without news from the core: where the terminals go (a layout
    /// that arrived late can place one now), what they mean now (a finished
    /// agent you're looking at is no longer news), their folders.
    fn tend_terminals(&mut self, listed: bool) {
        // Place newly labeled terminals first: one blocked on its first
        // listing is ours, and must ring.
        self.reconcile();
        let visible: HashSet<String> = self.visible_addresses().into_iter().collect();
        let mut alerts = Vec::new();
        for (addr, rec) in &self.live {
            let word = rec["status"].as_str().map(str::to_owned);
            let app = rec.pointer("/report/app").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
            let looking = visible.contains(addr);
            let prev = self.model.agents.get(addr).and_then(|a| a.word.clone());
            let child = rec["parent"].as_str().is_some_and(|p| !p.is_empty());
            if let Some(alert) = self.model.alert_for(addr, child, prev.as_deref(), word.as_deref(), looking) {
                alerts.push((addr.clone(), alert, looking));
            }
            self.model.set_word(addr, word, app, looking);
            let turn = rec["turn"].as_u64().unwrap_or(0);
            let label = rec.pointer("/labels/herdr.seen").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok());
            self.model.set_turn(addr, turn, label.max(self.written_seen.get(addr).copied()));
        }
        let live = &self.live;
        self.written_seen.retain(|a, _| live.contains_key(a));
        // Changes already made before this screen connected aren't news.
        if self.listed_once {
            for (addr, alert, looking) in alerts {
                self.alert(&addr, alert, looking);
            }
        }
        self.track_folders();
        if listed {
            self.listed_once = true;
        }
    }

    /// The parent chain's top: the agent a sub-agent belongs to.
    pub(crate) fn root_agent(&self, addr: &str) -> String {
        let mut cur = addr.to_owned();
        for _ in 0..32 {
            match self.live.get(&cur).and_then(|r| r["parent"].as_str()) {
                Some(p) if self.live.contains_key(p) => cur = p.to_owned(),
                _ => break,
            }
        }
        cur
    }

    /// An agent needs the person or finished: a sound, and a toast (tap it
    /// to go there) unless they're looking at that agent.
    fn alert(&mut self, addr: &str, alert: Alert, looking: bool) {
        // Needing the person brings a settled tab back.
        if let Some(tab) = self.model.pane_of(&self.root_agent(addr)).and_then(|p| self.model.tab_of(&p)).map(|c| c.id.clone()) {
            self.model.unsettle(&tab);
        }
        let rec = self.live.get(addr);
        let app = self.model.agents.get(addr).and_then(|a| a.app.clone());
        let name = rec
            .and_then(|r| r["name"].as_str().filter(|s| !s.is_empty()).map(str::to_owned))
            .or_else(|| app.as_deref().map(programs::name))
            .unwrap_or_else(|| "An agent".into());
        // A sub-agent lives where its top agent does.
        let home = self.root_agent(addr);
        let place = self.model.pane_of(&home).and_then(|pane| {
            let (p, c) = (self.model.space_of(&pane)?, self.model.tab_of(&pane)?);
            Some(format!("{} / {}", self.model.space_name(p, &self.home), Model::tab_name(p, c)))
        });
        let (title, level, sound) = match alert {
            Alert::Blocked => (format!("{name} needs you"), "error", "blocked"),
            Alert::Done => (format!("{name} finished"), "info", "done"),
        };
        self.note(Note {
            title,
            body: place,
            level,
            action: Some(view::Action { name: "agent.focus".into(), args: json!({"terminal": addr}) }),
            alert: Some(sound),
            toast: !looking,
        });
    }

    /// Keeps the model true to the core on every listing: drops panes whose
    /// terminal is gone, and places terminals labeled `herdr.space` that
    /// aren't shown (started by an agent, a script, another screen).
    fn reconcile(&mut self) {
        // Kept until hidden panes have passed through `check_closed`.
        self.born.retain(|_, t| t.elapsed() < Duration::from_secs(30));
        let just_born = |a: &str| self.born.get(a).is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        let missing: Vec<(String, String)> = self
            .model
            .tabs()
            .flat_map(|c| c.root.terminals())
            .filter_map(|n| n.terminal().filter(|a| !self.live.contains_key(*a) && !just_born(a)).map(|a| (n.id.clone(), a.to_owned())))
            .collect();
        if !missing.is_empty() && !self.checking_closed && self.events.is_some() {
            self.check_closed(missing);
        }
        if self.spawning > 0 {
            return;
        }
        let mut orphans: Vec<(String, String, Option<String>)> = self
            .live
            .iter()
            .filter(|(a, _)| self.model.pane_of(a).is_none() && !self.changing.contains(*a) && !self.closing.contains_key(*a))
            .filter_map(|(a, r)| {
                let machine = a.split_once('/').map(|(m, _)| m);
                if !self.documents.get(machine.unwrap_or("")).is_some_and(|s| s.available()) {
                    return None;
                }
                let space = r.pointer("/labels/herdr.space")?.as_str().filter(|s| !s.is_empty())?;
                let tab = r.pointer("/labels/herdr.tab").and_then(Value::as_str).filter(|s| !s.is_empty());
                Some((a.clone(), mirror::qualify(space, machine), tab.map(|t| mirror::qualify(t, machine))))
            })
            .collect();
        orphans.sort();
        self.orphaned.retain(|a, _| orphans.iter().any(|(o, ..)| o == a));
        for (addr, space, tab) in orphans {
            if self.orphaned.entry(addr.clone()).or_insert_with(Instant::now).elapsed() >= ADOPT_AFTER {
                self.start_adoption(addr, space, tab);
            }
        }
    }

    fn start_adoption(&mut self, addr: String, space: String, tab: Option<String>) {
        let Ok(core) = self.client() else { return };
        self.changing.insert(addr.clone());
        let (address, wanted_space, wanted_tab) = (addr.clone(), space.clone(), tab.clone());
        let ready = async move {
            let record = core.call("terminal.get", json!({"terminal": address})).await?;
            let machine = address.split_once('/').map(|(m, _)| m);
            if !adoptable(&record, &wanted_space, wanted_tab.as_deref(), machine) {
                return Ok(false);
            }
            if record["keep"] != true {
                core.call("terminal.keep", json!({"terminal": address, "keep": true})).await?;
            }
            Ok::<_, herdc_wire::Error>(true)
        };
        self.later(ready, move |s, ready| {
            s.changing.remove(&addr);
            if matches!(ready, Ok(true))
                && s.model.pane_of(&addr).is_none()
                && s.require_document(addr.split_once('/').map(|(m, _)| m)).is_ok()
            {
                s.adopt(&addr, &space, tab.as_deref());
            }
        });
    }

    /// Puts a live terminal where its labels say, without changing what's
    /// shown. A space or tab this screen doesn't know is made under the
    /// label's id, unnamed.
    fn adopt(&mut self, addr: &str, space: &str, tab: Option<&str>) {
        let machine = addr.split_once('/').map(|(m, _)| m.to_owned());
        let pane = self.model.fresh_id_on("p", machine.as_deref());
        let tab_id = tab.map(str::to_owned).unwrap_or_else(|| self.model.fresh_id_on("tb", machine.as_deref()));
        let new_tab = Tab::new(&tab_id, "", &pane, addr);
        if self.model.space(space).is_none() {
            // Its live folder may not be read yet: the core's record has it.
            let rec = self.live.get(addr);
            let folder = self
                .cwds
                .get(addr)
                .cloned()
                .or_else(|| rec.and_then(|r| r.pointer("/foreground/cwd").or_else(|| r.get("cwd"))).and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| self.home.clone());
            let active = self.model.active.clone();
            self.model.add_space(space, "", &folder, new_tab);
            self.model.space_mut(space).unwrap().machine = machine;
            self.model.active = active.or(Some(space.to_owned()));
            return;
        }
        let target = self.model.tab(&tab_id).and_then(|c| c.root.panes().last().map(|p| p.id.clone()));
        if let Some(target) = target {
            let tab = self.model.tab_of(&target).unwrap();
            let (focused, before, zoomed, id) = (tab.focused.clone(), tab.before.clone(), tab.zoomed.clone(), tab.id.clone());
            self.model.split(&target, Axis::Row, &pane, addr);
            let tab = self.model.tab_mut(&id).unwrap();
            (tab.focused, tab.before, tab.zoomed) = (focused, before, zoomed);
            return;
        }
        let keep = self.model.space(space).and_then(|p| p.active.clone());
        self.model.add_tab(space, new_tab);
        if let Some(p) = self.model.space_mut(space) {
            p.active = keep;
        }
    }

    /// Asks only whether a missing terminal is closed. The core owns restarting it.
    fn check_closed(&mut self, missing: Vec<(String, String)>) {
        let Ok(core) = self.client() else { return };
        self.checking_closed = true;
        let work = async move {
            let mut closed = Vec::new();
            for (pane, addr) in missing {
                let gone = match core.call("terminal.get", json!({"terminal": addr})).await {
                    Ok(v) => matches!(v["reason"].as_str(), Some("exited" | "killed")).then_some(v),
                    Err(e) => (e.kind() == Some("not_found")).then_some(Value::Null),
                };
                if let Some(record) = gone {
                    let output = match launch_failure(&record, "") {
                        Some(_) => core.call("terminal.read", json!({"terminal": addr, "lines": 5})).await.unwrap_or_default(),
                        None => Value::Null,
                    };
                    closed.push((pane, addr, record, output));
                }
            }
            closed
        };
        self.later(work, |s, closed| {
            s.checking_closed = false;
            for (pane, addr, record, output) in closed {
                if s.model.address_of(&pane).as_deref() != Some(&addr) {
                    continue;
                }
                if s.born.remove(&addr).is_some()
                    && let Some(body) = launch_failure(&record, output["text"].as_str().unwrap_or_default())
                {
                    s.note(Note::error("Terminal failed to start", body));
                }
                s.model.remove_pane(&pane);
            }
        });
    }

    /// Reads each terminal's live folder, and its repo and branch.
    fn track_folders(&mut self) {
        let Ok(core) = self.client() else { return };
        let mut todo: HashMap<(Option<String>, String), Vec<String>> = HashMap::new();
        for (addr, rec) in &self.live {
            // The core's live folder of what runs there, else where it started.
            let cwd = rec.pointer("/foreground/cwd").or_else(|| rec.get("cwd")).and_then(Value::as_str).map(str::to_owned);
            let Some(cwd) = cwd else { continue };
            let busy = busy(rec);
            let last = self.folders_checked.get_mut(addr);
            if recheck(last.as_deref(), &cwd, busy, self.active) {
                self.cwds.insert(addr.clone(), cwd.clone());
                let repo = last.as_ref().is_none_or(|l| l.cwd != cwd || l.repo); // until git says otherwise
                self.folders_checked.insert(addr.clone(), FolderCheck { at: Instant::now(), cwd: cwd.clone(), busy, repo });
                // Another machine's terminal (`vps/t7`): git runs there.
                todo.entry((addr.split_once('/').map(|(m, _)| m.to_owned()), cwd)).or_default().push(addr.clone());
            } else if let Some(last) = last {
                last.busy = busy;
            }
        }
        let live = &self.live;
        self.folders_checked.retain(|a, _| live.contains_key(a));
        self.cwds.retain(|a, _| live.contains_key(a) || self.born.contains_key(a));
        // Adopted ones wait for the core's first listing.
        if self.events.is_some() {
            self.here.lock().unwrap().started.retain(|a| live.contains_key(a) || self.born.contains_key(a));
        }
        // Once per machine and folder, however many terminals are there.
        for ((on, cwd), addrs) in todo {
            let (core, home, asked) = (core.clone(), self.home.clone(), cwd.clone());
            self.later(async move { exec::folder_info(&core, &asked, &home, on.as_deref()).await }, move |s, (info, repo)| {
                // Not one that moved on meanwhile: its own answer is coming.
                for addr in addrs.into_iter().filter(|a| s.cwds.get(a) == Some(&cwd)) {
                    if let Some(check) = s.folders_checked.get_mut(&addr) {
                        check.repo = repo;
                    }
                    s.model.folder_info.insert(addr, info.clone());
                }
            });
        }
    }

    // ---- creating and ending terminals -------------------------------------

    /// Starts a terminal; `then` gets its address and the folder it
    /// started in. A failed folder it only followed gets one retry in the
    /// machine's home.
    pub(crate) fn spawn(&mut self, request: Spawn, then: impl FnOnce(&mut State, Result<(String, String), String>) + Send + 'static) {
        let Spawn { argv, cwd, follow, space, tab, on } = request;
        let core = match self.client().and_then(|c| self.require_document(on.as_deref()).map(|_| c)) {
            Ok(c) => c,
            Err(e) => return then(self, Err(e)),
        };
        let argv = argv.unwrap_or_default();
        let labels = json!({
            "herdr.space": mirror::local(&space, on.as_deref()),
            "herdr.tab": mirror::local(&tab, on.as_deref()),
            "herdr.seen": "0",
        });
        let env = json!({"TERM": "xterm-256color", "COLORTERM": "truecolor", "TERM_PROGRAM": "herdr"});
        let mut params = json!({"argv": argv, "env": env, "labels": labels, "on": on, "keep": true});
        if !self.display.is_null() {
            params["display"] = self.display.clone();
        }
        if let Some((cols, rows)) = self.here.lock().unwrap().size {
            params["size"] = json!({"cols": cols, "rows": rows});
        }
        let home = if on.is_some() { String::new() } else { self.home.clone() };
        self.spawning += 1;
        let work = async move {
            let run = |cwd: String, argv: Vec<String>| {
                let mut p = params.clone();
                p["cwd"] = if cwd.is_empty() { Value::Null } else { json!(cwd) };
                p["argv"] = json!(argv);
                let core = core.clone();
                async move {
                    let r = core.call("terminal.run", p).await?;
                    Ok::<_, herdc_wire::Error>(r["address"].as_str().unwrap_or_default().to_owned())
                }
            };
            let first = match run(cwd.clone(), argv.clone()).await {
                // An agent on a machine without `sh` (Windows): its PowerShell instead.
                Err(e) if argv.first().map(String::as_str) == Some("sh") && e.to_string().contains("not found") => {
                    run(cwd.clone(), powershell_argv(&argv)).await
                }
                r => r,
            };
            let result = match first {
                // A missing folder and a missing program both say spawn_failed;
                // the holder's timeout may still create one: never retry that.
                Err(e) if follow && !cwd.is_empty() && e.kind() == Some("spawn_failed") && !e.to_string().contains("the holder did not answer") => {
                    run(home.clone(), argv).await.map(|a| (a, home))
                }
                r => r.map(|a| (a, cwd)),
            };
            result.map_err(|e| format!("cannot start a terminal: {e}")).and_then(|(a, cwd)| {
                if a.is_empty() { Err("terminal.run returned no address".into()) } else { Ok((a, cwd)) }
            })
        };
        self.later(work, move |s, result| {
            s.spawning = s.spawning.saturating_sub(1);
            if let Ok((addr, cwd)) = &result {
                s.born.insert(addr.clone(), Instant::now());
                s.here.lock().unwrap().started.insert(addr.clone());
                // Follow the launch folder until the first live folder report.
                if !cwd.is_empty() {
                    s.cwds.insert(addr.clone(), cwd.clone());
                }
            }
            then(s, result)
        });
    }

    /// Kills terminals and their sub-agents, all the way down. Kept
    /// sub-agents (given a tab) are the person's: spared.
    pub(crate) fn kill(&mut self, addresses: &[String]) {
        let mut doomed: Vec<String> = addresses.to_vec();
        let mut i = 0;
        while i < doomed.len() {
            let parent = doomed[i].clone();
            for (addr, rec) in &self.live {
                let child = rec["parent"].as_str() == Some(parent.as_str());
                let kept = rec.pointer("/labels/herdr.space").and_then(Value::as_str).is_some_and(|w| !w.is_empty());
                if child && !kept && !doomed.contains(addr) {
                    doomed.push(addr.clone());
                }
            }
            i += 1;
        }
        self.closing.extend(doomed.iter().cloned().map(|a| (a, false)));
        let Ok(core) = self.client() else { return };
        for address in doomed {
            let core = core.clone();
            let addr = address.clone();
            self.later(async move { core.call("terminal.kill", json!({"terminal": addr})).await }, move |s, result| {
                if result.is_ok() && s.live.contains_key(&address) {
                    // The live list may still predate the kill's reply.
                    s.closing.insert(address, true);
                } else {
                    s.closing.remove(&address);
                }
            });
        }
    }

    /// Labels and keep change first; only a successful reply moves the pane.
    pub(crate) fn change_terminal(&mut self, address: String, change: TerminalChange) -> Result<(), String> {
        let core = self.client()?;
        let machine = address.split_once('/').map(|(m, _)| m.to_owned());
        let (keep, labels) = match &change {
            TerminalChange::Keep { space, tab, .. } => (
                true,
                json!({"herdr.space": mirror::local(space, machine.as_deref()), "herdr.tab": mirror::local(tab, machine.as_deref())}),
            ),
            TerminalChange::Park => (false, json!({"herdr.space": "", "herdr.tab": ""})),
        };
        self.changing.insert(address.clone());
        let addr = address.clone();
        let work = async move {
            core.call("terminal.label", json!({"terminal": addr, "labels": labels})).await?;
            core.call("terminal.keep", json!({"terminal": addr, "keep": keep})).await?;
            Ok::<_, herdc_wire::Error>(())
        };
        self.later(work, move |s, result| s.apply_terminal_change(address, change, result.map_err(|e| e.to_string())));
        Ok(())
    }

    fn apply_terminal_change(&mut self, address: String, change: TerminalChange, result: Result<(), String>) {
        self.changing.remove(&address);
        if let Err(error) = result {
            tracing::error!("terminal change for {address} failed: {error}");
            return self.note(Note::error("Could not change terminal pane", error));
        }
        match change {
            TerminalChange::Keep { space, tab, pane, name, target, axis } => {
                if !self.live.contains_key(&address) {
                    return;
                }
                if self.model.space(&space).is_none() {
                    if let Err(error) = self.change_terminal(address, TerminalChange::Park) {
                        tracing::error!("cannot undo pane placement into closed space: {error}");
                    }
                    return;
                }
                if self.model.pane_of(&address).is_none() {
                    let destination = target
                        .filter(|p| self.model.tab_of(p).is_some_and(|t| t.id == tab))
                        .or_else(|| self.model.tab(&tab).and_then(|t| t.root.panes().first().map(|p| p.id.clone())));
                    match destination {
                        Some(target) => {
                            self.model.split(&target, axis, &pane, &address);
                        }
                        None => {
                            self.model.add_tab(&space, Tab::new(&tab, &name, &pane, &address));
                        }
                    }
                }
                if let Some(pane) = self.model.pane_of(&address) {
                    self.model.focus_pane(&pane);
                }
            }
            TerminalChange::Park => {
                if let Some(pane) = self.model.pane_of(&address) {
                    self.model.remove_pane(&pane);
                }
            }
        }
    }

    // ---- the published view ------------------------------------------------


}

/// A terminal record as screens use it: `status` its 7501 word, `report`
/// the agent's own, `program` how it shows, the title without a spinner.
fn normalize(mut t: Value) -> Value {
    let Some(obj) = t.as_object_mut() else { return t };
    let report = obj.remove("status").unwrap_or(Value::Null);
    let word = report.get("state").cloned().unwrap_or_else(|| if report.is_string() { report.clone() } else { Value::Null });
    obj.insert("status".into(), word);
    let argv: Vec<String> = running_argv(obj).and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
    obj.insert("program".into(), programs::of(report.get("app").and_then(Value::as_str), &argv));
    obj.insert("report".into(), report);
    if let Some(title) = obj.get("title").and_then(Value::as_str).map(|t| view::shown_title(t).to_owned()) {
        obj.insert("title".into(), title.into());
    }
    t
}

fn running_argv(rec: &serde_json::Map<String, Value>) -> Option<&Value> {
    rec.get("foreground").and_then(|f| f.get("argv")).filter(|a| a.as_array().is_some_and(|a| !a.is_empty())).or_else(|| rec.get("argv"))
}

fn busy(rec: &Value) -> bool {
    let program = |k: &str| rec["program"][k].as_bool() == Some(true);
    rec["status"] == "working" || !(program("shell") || program("agent"))
}

/// Whether to ask a folder for its repo and branch again: only when the
/// answer could have changed.
fn recheck(last: Option<&FolderCheck>, cwd: &str, busy: bool, active: bool) -> bool {
    let Some(last) = last else { return true };
    last.cwd != cwd || (last.busy && !busy) || (active && last.repo && last.at.elapsed() > FOLDER_FALLBACK)
}

/// Whether the core's record still puts a terminal in this space and tab
/// and it still runs (labels outlive the program).
fn adoptable(record: &Value, space: &str, tab: Option<&str>, machine: Option<&str>) -> bool {
    let label = record.pointer("/labels/herdr.space").and_then(Value::as_str);
    let labeled_tab = record.pointer("/labels/herdr.tab").and_then(Value::as_str).filter(|s| !s.is_empty());
    record["state"] == "live" && label == Some(mirror::local(space, machine)) && labeled_tab == tab.map(|t| mirror::local(t, machine))
}

/// Why a terminal that just started ended at once: its code and last lines.
fn launch_failure(record: &Value, output: &str) -> Option<String> {
    let code = record["exit_code"].as_i64().filter(|c| *c != 0)?;
    let elapsed = record["finished_ms"].as_u64()?.checked_sub(record["created_ms"].as_u64()?)?;
    if record["reason"].as_str() != Some("exited") || elapsed > 2000 {
        return None;
    }
    let lines = output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    Some(if tail.is_empty() { format!("Exit code {code}") } else { format!("Exit code {code}\n{tail}") })
}

/// The PowerShell form of a remote agent start (`shell::launch`): the
/// program, then a shell that stays.
fn powershell_argv(sh: &[String]) -> Vec<String> {
    let cmd = sh.get(4..).unwrap_or_default();
    vec!["powershell.exe".into(), "-NoLogo".into(), "-NoExit".into(), "-Command".into(), programs::powershell_line(cmd)]
}

#[cfg(test)]
mod tests;
