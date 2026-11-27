//! The computer the app is on now: its engine, the world it publishes as
//! a signal, the notes it raises, and commands run with their questions
//! asked here.

use crate::link::{self, Computer};
use crate::i18n::tr_string;
use crate::ui::{self, Prompt};
use herdc_engine::tools::There;
use herdc_engine::{Engine, Note, View};
use herdc_ssh::Session;
use nsh_core::prelude::*;
use serde_json::{json, Value};
use std::sync::Arc;

/// Takes the words a prompt got (None: cancelled).
type Answer = Box<dyn FnOnce(Option<String>)>;

/// The agent a note says needs you, if it says so.
pub fn waits_on(note: &Note) -> Option<String> {
    let action = note.action.as_ref().filter(|_| note.alert == Some("blocked"))?;
    action.args["terminal"].as_str().map(str::to_owned)
}

#[derive(Clone)]
pub struct Live {
    pub computer: Computer,
    pub engine: Engine,
    pub view: Signal<Arc<View>>,
    pub notes: Signal<Vec<(u64, Note)>>,
    /// A command's question that wants words.
    pub asking: Signal<Option<Prompt>>,
    /// Who gets the words of the [`Prompt`] shown.
    answering: Stored<Option<Answer>>,
    /// What the line logs in with (typed again once refused).
    password: link::Password,
    /// What lives as long as the connection: its tasks and signals.
    owner: Owner,
    /// The last note's number: never given twice (a note gone, its
    /// timer still runs).
    noted: Stored<u64>,
}

/// The app's one connection, if any.
#[derive(Clone, Default)]
pub struct Current(pub Option<Live>);

/// Equal while it is the same engine on the same computer.
impl PartialEq for Current {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => a.computer == b.computer && a.view == b.view,
            (None, None) => true,
            _ => false,
        }
    }
}

pub fn current() -> Signal<Current> {
    global::<Current>()
}

/// The connection now (outside it, None).
pub fn live() -> Option<Live> {
    current().with(|c| c.0.clone())
}

/// Whether `view` is the computer's, read in full: until then, what it
/// showed last is drawn ([`link::Seen`]).
fn known(view: &View) -> bool {
    view.connected() && !view.machines.is_empty() && view.machines.iter().all(|m| m.layout != "loading")
}

/// Opens `computer` at space `at` (a space it showed last), once it is reached.
pub fn open_at(computer: Computer, at: String) {
    AT.with(|a| a.replace(Some(at)));
    open(computer, None);
}

