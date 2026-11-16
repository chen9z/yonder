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
mod tasks;
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
    session: Arc<tasks::Session>,
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
        let (session, tasks) = tasks::Session::new();
        let (jobs, mut queue) = mpsc::unbounded_channel::<Job>();
        let (view_tx, view) = watch::channel(Arc::new(View::default()));
        let (notes, _) = broadcast::channel(64);
        let (clients_tx, clients) = watch::channel(None);
        let redial = Arc::new(Notify::new());
        let out = Outlets { view: view_tx, notes: notes.clone(), clients: clients_tx };
        let mut state = State::new(jobs.clone(), out, options.store, tasks.clone());
        state.display = options.display;
        let here = Arc::new(Mutex::new(Here { size: options.size, ..Here::default() }));
        state.here = here.clone();

        let weak = jobs.downgrade();
        tasks.spawn(async move {
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
            // Stopping: the view as it is now, kept.
            state.save_view(true);
        });
        tasks.spawn(line(weak, connect, redial.clone()));
        Engine { session, jobs, view, notes, clients, redial, here, shown: Arc::new(watch::Sender::new(true)) }
    }

    /// Stops this session, including work retained by other handles.
    pub fn close(&self) {
        self.session.close();
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
        let (name, notes) = (name.to_owned(), self.notes.clone());
        let (tx, rx) = oneshot::channel();
        let command = name.clone();
        self.job(move |s| s.run(&command, args, Reply::new(tx)));
        self.session.tasks().spawn(async move {
            if let Ok(Err(error)) = rx.await {
                let _ = notes.send(Note::error(format!("{name} failed"), error));
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
            // Back in front: the machines, as they are now.
            if active && !s.active && s.core.is_some() {
                s.ask_machines();
            }
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
        Viewer::open(Seat { tasks: self.session.tasks(), clients: self.clients(), view: self.view.clone(), here: self.here.clone(), shown: self.shown.subscribe() }, address, mode, palette, Value::Null)
    }

    fn job(&self, job: impl FnOnce(&mut State) + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }
}

/// Keeps a line to the core: dials, subscribes, forwards events, and after
/// a loss dials again (0.5 s, doubling to 10 s, till a subscription takes).
/// A redial asked for while it dials or looks around dials again then: no
/// permit is left to end the next line that works.
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
        let probed = tokio::select! {
            probed = async { tokio::join!(exec::home(&client), exec::headless(&client)) } => Some(probed),
            _ = redial.notified() => None,
        };
        let Some((home, headless)) = probed else { continue };
        let c = client.clone();
        if !send(Box::new(move |s| {
            s.here.lock().unwrap().headless = Some(headless);
            s.connected(c, home)
        })) {
            return;
        }
        // Events until the line goes; a stream that ends alone is asked
        // again, after a moment.
        let mut first = true;
        let error = loop {
            if !first {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    _ = redial.notified() => break "lost the connection to the core".to_owned(),
                }
            }
            first = false;
            let (stream, snapshot) = match client.subscribe(json!({})).await {
                Ok(s) => s,
                Err(e) => break e.to_string(),
            };
            backoff = Duration::from_millis(500);
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
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = redial.notified() => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(10));
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    async fn stopped(mut view: watch::Receiver<Arc<View>>) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while view.changed().await.is_ok() {}
        }).await.expect("session must release the publisher");
    }

    #[tokio::test]
    async fn last_handle_stops_even_a_pending_connection() {
        let marker = Arc::new(());
        let weak = Arc::downgrade(&marker);
        let connector: Connector = Arc::new(move || {
            let _ = &marker;
            Box::pin(std::future::pending())
        });
        let engine = Engine::start(connector, Options::default());
        let view = engine.watch();
        let clone = engine.clone();
        drop(engine);
        assert!(view.has_changed().is_ok());
        drop(clone);
        stopped(view).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.upgrade().is_some() { tokio::task::yield_now().await; }
        }).await.expect("pending connector must be dropped");
    }

    #[tokio::test]
    async fn explicit_close_stops_retained_handles_and_rejects_commands() {
        let engine = Engine::start(Arc::new(|| Box::pin(std::future::pending())), Options::default());
        let other = engine.clone();
        let view = engine.watch();
        engine.close();
        stopped(view).await;
        let result = tokio::time::timeout(Duration::from_secs(2), other.run("space.new", serde_json::json!({}))).await.unwrap();
        assert!(result.is_err());
    }
}
