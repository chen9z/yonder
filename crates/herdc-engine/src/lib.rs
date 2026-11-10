//! Herdr's window without the window: what herdr-app does between its core
//! and its screen, for a screen of any kind (a phone's). It keeps spaces,
//! tabs and panes with each machine's layout document, runs the commands
//! that change them, tends the terminals (agents' words, what was seen,
//! where they live, folders and branches) and publishes all of it as one
//! [`View`]. [`Viewer`]s show single terminals.
//!
//! The line to the core is the caller's: a [`Connector`] dials a fresh
//! client (over SSH, say) whenever the engine needs one.

mod commands;
mod docs;
mod exec;
mod geometry;
mod state;
pub mod tools;
mod view;
mod viewer;

pub use commands::{find as find_command, Arg, Command, COMMANDS};
pub use exec::{exec, folder_info, Ran};
pub use geometry::{nearest, rects, seams, Rect, Seam};
pub use view::{shown_title, Action, AgentView, Focused, Link, Machine, Note, PaneView, SpaceView, TabView, View};
pub use viewer::{Info, Mode, Viewer, HISTORY};
use viewer::Seat;

use commands::Reply;
use herdc_wire::{Client, Item};
use serde_json::{json, Value};
use state::{Here, Job, Outlets, State};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, watch, Notify};

pub type Connecting = Pin<Box<dyn Future<Output = Result<Client, String>> + Send>>;
/// Dials the core: called again after every loss, with backoff.
pub type Connector = Arc<dyn Fn() -> Connecting + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Where this screen keeps its own choices (focus, zoom, settled tabs,
    /// what was seen) across restarts. None: nowhere.
    pub store: Option<PathBuf>,
    /// The grid new terminals start with (the screen's).
    pub size: Option<(u16, u16)>,
    /// Display facts for new terminals (`Display` in herdr-core's schema).
    pub display: Value,
}

/// A running engine. Cheap to clone; it stops when every clone is gone.
#[derive(Clone)]
pub struct Engine {
    jobs: mpsc::UnboundedSender<Job>,
    view: watch::Receiver<Arc<View>>,
    notes: broadcast::Sender<Note>,
    clients: watch::Receiver<Option<Client>>,
    redial: Arc<Notify>,
    here: Arc<Mutex<Here>>,
    /// The screen is seen (the app in front): viewers stream only then.
    shown: Arc<watch::Sender<bool>>,
}

impl Engine {
    /// Starts on the current tokio runtime.
    pub fn start(connect: Connector, options: Options) -> Engine {
        let (jobs, mut queue) = mpsc::unbounded_channel::<Job>();
        let (view_tx, view) = watch::channel(Arc::new(View::default()));
        let (notes, _) = broadcast::channel(64);
        let (clients_tx, clients) = watch::channel(None);
        let redial = Arc::new(Notify::new());
        let out = Outlets { view: view_tx, notes: notes.clone(), clients: clients_tx };
        let mut state = State::new(jobs.clone(), out, options.store);
        state.display = options.display;
        let here = Arc::new(Mutex::new(Here { size: options.size, ..Here::default() }));
        state.here = here.clone();

        let weak = jobs.downgrade();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    job = queue.recv() => match job {
                        Some(job) => job(&mut state),
                        None => break,
                    },
                    _ = tick.tick() => state.tick(),
                }
                // Everything that arrived together, then one settle.
                while let Ok(job) = queue.try_recv() {
                    job(&mut state);
                }
                state.settle();
            }
        });
        tokio::spawn(line(weak, connect, redial.clone()));
        Engine { jobs, view, notes, clients, redial, here, shown: Arc::new(watch::Sender::new(true)) }
    }

    /// Runs a command (see [`COMMANDS`]): what it made, or why not, once
    /// the view shows it. A reply with `ask` wants the person's answer first.
    pub async fn run(&self, name: &str, args: Value) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        let (name, reply) = (name.to_owned(), Reply::new(tx));
        self.job(move |s| s.run(&name, args, reply));
        let result = rx.await.unwrap_or_else(|_| Err("the engine stopped".into()));
        self.settled().await;
        result
    }

    /// Resolves once everything sent so far is in the published view.
    pub async fn settled(&self) {
        let (tx, rx) = oneshot::channel();
        self.job(move |s| s.settled.push(tx));
        let _ = rx.await;
    }

    /// Runs a command nobody waits for: a failure becomes a note.
    pub fn send(&self, name: &str, args: Value) {
        let (engine, name) = (self.clone(), name.to_owned());
        tokio::spawn(async move {
            if let Err(error) = engine.run(&name, args).await {
                let _ = engine.notes.send(Note::error(format!("{name} failed"), error));
            }
        });
    }

    pub fn view(&self) -> Arc<View> {
        self.view.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<Arc<View>> {
        self.view.clone()
    }

    pub fn notes(&self) -> broadcast::Receiver<Note> {
        self.notes.subscribe()
    }

    /// The line to the core now (None while dialing).
    pub fn client(&self) -> Option<Client> {
        self.clients.borrow().clone()
    }

    pub fn clients(&self) -> watch::Receiver<Option<Client>> {
        self.clients.clone()
    }

    /// The person has the app in front (what's shown counts as seen) or not.
    pub fn set_active(&self, active: bool) {
        self.job(move |s| {
            s.active = active;
        });
    }

    /// The app is on screen or not. Out of sight, viewers let their
    /// terminals go (no output crosses the line for nobody); back, they
    /// start again at the screen as it is. Statuses keep coming.
    pub fn set_shown(&self, shown: bool) {
        self.shown.send_replace(shown);
    }

    /// The terminals the screen shows now; None: the shown tab's panes.
    pub fn set_looking(&self, addresses: Option<Vec<String>>) {
        self.job(move |s| s.looking = addresses);
    }

    /// The grid new terminals start with.
    pub fn set_size(&self, cols: u16, rows: u16) {
        self.here.lock().unwrap().size = Some((cols, rows));
    }

    pub fn set_display(&self, display: Value) {
        self.job(move |s| s.display = display);
    }

    /// Back in front after the system froze the app: a line that went
    /// meanwhile can look alive for a minute (until its keepalives fail),
    /// so one that doesn't answer at once is dialed again.
    pub async fn check(&self) {
        let Some(core) = self.client() else { return self.redial() };
        if core.call_timeout("server.status", json!({}), Duration::from_secs(3)).await.is_err() {
            self.redial();
        }
    }

    /// The terminals this engine started (or adopted), to tell the next
    /// one: the app's next start.
    pub fn started(&self) -> Vec<String> {
        let mut all: Vec<String> = self.here.lock().unwrap().started.iter().cloned().collect();
        all.sort();
        all
    }

    /// Terminals a screen before this one started (the app, before it
    /// stopped): taken at this screen's grid, as if this engine had.
    pub fn adopt(&self, started: impl IntoIterator<Item = String>) {
        self.here.lock().unwrap().started.extend(started);
    }

    /// Drops the line and dials again now (after a login, a network change).
    pub fn redial(&self) {
        self.redial.notify_one();
    }

    /// How a screen first shows `address`. `take`: every terminal is taken
    /// at its grid, as the screen shown last (as tmux gives a window to the
    /// latest client); the desktop's is drawn shrunk to fit a phone, too
    /// small to read. Else only one this engine started is (nobody else
    /// holds it; watched, it would first run at the core's headless size),
    /// and any other is watched.
    pub fn first_mode(&self, address: &str, take: bool) -> Mode {
        let here = self.here.lock().unwrap();
        match here.size {
            Some((cols, rows)) if take || here.started.contains(address) => Mode::Type { cols, rows },
            _ => Mode::Watch,
        }
    }

    /// A viewer for a terminal, on this engine's line.
    pub fn viewer(&self, address: &str, mode: Mode, palette: herdc_term::Palette) -> Viewer {
        Viewer::open(Seat { clients: self.clients(), view: self.view.clone(), here: self.here.clone(), shown: self.shown.subscribe() }, address, mode, palette, Value::Null)
    }

    fn job(&self, job: impl FnOnce(&mut State) + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }
}