thread_local! {
    /// The space [`open_at`] shows once the computer is known.
    static AT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Opens `computer`: its engine starts dialing, the app shows it, on the
/// space it showed last.
pub fn open(computer: Computer, first: Option<Session>) {
    if let Err(error) = switch_connection(computer, first, None) {
        link::storage_error(error);
    }
}

pub fn switch_connection(computer: Computer, first: Option<Session>, replacing: Option<&str>) -> Result<(), String> {
    let palette = untracked(crate::theme::palette);
    let cell = crate::terminal::Cell::at(12.0);
    // The pane's grid, near enough: the window below the header and tabs.
    let (w, h) = untracked(|| (window().width.0, window().height.0 - safe_area().top.0 - safe_area().bottom.0 - 88.0));
    let password: link::Password = Arc::new(std::sync::Mutex::new(computer.password.clone()));
    // Its choices (each space's tab, focus, …) read by the engine as it
    // starts: moved first when the connection was saved under another id.
    let was = replacing.and_then(|id| link::computers().into_iter().find(|c| c.id() == id));
    if let Some(was) = &was {
        link::carry_view(was, &computer);
    }
    let engine = link::open(&computer, first, link::display(&palette, (cell.width, cell.height)), cell.fit((w - 2.0 * crate::terminal::INSET_X, h - 2.0 * crate::terminal::INSET_Y)), password.clone())?;
    let previous = live();
    if let Some(old) = &previous { old.keep(); }
    let saved = match replacing {
        Some(id) => link::replace(id, &computer),
        None => link::remember(&computer),
    };
    if let Err(error) = saved {
        engine.close();
        return Err(error);
    }
    // Saved again: its choices (main agent, names, colors) move before its
    // page is made, which would give its spaces new colors meanwhile.
    if let (Some(was), Some(prefs)) = (&was, use_context::<crate::prefs::Prefs>()) {
        prefs.saved_again(was, &computer);
        // Another computer, without a main agent: its page would be empty.
        if prefs.lead(&computer.id()).is_none() && crate::home::shown().get_untracked() == crate::home::Home::Lead {
            crate::home::shown().set(crate::home::Home::Agents);
        }
    }
    if let Some(previous) = previous {
        previous.engine.close();
        previous.owner.dispose();
    }
    link::set_open(Some(&computer.id()));
    let id = computer.id();
    // Drawn until the computer is known: what it showed last.
    let seen = link::seen(&id);
    // The phone's terminals before it stopped are still its own.
    engine.adopt(seen.iter().flat_map(|s| s.started.clone()));
    let mut stale = seen.map(|s| s.view);
    // Opened again where it was: the space it showed last, unless told.
    let at = AT.with(|a| a.take()).or_else(|| stale.as_ref()?.spaces.iter().find(|s| s.active).map(|s| s.id.clone()));
    // On a space before, it opens on its workspace; else on the home page.
    let reopened = if replacing.is_some() {
        crate::nav::routes().with_untracked(|r| r.0.contains(&crate::nav::Route::Workspace))
    } else { at.is_some() };
    let drawn = move |stale: &Option<View>, fresh: Arc<View>| match stale {
        Some(old) => Arc::new(View { link: fresh.link.clone(), ..old.clone() }),
        None => fresh,
    };
    // The connection's own root, detached: its tasks outlive the page that
    // opened it (often gone by now: what is made under a disposed owner is
    // disposed at once).
    let (owner, live) = Owner::detached().run(|| nsh_core::element::create_root(move || {
        let live = Live {
            computer,
            engine: engine.clone(),
            view: signal(drawn(&stale, engine.view())),
            notes: signal(Vec::new()),
            asking: signal(None),
            answering: stored(None),
            password,
            owner: Owner::current(),
            noted: stored(0),
        };
        let published = live.view;
        let mut world = engine.watch();
        let (selects, mut at) = (live.clone(), at);
        // Selected again, the space it showed last; what was seen is drawn
        // till then (3 s at most), not the computer's own space a moment:
        // shown, a terminal there would be the phone's.
        let mut selecting: Option<(String, std::time::Instant)> = None;
        // Kept 10 s after it changed at most, and when the app goes (`Live::keep`).
        let changed = std::rc::Rc::new(std::cell::Cell::new(false));
        let keeper = (live.clone(), changed.clone());
        set_interval(std::time::Duration::from_secs(10), move || {
            if keeper.1.replace(false) {
                keeper.0.keep();
            }
        });
        spawn(async move {
            while world.changed().await.is_ok() {
                let fresh = world.borrow_and_update().clone();
                if known(&fresh) {
                    if let Some(space) = at.take().filter(|s| fresh.space(s).is_some_and(|s| !s.active)) {
                        selects.run("space.select", json!({ "id": space.clone() }));
                        selecting = Some((space, std::time::Instant::now()));
                    }
                    if selecting.as_ref().is_none_or(|(s, since)| fresh.space(s).is_none_or(|s| s.active) || since.elapsed() > std::time::Duration::from_secs(3)) {
                        selecting = None;
                        stale = None;
                        changed.set(true);
                    }
                }
                published.set(drawn(&stale, fresh));
            }
        });
        // The network back, the computer is dialed at once (the engine
        // would wait up to 10 s).
        let (lost, again, computer) = (live.view, engine.clone(), live.computer.clone());
        spawn(async move {
            loop {
                off(tokio::time::sleep(std::time::Duration::from_secs(1))).await;
                let network = lost.with_untracked(|v| matches!(&v.link, herdc_engine::Link::Lost { error } if link::Lost::of(error).network()));
                let computer = computer.clone();
                if network && off(async move { link::reachable(&computer).await }).await == Some(true) {
                    again.redial();
                    // Give the dial its time.
                    off(tokio::time::sleep(std::time::Duration::from_secs(10))).await;
                }
            }
        });
        let mut said = engine.notes();
        let shown = live.clone();
        spawn(async move {
            loop {
                match said.recv().await {
                    // Agents' news while the app is away isn't kept for
                    // later: the badges say it then.
                    Ok(note) if note.toast && (note.alert.is_none() || lifecycle() == nsh_core::platform::Lifecycle::Active) => shown.say(note),
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        });
        live
    }));
    debug_assert_eq!(owner, live.owner);
    // Opened afresh, it shows the main agent, when there is one.
    let lead = replacing.is_none() && crate::prefs::Prefs::kept_lead(&live.computer.id());
    if replacing.is_none() {
        crate::home::shown().set(if lead { crate::home::Home::Lead } else { crate::home::Home::Agents });
    }
    crate::nav::routes().set(crate::nav::Routes(if reopened && !lead { vec![crate::nav::Route::Workspace] } else { Vec::new() }));
    current().set(Current(Some(live)));
    Ok(())
}

/// Leaves the computer: its engine stops once nothing holds it. The app
/// starts on the computers then.
pub fn close() {
    link::set_open(None);
    if let Some(live) = live() {
        live.keep();
        live.engine.close();
        current().set(Current(None));
        live.owner.dispose();
    }
}

/// Runs `work` on the app's runtime (where the core's client lives) and
/// waits for it here; None when the app stopped it.
pub async fn off<T: Send + 'static>(work: impl std::future::Future<Output = T> + Send + 'static) -> Option<T> {
    background(work).await.ok()
}

impl Live {
    /// Keeps what the computer shows now, to draw first next time.
    pub fn keep(&self) {
        let now = self.engine.view();
        if known(&now) {
            link::keep_seen(&self.computer.id(), &now, self.engine.started());
        }
    }

    /// Asks for the computer's password again, after the one kept was
    /// refused; kept (sealed) and logged in with at once. The connection's,
    /// as [`Live::run`] is.
    pub fn ask_password(&self) {
        let live = self.clone();
        self.owner.run(|| spawn(async move { live.ask_password_now().await }));
    }

    async fn ask_password_now(&self) {
        let prompt = Prompt {
            title: t!("Password for {}", self.computer.id()),
            placeholder: t!("Password").into(),
            value: String::new(),
            done: t!("Connect"),
            masked: true,
        };
        let Some(words) = self.words(prompt).await.filter(|w| !w.is_empty()) else { return };
        if let Err(error) = link::update(&Computer { password: Some(words.clone()), ..self.computer.clone() }) {
            self.say(Note::error("Cannot save credentials", error));
            return;
        }
        *self.password.lock().unwrap() = Some(words);
        self.engine.redial();
    }

    /// A machine (None: the core's own) through the core, while connected.
    pub fn there(&self, on: Option<String>) -> Result<There, String> {
        self.engine.client().map(|core| There::new(core, on)).ok_or_else(|| "Not connected".to_owned())
    }

    /// Runs `work` as the connection's, not the page's: what started it
    /// (a button, redrawn as the work goes) may be gone before it ends.
    pub fn spawn(&self, work: impl std::future::Future<Output = ()> + 'static) {
        self.owner.run(|| spawn(work));
    }

    /// Runs a command; questions are asked, failures said. It is the
    /// connection's: a row or sheet that asked for it may be gone before
    /// its question is answered (the drawer closes on a pick).
    pub fn run(&self, name: &str, args: Value) {
        let (live, name) = (self.clone(), name.to_owned());
        self.owner.run(|| spawn(async move {
            let engine = live.engine.clone();
            let ran = {
                let name = name.clone();
                background(async move { engine.run(&name, args).await }).await
            };
            match ran {
                Ok(Ok(reply)) => live.answer(reply).await,
                Ok(Err(error)) => live.say(Note::error(t!("{} failed", name), error)),
                Err(_) => {}
            }
        }));
    }

    async fn answer(&self, reply: Value) {
        let Some(ask) = reply.get("ask") else { return };
        let (name, args) = (ask["command"]["name"].as_str().unwrap_or_default().to_owned(), ask["command"]["args"].clone());
        match ask["kind"].as_str() {
            Some("confirm") => {
                let title = ask["title"].as_str().map_or_else(|| t!("Sure?").to_owned(), tr_string);
                let detail = tr_string(ask["detail"].as_str().unwrap_or_default());
                let action = ask["action"].as_str().map_or_else(|| t!("OK").to_owned(), tr_string);
                if ui::confirm(&title, &detail, &action).await {
                    self.run(&name, args);
                }
            }
            Some("prompt") => {
                let field = ask["field"].as_str().unwrap_or("name").to_owned();
                let prompt = Prompt {
                    title: tr_string(ask["title"].as_str().unwrap_or_default()),
                    placeholder: tr_string(ask["hint"].as_str().unwrap_or_default()),
                    value: ask["value"].as_str().unwrap_or_default().to_owned(),
                    done: t!("OK"),
                    masked: false,
                };
                if let Some(words) = self.words(prompt).await {
                    let mut args = if args.is_object() { args } else { json!({}) };
                    args[field] = json!(words);
                    self.run(&name, args);
                }
            }
            _ => {}
        }
    }

    /// Asks for words; None when the person cancelled.
    pub async fn words(&self, prompt: Prompt) -> Option<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Some(earlier) = self.answering.replace(Some(Box::new(move |w| drop(tx.send(w))))) {
            earlier(None);
        }
        self.asking.set(Some(prompt));
        rx.await.ok().flatten()
    }

    /// The words the [`Prompt`] shown got.
    pub fn answered(&self, words: Option<String>) {
        if let Some(answer) = self.answering.replace(None) {
            answer(words);
        }
    }

    pub fn say(&self, mut note: Note) {
        note.title = tr_string(&note.title);
        note.body = note.body.as_deref().map(tr_string);
        let notes = self.notes;
        let id = self.noted.get() + 1;
        self.noted.replace(id);
        // An agent that needs you has one note, a while longer than others;
        // after it, the badges still say so.
        let waits = waits_on(&note);
        let lasts = if waits.is_some() { 10 } else { 5 };
        // It slides in, and fades away.
        crate::ui::smoothly();
        notes.update(|all| {
            all.retain(|(_, n)| waits.is_none() || waits_on(n) != waits);
            all.push((id, note));
        });
        // The connection's: the page that said it may close first.
        self.owner.run(|| {
            set_timeout(std::time::Duration::from_secs(lasts), move || {
                if notes.with_untracked(|all| all.iter().any(|(i, _)| *i == id)) {
                    crate::ui::smoothly();
                    notes.update(|all| all.retain(|(i, _)| *i != id));
                }
            })
        });
    }
}