/// Keeps a line to the core: dials, subscribes, forwards events, and after
/// a loss dials again (0.5 s, doubling to 10 s).
async fn line(jobs: mpsc::WeakUnboundedSender<Job>, connect: Connector, redial: Arc<Notify>) {
    let send = |job: Job| jobs.upgrade().is_some_and(|j| j.send(job).is_ok());
    let mut backoff = Duration::from_millis(500);
    loop {
        let client = tokio::select! {
            c = connect() => c,
            _ = redial.notified() => continue,
        };
        let client = match client {
            Ok(c) => c,
            Err(error) => {
                tracing::warn!("cannot reach the core: {error}");
                if !send(Box::new(move |s| s.lost(error))) {
                    return;
                }
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = redial.notified() => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
        };
        backoff = Duration::from_millis(500);
        let home = exec::home(&client).await;
        let headless = exec::headless(&client).await;
        let c = client.clone();
        if !send(Box::new(move |s| {
            s.here.lock().unwrap().headless = Some(headless);
            s.connected(c, home)
        })) {
            return;
        }
        // Events until the line goes; a stream that ends alone is asked again.
        let error = loop {
            let (stream, snapshot) = match client.subscribe(json!({})).await {
                Ok(s) => s,
                Err(e) => break e.to_string(),
            };
            if !send(Box::new(move |s| s.subscribed(snapshot))) {
                return;
            }
            let lost = loop {
                let item = tokio::select! {
                    item = stream.next() => item,
                    _ = redial.notified() => break true,
                };
                let mut bytes = Vec::new();
                let mut over = None;
                for item in item.into_iter().chain(std::iter::from_fn(|| stream.try_next())) {
                    match item {
                        Item::Data(b) => bytes.extend(b),
                        Item::Resize { .. } => {}
                        Item::Closed(_) => over = Some(false),
                        Item::Lost => over = Some(true),
                    }
                }
                if !bytes.is_empty() && !send(Box::new(move |s| s.pushed(&bytes))) {
                    return;
                }
                if let Some(lost) = over {
                    break lost || client.is_closed();
                }
            };
            if lost {
                break "lost the connection to the core".to_owned();
            }
            if !send(Box::new(|s| s.unsubscribed())) {
                return;
            }
        };
        drop(client);
        if !send(Box::new(move |s| s.lost(error))) {
            return;
        }
        tokio::time::sleep(backoff).await;
    }
}
