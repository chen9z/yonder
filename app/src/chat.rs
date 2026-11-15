//! The main agent as a conversation, drawn rather than shown as its
//! terminal: Pi's session read as it grows ([`transcript`]), the
//! person's words on the right, the agent's as they read, the tools it
//! used folded between them, the time where much of it passed. What is
//! written goes in as `hc send` puts it (no terminal held for it), with
//! pictures put beside the agent; working, it can be stopped; what it
//! asks in its terminal shows in the conversation, answered from there.
//! Pi's own commands are offered as `/` is written; a message of yours
//! is edited by going back to before it ([`transcript::rewind`]).

use crate::live::{off, Live};
use crate::nav::{self, Route};
use crate::{theme, ui};
use herdc_engine::tools::transcript::{self, Call, Change, Line, Said, Transcript};
use herdc_engine::{Note, Viewer};
use herdc_term::{Key, Mods};
use nsh_base::{text_view, TextViewLinks};
use nsh_core::element::Screen;
use nsh_core::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the session is read again: while the agent works, else.
const WORKING: Duration = Duration::from_secs(2);
const RESTING: Duration = Duration::from_secs(10);

/// Within this of the end, the list is at the end and stays there as
/// the conversation grows.
const NEAR_END: f32 = 48.0;

/// Lines further apart than this (ms) have the time said between them.
const APART: u64 = 30 * 60 * 1000;

/// A message the core took and the session never showed (Pi changed its
/// words) stops waiting after this.
const SENT_KEPT: Duration = Duration::from_secs(120);

/// Whether the main agent shows as a conversation (else its terminal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Talking(pub bool);

impl Default for Talking {
    fn default() -> Self {
        Talking(true)
    }
}

pub fn talking() -> Signal<Talking> {
    global::<Talking>()
}

/// Whether an agent can be shown as a conversation: Pi, whose sessions
/// are files.
pub fn readable(a: &herdc_engine::AgentView) -> bool {
    a.app.as_deref() == Some("pi") || a.program["key"] == "pi"
}

/// Where the session stands.
#[derive(Clone, Debug, PartialEq)]
enum Found {
    Looking,
    Read,
    /// No session file to read (another program, an old core): why.
    Not(String),
}

/// A message sent and not yet in the session: going, until the core
/// took it (`went`); first going `back` to before the one it replaces.
/// A `command` of Pi's shows apart, and not for long.
#[derive(Clone, Debug, PartialEq)]
struct Sent {
    n: u64,
    text: String,
    went: bool,
    at: Instant,
    back: bool,
    command: bool,
    /// Your messages shown when it went: the same words among them are
    /// older ones ("go on"), not this.
    known: Rc<HashSet<String>>,
}

/// Why it wasn't sent.
enum Failed {
    /// It didn't go back; nothing changed.
    Unchanged(String),
    /// It went back, then the words didn't go.
    Moved,
    Refused(String),
}

/// A command sent shows this long after it went.
const COMMAND_SHOWN: Duration = Duration::from_secs(3);

/// A message of yours being edited: its line, its words, and what was
/// written before (back in the field if let go).
#[derive(Clone, Debug, PartialEq)]
struct Editing {
    id: String,
    text: String,
    before: String,
}

/// Pi's own commands (1.5), as written after `/`.
const PI: &[&str] = &[
    "settings", "model", "tree", "thinking", "scoped-models", "export", "import", "share", "bug", "copy", "name", "session", "changelog", "hotkeys", "fork", "clone", "trust", "login", "logout", "new", "compact", "resume", "reload", "quit",
];

/// Those offered as `/` is written: what is of use from a phone.
const OFFERED: &[&str] = &["compact", "new", "name", "model", "thinking", "tree", "fork", "clone", "resume", "session", "export", "settings", "reload"];

/// What a command offered does, in a few words.
fn about(name: &str) -> &'static str {
    match name {
        "compact" => t!("Sum up the context so far"),
        "new" => t!("Start a new session"),
        "name" => t!("Name this session"),
        "model" => t!("Change the model"),
        "thinking" => t!("How hard it thinks"),
        "tree" => t!("Go back to an earlier point"),
        "fork" => t!("A new session from an earlier message"),
        "clone" => t!("Copy this session"),
        "resume" => t!("Open another session"),
        "session" => t!("This session's details"),
        "export" => t!("Save the session as a file"),
        "settings" => t!("Pi's settings"),
        _ => t!("Load extensions and skills again"),
    }
}

/// A command of Pi's, written: its name and what follows it.
fn command(text: &str) -> Option<(&str, &str)> {
    let said = text.trim().strip_prefix('/')?;
    if said.contains('\n') {
        return None;
    }
    let (name, rest) = said.split_once(' ').unwrap_or((said, ""));
    PI.contains(&name).then_some((name, rest.trim()))
}

/// Whether what a command does shows only in its terminal: a list to
/// choose from, what it says back.
fn shown_there(name: &str, rest: &str) -> bool {
    match name {
        "compact" | "new" | "name" | "clone" | "reload" => false,
        "model" | "thinking" => rest.is_empty(),
        _ => true,
    }
}

/// A row of the conversation: the time, or a thing said.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Time(String, u64),
    Said(Line),
}

impl Item {
    fn key(&self) -> String {
        match self {
            Item::Time(id, _) => id.clone(),
            Item::Said(l) => l.id.clone(),
        }
    }
}

/// The lines as rows: the time before the first, and before any said
/// long after the one before it.
fn items(lines: &[Line]) -> Vec<Item> {
    let mut out = Vec::with_capacity(lines.len() + 4);
    let mut last = 0;
    for l in lines {
        if l.at > 0 && (last == 0 || l.at.saturating_sub(last) > APART) {
            out.push(Item::Time(format!("time:{}", l.id), l.at));
        }
        if l.at > 0 {
            last = l.at;
        }
        out.push(Item::Said(l.clone()));
    }
    out
}

/// The machine the agent at `address` runs on (None: the core's own).
fn machine_of(live: &Live, address: &str) -> Option<String> {
    live.view.with_untracked(|v| v.agents.iter().find(|a| a.address == address).and_then(|a| a.machine.clone()))
}

/// The conversation with the agent at `address`; `name` says whose it is.
pub fn chat(live: Live, address: String, name: String) -> impl IntoElement {
    let world = live.view;
    let lines = signal(Vec::<Line>::new());
    let cut = signal(false);
    let found = signal(Found::Looking);
    let sent = signal(Vec::<Sent>::new());
    // A read wanted now (a message went in, it was stopped).
    let again = signal(0u32);
    // Earlier lines asked for, and being read.
    let earlier = signal(0u32);
    let loading = signal(false);
    let status = {
        let address = address.clone();
        computed(move || world.with(|v| v.agents.iter().find(|a| a.address == address).map_or("idle", |a| a.status)))
    };
    let list = element_ref();
    // At the end, following it; else where it was left (`away`), with
    // more under it unseen. Earlier lines put in above keep the place:
    // the height and offset before.
    let stuck = stored(true);
    let away = signal(false);
    let unseen = signal(false);
    let offset = stored(0.0f32);
    let height = stored(0.0f32);
    let keep = stored(None::<(f32, f32)>);
    {
        let (live, address) = (live.clone(), address.clone());
        spawn(async move {
            let mut read = Transcript::default();
            let mut path: Option<(String, String)> = None;
            let mut seen = (u64::MAX, String::new(), 0u32);
            let mut earlier_seen = 0u32;
            let mut last = Instant::now() - RESTING;
            loop {
                let (label, cwd, turn, word) = world.with_untracked(|v| {
                    v.terminal(&address)
                        .map(|t| (t["labels"]["alma.session"].as_str().unwrap_or_default().to_owned(), t["cwd"].as_str().unwrap_or_default().to_owned(), t["turn"].as_u64().unwrap_or(0), t["status"].as_str().unwrap_or_default().to_owned()))
                        .unwrap_or_default()
                });
                let on = machine_of(&live, &address);
                // Named by its label; without one, looked for again each
                // turn (a `/new` shows only as a newer file).
                let key = if label.is_empty() { format!("turn {turn}") } else { label.clone() };
                let now = (turn, word.clone(), again.get_untracked());
                let every = if word == "working" { WORKING } else { RESTING };
                let due = now != seen || last.elapsed() >= every;
                let shown = lifecycle() == nsh_core::platform::Lifecycle::Active;
                if due && shown && let Ok(there) = live.there(on.clone()) {
                    seen = now;
                    last = Instant::now();
                    // Another session (`/new`, a restart): read afresh.
                    if path.as_ref().is_none_or(|(k, _)| *k != key) {
                        let (t, l, c) = (there.clone(), label.clone(), cwd.clone());
                        match off(async move { transcript::find(&t, Some(&l), &c).await }).await {
                            Some(Ok(Some(file))) => {
                                if path.as_ref().is_none_or(|(_, f)| *f != file) {
                                    read = Transcript::default();
                                }
                                path = Some((key.clone(), file));
                            }
                            // Named and not written yet (a Pi just started): nothing said yet.
                            Some(Ok(None)) if !label.is_empty() => found.set(Found::Read),
                            Some(Ok(None)) if path.is_none() => found.set(Found::Not(t!("No Pi session found for it.").into())),
                            Some(Err(e)) if path.is_none() => found.set(Found::Not(e)),
                            _ => {}
                        }
                    }
                    if let Some((_, file)) = path.clone() {
                        let (t, from) = (there.clone(), read.read);
                        match off(async move { transcript::read(&t, &file, from).await }).await {
                            Some(Ok(got)) => {
                                // Shorter than read: made again, read it all again.
                                if got.lines < read.read {
                                    read = Transcript::default();
                                    seen.0 = u64::MAX;
                                    continue;
                                }
                                let first = found.get_untracked() != Found::Read;
                                if first || !got.text.is_empty() {
                                    read.take(&got);
                                    let now = read.lines();
                                    if !first {
                                        ui::smoothly();
                                    }
                                    lines.set(now);
                                    cut.set(read.cut());
                                    found.set(Found::Read);
                                }
                            }
                            Some(Err(e)) => {
                                if found.get_untracked() != Found::Read {
                                    found.set(Found::Not(e));
                                }
                                path = None;
                            }
                            None => {}
                        }
                    }
                }
                // Earlier lines, asked for: put in above, the place kept.
                let wanted = earlier.get_untracked();
                if wanted != earlier_seen {
                    earlier_seen = wanted;
                    if let (Some((_, file)), Ok(there)) = (path.clone(), live.there(on)) {
                        let before = read.start;
                        if let Some(Ok(got)) = off(async move { transcript::earlier(&there, &file, before).await }).await {
                            read.take(&got);
                            keep.replace(Some((height.get(), offset.get())));
                            lines.set(read.lines());
                            cut.set(read.cut());
                        }
                    }
                    loading.set(false);
                }
                // What was sent and shows now is no longer waiting: once the
                // core took it, a message of yours not there when it went.
                let shows = |m: &Sent| m.went && lines.with_untracked(|l| l.iter().any(|l| matches!(&l.said, Said::You { text, .. } if text == m.text.trim()) && !m.known.contains(&l.id)));
                if sent.with_untracked(|s| s.iter().any(|m| !m.command && shows(m))) {
                    sent.update(|s| s.retain(|m| m.command || !shows(m)));
                }
                // Taken by the core long ago and never shown: let go.
                if sent.with_untracked(|s| s.iter().any(|m| m.went && m.at.elapsed() > SENT_KEPT)) {
                    sent.update(|s| s.retain(|m| !(m.went && m.at.elapsed() > SENT_KEPT)));
                }
                off(tokio::time::sleep(Duration::from_millis(300))).await;
            }
        });
    }

    // Asked something in its terminal: the question's viewer, while shown.
    let answering = stored(None::<Rc<Viewer>>);
    // Kept with what is written (the page is made again as tabs change),
    // and let go with it, for another agent.
    let editing = global::<Option<Editing>>();
    if crate::terminal::draft_for(&address) {
        editing.set(None);
    }
    // Going back to before a message: nothing else goes to it meanwhile
    // (Pi's keys are pressed for it, its screen read).
    let going_back = move || sent.with_untracked(|s| s.iter().any(|m| m.back && !m.went));
    let wait = {
        let live = live.clone();
        move |text: &str| {
            crate::terminal::unsent(text);
            live.say(Note::info(t!("A moment"), t!("It is still going back to before your message.")));
        }
    };
    // The conversation's: what a button starts outlives the button (Send
    // becomes Stop as the field empties while it works).
    let owner = Owner::current();
    let send = {
        let (live, address) = (live.clone(), address.clone());
        let wait = wait.clone();
        move |text: String| {
            stuck.replace(true);
            if going_back() {
                wait(&text);
                return;
            }
            // Edited while it asks something: an answer first, not this.
            if status.get_untracked() == "blocked" && editing.with_untracked(Option::is_some) {
                crate::terminal::unsent(&text);
                live.say(Note::info(t!("It is asking something"), t!("Answer it first, then edit.")));
                return;
            }
            // Waiting for an answer: what is written answers it there.
            if status.get_untracked() == "blocked" && let Some(viewer) = answering.get() {
                viewer.paste(&text);
                owner.run(|| set_timeout(crate::terminal::ENTER_AFTER, move || viewer.key(Key::Enter, Mods::NONE)));
                return;
            }
            let busy = status.get_untracked() == "working";
            // Edited: Pi goes back to before the message, with how many
            // of yours came after it, then takes the words instead.
            let back = match editing.get_untracked() {
                None => None,
                Some(e) => {
                    let later = lines.with_untracked(|l| {
                        let at = l.iter().position(|l| l.id == e.id)?;
                        Some(l[at + 1..].iter().filter(|l| matches!(l.said, Said::You { .. })).count())
                    });
                    let Some(later) = later else {
                        editing.set(None);
                        crate::terminal::unsent(&text);
                        live.say(Note::error(t!("Not changed"), t!("That message isn't in the conversation any more.")));
                        return;
                    };
                    if busy {
                        crate::terminal::unsent(&text);
                        live.say(Note::info(t!("Still working"), t!("Edit it once it's done, or stop it first.")));
                        return;
                    }
                    editing.set(None);
                    Some((e, later))
                }
            };
            let command = back.is_none().then(|| command(&text).map(|(name, rest)| shown_there(name, rest))).flatten();
            if command.is_some() && busy {
                crate::terminal::unsent(&text);
                live.say(Note::info(t!("Still working"), t!("Commands go in once it's done.")));
                return;
            }
            let n = sent.with_untracked(|s| s.last().map_or(0, |m| m.n + 1));
            ui::smoothly();
            let known = Rc::new(lines.with_untracked(|l| l.iter().filter(|l| matches!(l.said, Said::You { .. })).map(|l| l.id.clone()).collect()));
            sent.update(|s| s.push(Sent { n, text: text.clone(), went: false, at: Instant::now(), back: back.is_some(), command: command.is_some(), known }));
            let (live, address) = (live.clone(), address.clone());
            owner.run(|| spawn(async move {
                let (words, going) = (text.clone(), back.clone());
                let went = match live.there(machine_of(&live, &address)) {
                    Ok(there) => off(async move {
                        if let Some((e, later)) = going {
                            transcript::rewind(&there, &address, &e.text, later).await.map_err(|why| match why {
                                transcript::Rewound::Unchanged(why) => Failed::Unchanged(why),
                                transcript::Rewound::Moved(_) => Failed::Moved,
                            })?;
                        }
                        transcript::send(&there, &address, &words).await.map_err(Failed::Refused)
                    })
                    .await
                    .unwrap_or_else(|| Err(Failed::Refused("Not connected".into()))),
                    Err(e) => Err(Failed::Refused(e)),
                };
                match went {
                    Ok(()) => {
                        sent.update(|s| s.iter_mut().filter(|m| m.n == n).for_each(|m| {
                            m.went = true;
                            m.at = Instant::now();
                        }));
                        again.update(|a| *a += 1);
                        // A command: what it shows is in its terminal, or
                        // in the conversation as it changes; the line goes.
                        if let Some(there) = command {
                            if there {
                                ui::smoothly();
                                talking().set(Talking(false));
                            }
                            off(tokio::time::sleep(COMMAND_SHOWN)).await;
                            sent.update(|s| s.retain(|m| m.n != n));
                        }
                    }
                    // Not sent: back in the field, and why.
                    Err(not) => {
                        sent.update(|s| s.retain(|m| m.n != n));
                        crate::terminal::unsent(&text);
                        again.update(|a| *a += 1);
                        match not {
                            // Still being edited, as it was.
                            Failed::Unchanged(why) => {
                                editing.set(back.map(|(e, _)| e));
                                live.say(Note::error(t!("Couldn't edit it"), why));
                            }
                            // Gone back: the words, sent now, go on from there.
                            Failed::Moved => live.say(Note::error(t!("Not sent"), t!("It went back to before that message, but your words didn't go: send them again."))),
                            Failed::Refused(why) => {
                                let why = if why == "blocked" { t!("It is waiting for an answer in its terminal.").to_owned() } else { why };
                                live.say(Note::error(t!("Not sent"), why));
                            }
                        }
                    }
                }
            }));
        }
    };
    let working = computed(move || status.get() == "working");
    let stop: crate::terminal::Act = {
        let (live, address) = (live.clone(), address.clone());
        Rc::new(move || {
            let (live, address) = (live.clone(), address.clone());
            owner.run(|| spawn(async move {
                let Ok(there) = live.there(machine_of(&live, &address)) else { return };
                match off(async move { transcript::interrupt(&there, &address).await }).await {
                    Some(Ok(outcome)) if outcome == "still_working" => live.say(Note::info(t!("Still working"), t!("It didn't stop yet; its terminal shows why."))),
                    Some(Err(e)) => live.say(Note::error(t!("Couldn't stop it"), e)),
                    _ => {}
                }
                again.update(|a| *a += 1);
            }));
        })
    };
    // What it is doing now: the step it took last, not done yet.
    let doing = computed(move || {
        lines.with(|l| match l.last().map(|l| &l.said) {
            Some(Said::Tools(calls)) => calls.last().filter(|c| c.result.is_none()).map(|c| format!("{} {}", c.name, c.said)),
            _ => None,
        })
    });

    let placeholder = {
        let name = name.clone();
        move || if status.get() == "blocked" { t!("Type an answer").to_owned() } else { t!("Message {} · / for commands", name.clone()) }
    };
    // Held, a message of yours offers to be edited: back in the field.
    let edit: Edit = {
        let live = live.clone();
        Rc::new(move |id: String, text: String| {
            if going_back() {
                live.say(Note::info(t!("A moment"), t!("It is still going back to before your message.")));
                return;
            }
            if status.get_untracked() != "idle" && status.get_untracked() != "done" {
                live.say(Note::info(t!("Still working"), t!("Edit it once it's done, or stop it first.")));
                return;
            }
            let before = editing.get_untracked().map_or_else(crate::terminal::written, |e| e.before);
            ui::smoothly();
            editing.set(Some(Editing { id, text: text.clone(), before }));
            crate::terminal::rewrite(&text);
        })
    };
    // Written `/` and some of a name: Pi's commands it may be.
    let offered = computed(move || {
        let written = crate::terminal::draft();
        let typed = written.strip_prefix('/').filter(|t| !t.contains(char::is_whitespace));
        match typed {
            Some(typed) if editing.with(Option::is_none) && status.get() != "blocked" => OFFERED.iter().copied().filter(|c| c.starts_with(typed)).collect(),
            _ => Vec::new(),
        }
    });
    let offers = crate::terminal::Offers { stop: Some((working, stop)), attach: Some(attach(address.clone())) };
    let asked = (live.clone(), address.clone(), name.clone());
    view()
        .flex_1()
        .child(dynamic(move || match found.get() {
            Found::Looking => view().flex_1().items_center().justify_center().child(ui::glyph("spinner", 18.0, theme::DIM).animate_spin()).into_element(),
            Found::Not(why) => missing(why).into_element(),
            Found::Read => {
                let asked = asked.clone();
                view()
                    .flex_1()
                    .child(
                        scroll()
                            .flex_1()
                            .element_ref(&list)
                            .keyboard_dismiss_mode(KeyboardDismissMode::OnDrag)
                            .on_scroll(move |e: &ScrollEvent| {
                                offset.replace(e.offset.y.0);
                                let end = e.offset.y.0 + e.size.height.0 >= e.content_size.height.0 - NEAR_END;
                                stuck.replace(end);
                                if away.get_untracked() == end {
                                    away.set(!end);
                                }
                                if end && unseen.get_untracked() {
                                    unseen.set(false);
                                }
                            })
                            // The keyboard up, the list shorter: still at the end.
                            .on_layout(move |_: &Bounds<Points>| if stuck.get() { list.scroll_to_end(false) })
                            .child(
                                view()
                                    // Grown: at the end, if it was; else more is unseen. Grown
                                    // above (earlier lines), where it was.
                                    .on_layout(move |b: &Bounds<Points>| {
                                        let now = b.size.height.0;
                                        let was = height.replace(now);
                                        if let Some((h, y)) = keep.replace(None) {
                                            list.scroll_to(point(px(0.0), px(y + now - h)), false);
                                        } else if stuck.get() {
                                            list.scroll_to_end(false);
                                        } else if now > was + 1.0 {
                                            unseen.set(true);
                                        }
                                    })
                                    .px(px(16.0))
                                    .pt(px(8.0))
                                    .pb(px(16.0))
                                    .gap(px(14.0))
                                    .child(when(move || cut.get(), move || before(loading, earlier)))
                                    .child({
                                        let edit = edit.clone();
                                        each(move || lines.with(|l| items(l)), Item::key, move |i, _| item(i, edit.clone()))
                                    })
                                    .child(each(move || sent.get(), |m| m.n, move |m, _| waiting(m, working)))
                                    .child(state(asked.0.clone(), asked.1.clone(), status, doing, asked.2.clone(), answering)),
                            ),
                    )
                    .child(when(move || away.get(), move || {
                        jump(unseen, move || {
                            stuck.replace(true);
                            away.set(false);
                            unseen.set(false);
                            list.scroll_to_end(true);
                        })
                    }))
                    .into_element()
            }
        }))
        .child(when(move || !offered.with(Vec::is_empty), move || commands(offered)))
        .child(when(move || editing.with(Option::is_some), move || edited(editing)))
        .child(crate::terminal::talk_bar(placeholder, send, offers))
}

/// Edits a message of yours: its line, its words.
type Edit = Rc<dyn Fn(String, String)>;

/// Commands shown at most, the keyboard up or not: more letters find
/// the others.
const COMMANDS_SHOWN: (usize, usize) = (5, 8);

/// Above the field, as `/` and a name are written: the commands of Pi's
/// it may be, chosen into the field (those wanting words, with a space
/// for them); a glyph says it shows in its terminal. Not a scroll: one
/// takes the touch from the keyboard, which then closes, the press lost.
fn commands(offered: Memo<Vec<&'static str>>) -> impl IntoElement {
    let shown = move || {
        let most = if keyboard_height().0 > 0.0 { COMMANDS_SHOWN.0 } else { COMMANDS_SHOWN.1 };
        offered.with(|o| o.iter().copied().take(most).collect::<Vec<_>>())
    };
    view()
        .border_t(px(1.0))
        .border_color(theme::BORDER)
        .bg(theme::CHROME)
        .test_id("commands")
        .child(each(shown, |c| *c, |c, _| {
            let name = c.get_untracked();
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .h(px(48.0))
                .px(px(16.0))
                .active(|s| s.bg(theme::HOVER))
                .role(AccessibilityRole::Button)
                .label(format!("/{name}"))
                .test_id(&format!("cmd:{name}"))
                .on_press(move || crate::terminal::rewrite(&if name == "name" { format!("/{name} ") } else { format!("/{name}") }))
                .child(text(format!("/{name}")).text_size(theme::ROW).font_family(theme::MONO).text_color(theme::FOREGROUND))
                .child(text(about(name)).flex_1().text_size(theme::FOOTNOTE).text_color(theme::MUTED).number_of_lines(1))
                .child(if shown_there(name, "") { ui::glyph("terminal", 14.0, theme::DIM).into_element() } else { view().into_element() })
        }))
}

/// Above the field while a message of yours is edited: which, and the
/// way out of it (what was written before, back).
fn edited(editing: Signal<Option<Editing>>) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .pl(px(16.0))
        .pr(px(6.0))
        .py(px(6.0))
        .border_t(px(1.0))
        .border_color(theme::BORDER)
        .bg(theme::CHROME)
        .test_id("editing")
        .child(ui::glyph("pencil", 15.0, theme::ACCENT))
        .child(
            view()
                .flex_1()
                .child(text(t!("Editing your message")).text_size(theme::FOOTNOTE).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT))
                .child(text(move || editing.with(|e| e.as_ref().map(|e| e.text.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default())).text_size(theme::CAPTION).text_color(theme::DIM).number_of_lines(1)),
        )
        .child(
            view()
                .w(px(36.0))
                .h(px(36.0))
                .rounded(px(18.0))
                .items_center()
                .justify_center()
                .active(|s| s.bg(theme::HOVER))
                .role(AccessibilityRole::Button)
                .label(t!("Cancel editing"))
                .test_id("unedit")
                .on_press(move || {
                    if let Some(e) = editing.get_untracked() {
                        ui::smoothly();
                        editing.set(None);
                        crate::terminal::rewrite(&e.before);
                    }
                })
                .child(ui::glyph("x", 16.0, theme::MUTED)),
        )
}

/// Pictures from the phone, put beside the agent at `address` on its
/// machine, their paths added to what is written (the agent reads them
/// from there); whether they are going.
pub fn attach(address: String) -> (Signal<bool>, crate::terminal::Act) {
    let busy = signal(false);
    // Its caller's: the field may be drawn again while they go.
    let owner = Owner::current();
    let go = Rc::new(move || {
        let address = address.clone();
        owner.run(|| spawn(async move {
            let mut request = nsh_core::picker::PickRequest::new(true, 4);
            // Larger says no more to a model, and takes longer to go.
            request.max_dimension = Some(DevicePixels(2048));
            let picked = pick_images(request).await;
            let Some(live) = crate::live::live() else { return };
            if picked.is_empty() {
                return;
            }
            busy.set(true);
            for file in picked {
                let there = match live.there(machine_of(&live, &address)) {
                    Ok(there) => there,
                    Err(e) => {
                        live.say(Note::error(t!("Picture not sent"), e));
                        break;
                    }
                };
                let (name, bytes, address) = (file.name.clone(), file.data.bytes().to_vec(), address.clone());
                match off(async move { transcript::put(&there, &address, &name, &bytes).await }).await {
                    Some(Ok(path)) => crate::terminal::add_to_draft(&path),
                    Some(Err(e)) => {
                        live.say(Note::error(t!("Picture not sent"), e));
                        break;
                    }
                    None => break,
                }
            }
            busy.set(false);
        }));
    });
    (busy, go)
}

/// Above the lines read: the way to those before them.
fn before(loading: Signal<bool>, earlier: Signal<u32>) -> impl IntoElement {
    view()
        .self_center()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(14.0))
        .h(px(32.0))
        .rounded(px(16.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(t!("Show earlier messages"))
        .disabled(move || loading.get())
        .on_press(move || {
            loading.set(true);
            earlier.update(|e| *e += 1);
        })
        .child(when(move || loading.get(), || ui::glyph("spinner", 13.0, theme::DIM).animate_spin()))
        .child(text(t!("Show earlier messages")).text_size(theme::FOOTNOTE).text_color(theme::ACCENT))
}

/// Down to the newest, over the list scrolled up; a dot while more came.
fn jump(unseen: Signal<bool>, press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .absolute()
        .right(px(16.0))
        .bottom(px(12.0))
        .w(px(40.0))
        .h(px(40.0))
        .rounded(px(20.0))
        .items_center()
        .justify_center()
        .bg(theme::POPOVER)
        .border_1()
        .border_color(theme::BORDER)
        .active(|s| s.scale(ui::PRESSED))
        .transition()
        .duration(ui::QUICK)
        .role(AccessibilityRole::Button)
        .label(move || if unseen.get() { t!("New messages").to_owned() } else { t!("Go to the newest").to_owned() })
        .test_id("jump")
        .on_press(press)
        .child(ui::glyph("arrow-down", 20.0, theme::FOREGROUND))
        .child(when(move || unseen.get(), || {
            view().absolute().top(px(2.0)).right(px(2.0)).w(px(10.0)).h(px(10.0)).rounded(px(5.0)).bg(theme::ACCENT).pointer_events(PointerEvents::None)
        }))
}

/// No session to read: why, and the terminal instead.
fn missing(why: String) -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .px(px(32.0))
        .gap(px(12.0))
        .child(text(t!("Can't show it as a conversation")).text_size(theme::HEADLINE).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND).text_center())
        .child(text(why).text_size(theme::CALLOUT).text_color(theme::MUTED).text_center().number_of_lines(4))
        .child(view().h(px(4.0)))
        .child(ui::ghost_button(t!("Show the terminal"), || talking().set(Talking(false))))
}

/// A row: the time, or what was said.
fn item(i: ReadSignal<Item>, edit: Edit) -> impl IntoElement {
    match i.get_untracked() {
        Item::Time(_, at) => text(time_of(at)).self_center().pt(px(4.0)).text_size(theme::CAPTION).font_weight(FontWeight::MEDIUM).text_color(theme::DIM).into_element(),
        Item::Said(_) => {
            let line = computed(move || match i.get() {
                Item::Said(l) => l,
                Item::Time(id, at) => Line { id, said: Said::Stopped, at },
            });
            said(line, edit).into_element()
        }
    }
}

/// One thing said.
fn said(l: Memo<Line>, edit: Edit) -> impl IntoElement {
    let kind = computed(move || l.with(|l| std::mem::discriminant(&l.said)));
    dynamic(move || {
        let _ = kind.get();
        match l.with_untracked(|l| l.said.clone()) {
            Said::You { .. } => {
                let edit = edit.clone();
                let edit = move |text| edit(l.with_untracked(|l| l.id.clone()), text);
                you(move || l.with(|l| match &l.said { Said::You { text, .. } => text.clone(), _ => String::new() }), move || l.with(|l| match &l.said { Said::You { images, .. } => *images, _ => 0 }), edit).into_element()
            }
            Said::Reply(_) => text_view()
                .markdown(move || l.with(|l| match &l.said { Said::Reply(words) => Some(SharedString::from(words.clone())), _ => None }))
                .selectable(true)
                .links(TextViewLinks::Open)
                .text_size(theme::ROW)
                .line_height(px(23.0))
                .text_color(theme::FOREGROUND)
                .into_element(),
            Said::Tools(_) => tools(move || l.with(|l| match &l.said { Said::Tools(calls) => calls.clone(), _ => Vec::new() })).into_element(),
            Said::Compacted => view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(view().flex_1().h(px(1.0)).bg(theme::BORDER))
                .child(text(t!("Earlier context summed up")).text_size(theme::CAPTION).text_color(theme::DIM))
                .child(view().flex_1().h(px(1.0)).bg(theme::BORDER))
                .into_element(),
            Said::Failed(why) => view()
                .flex_row()
                .gap(px(8.0))
                .child(ui::glyph("x", 14.0, theme::BLOCKED).mt(px(2.0)))
                .child(text(t!("Failed: {}", why)).flex_1().text_size(theme::FOOTNOTE).text_color(theme::BLOCKED).number_of_lines(3))
                .into_element(),
            Said::Stopped => text(t!("Stopped")).text_size(theme::FOOTNOTE).text_color(theme::DIM).into_element(),
            Said::Changed(change) => text(match change {
                Change::Name(name) => t!("Named “{}”", name),
                Change::Model(model) => t!("Model: {}", model),
                Change::Thinking(level) => t!("Thinking: {}", level),
            })
            .self_center()
            .text_size(theme::CAPTION)
            .text_color(theme::DIM)
            .into_element(),
        }
    })
}

/// The person's words, on the right in a bubble; held, edited (Pi goes
/// back to before them), copied, or put in the field again.
fn you(words: impl Fn() -> String + Clone + 'static, images: impl Fn() -> usize + 'static, edit: impl Fn(String) + Clone + 'static) -> View {
    let held = words.clone();
    view().flex_row().justify_end().pl(px(48.0)).child(
        bubble(words, images)
            .active(|s| s.opacity(0.8))
            .role(AccessibilityRole::Button)
            .on_long_press(move || {
                let (words, edit) = (held(), edit.clone());
                spawn(async move {
                    use crate::pick::{self, Item};
                    let mut items = vec![Item::glyph("copy", t!("Copy"), "copy"), Item::glyph("again", t!("Write it again"), "corner-down-left")];
                    if !words.trim().is_empty() {
                        items.insert(0, Item::glyph("edit", t!("Edit"), "pencil"));
                    }
                    match pick::one(t!("Your message"), items).await.as_deref() {
                        Some("edit") => edit(words),
                        Some("copy") => {
                            set_clipboard(words);
                            if let Some(live) = crate::live::live() {
                                live.say(Note::info(t!("Copied"), t!("The message is on the clipboard.")));
                            }
                        }
                        Some("again") => crate::terminal::add_to_draft(&words),
                        _ => {}
                    }
                });
            }),
    )
}

fn bubble(words: impl Fn() -> String + 'static, images: impl Fn() -> usize + 'static) -> View {
    view()
        .flex_shrink(1.0)
        .gap(px(4.0))
        .px(px(14.0))
        .py(px(9.0))
        .rounded(px(18.0))
        .bg(theme::RAISED)
        .child(dynamic(move || match images() {
            0 => view().into_element(),
            n => view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(ui::glyph("image", 13.0, theme::MUTED))
                .child(text(t!("{} images", n)).text_size(theme::CAPTION).text_color(theme::MUTED))
                .into_element(),
        }))
        .child(text(words).text_size(theme::ROW).line_height(px(21.0)).text_color(theme::FOREGROUND))
}

/// A message sent, not in the session yet: quieter, saying how it goes.
fn waiting(m: ReadSignal<Sent>, working: Memo<bool>) -> impl IntoElement {
    // A command: a line of its own, quiet.
    if m.with_untracked(|m| m.command) {
        return text(move || m.with(|m| format!("{} · {}", m.text.trim(), if m.went { t!("Sent") } else { t!("Sending…") })))
            .self_center()
            .text_size(theme::CAPTION)
            .font_family(theme::MONO)
            .text_color(theme::DIM)
            .into_element();
    }
    view()
        .items_end()
        .pl(px(48.0))
        .gap(px(4.0))
        .child(bubble(move || m.with(|m| m.text.clone()), || 0).opacity(0.6))
        .child(
            text(move || match (m.with(|m| m.went), working.get()) {
                (false, _) if m.with(|m| m.back) => t!("Going back to before it…").to_owned(),
                (false, _) => t!("Sending…").to_owned(),
                (true, true) => t!("Queued: it reads it once this step is done").to_owned(),
                (true, false) => t!("Sent").to_owned(),
            })
            .pr(px(6.0))
            .text_size(theme::CAPTION)
            .text_color(theme::DIM),
        )
        .into_element()
}

/// The glyph for a tool.
fn icon(name: &str) -> &'static str {
    match name {
        "bash" | "!" => "terminal",
        "read" => "file",
        "write" | "edit" => "pencil",
        "grep" | "find" | "ls" | "ffgrep" | "fffind" | "web_search" | "source_check" => "search",
        "fetch_content" | "get_search_content" => "external-link",
        "Agent" | "SubagentWorkflow" | "subagent" => "layers",
        _ => "command",
    }
}

/// Tools used one after another: a line saying so, opened on each; one
/// pressed shows what it was given and gave back.
fn tools(calls: impl Fn() -> Vec<Call> + Copy + 'static) -> impl IntoElement {
    let open = signal(false);
    let count = move || calls().len();
    let failed = move || calls().iter().filter(|c| c.error).count();
    let summary = move || {
        let calls = calls();
        match calls.as_slice() {
            [one] => format!("{} {}", one.name, one.said),
            many => {
                let mut names: Vec<&str> = Vec::new();
                for c in many {
                    if !names.contains(&c.name.as_str()) {
                        names.push(&c.name);
                    }
                }
                format!("{} · {}", t!("{} steps", many.len()), names.join(", "))
            }
        }
    };
    view()
        .rounded(px(12.0))
        .border_1()
        .border_color(theme::BORDER)
        .overflow_hidden()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .h(px(38.0))
                .active(|s| s.bg(theme::HOVER))
                .role(AccessibilityRole::Button)
                .label(summary)
                .on_press(move || {
                    if count() == 1 {
                        step(&calls()[0]);
                    } else {
                        ui::smoothly();
                        open.update(|o| *o = !*o);
                    }
                })
                .child(dynamic(move || ui::glyph(icon(&calls().first().map(|c| c.name.clone()).unwrap_or_default()), 14.0, theme::MUTED).into_element()))
                .child(text(summary).flex_1().text_size(theme::FOOTNOTE).font_family(theme::MONO).text_color(theme::MUTED).number_of_lines(1))
                .child(when(move || failed() > 0, || ui::dot(|| Some("blocked"))))
                .child(dynamic(move || if count() > 1 { ui::chevron(move || open.get(), 14.0).into_element() } else { ui::glyph("chevron-right", 14.0, theme::DIM).into_element() })),
        )
        .child(when(move || open.get(), move || {
            view().border_t(px(1.0)).border_color(theme::BORDER).py(px(4.0)).child(each(calls, |c| c.id.clone(), |c, _| call_row(c)))
        }))
}

/// One tool used, in the opened list.
fn call_row(c: ReadSignal<Call>) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .h(px(34.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(move || c.with(|c| format!("{} {}", c.name, c.said)))
        .on_press(move || step(&c.get_untracked()))
        .child(dynamic(move || ui::glyph(icon(&c.with(|c| c.name.clone())), 13.0, if c.with(|c| c.error) { theme::BLOCKED } else { theme::DIM }).into_element()))
        .child(text(move || c.with(|c| c.name.clone())).flex_shrink(0.0).text_size(theme::CAPTION).font_family(theme::MONO).text_color(theme::MUTED))
        .child(text(move || c.with(|c| c.said.clone())).flex_1().text_size(theme::CAPTION).font_family(theme::MONO).text_color(theme::DIM).number_of_lines(1))
}

fn step(c: &Call) {
    nav::push(Route::Step(Arc::new(c.clone())));
}

/// One step the agent took, on a page of its own: what the tool was
/// given (an edit as its lines out and in), and what it gave back.
pub fn step_page(live: Live, call: Arc<Call>) -> Screen {
    let copied = call.clone();
    ui::page(call.name.clone())
        .header_right(
            view()
                .px(px(12.0))
                .h(px(40.0))
                .justify_center()
                .role(AccessibilityRole::Button)
                .label(t!("Copy"))
                .on_press(move || {
                    set_clipboard(format!("{}\n\n{}", copied.input, copied.result.clone().unwrap_or_default()));
                    live.say(Note::info(t!("Copied"), t!("What it was given and gave back is on the clipboard.")));
                })
                .child(text(t!("Copy")).text_size(theme::BODY).text_color(theme::ACCENT)),
        )
        .child(
            scroll().flex_1().bg(theme::BACKGROUND).child(
                view()
                    .p(px(16.0))
                    .pb(move || ui::bottom_inset() + px(16.0))
                    .gap(px(8.0))
                    .child(text(t!("GIVEN")).text_size(theme::CAPTION).font_weight(FontWeight::SEMIBOLD).text_color(theme::MUTED))
                    .child(if call.diff { diff(&call.input) } else { block(call.input.clone(), false) }.test_id("step-input"))
                    .child(view().h(px(10.0)))
                    .child(text(if call.error { t!("FAILED") } else { t!("GAVE BACK") }).text_size(theme::CAPTION).font_weight(FontWeight::SEMIBOLD).text_color(if call.error { theme::BLOCKED } else { theme::MUTED }))
                    .child(block(call.result.clone().unwrap_or_else(|| t!("(no result yet)").into()), call.error).test_id("step-result")),
            ),
        )
}

/// Text as a terminal shows it, to select from.
fn block(said: String, failed: bool) -> View {
    view()
        .selectable(true)
        .p(px(12.0))
        .rounded(px(10.0))
        .bg(theme::RAISED)
        .child(text(said).font_family(theme::MONO).text_size(theme::FOOTNOTE).line_height(px(18.0)).text_color(if failed { theme::BLOCKED } else { theme::FOREGROUND }))
}

/// An edit: its file, then the lines it took out and put in, colored as
/// a diff is.
fn diff(input: &str) -> View {
    input.lines().fold(view().selectable(true).py(px(6.0)).rounded(px(10.0)).overflow_hidden().bg(theme::RAISED), |v, line| {
        let (ink, ground) = match line.get(..2) {
            Some("- ") => (theme::REMOVED, Some(theme::REMOVED_ROW)),
            Some("+ ") => (theme::ADDED, Some(theme::ADDED_ROW)),
            _ => (theme::FOREGROUND, None),
        };
        let row = view().px(px(12.0)).py(px(1.0));
        let row = if let Some(ground) = ground { row.bg(ground) } else { row };
        v.child(row.child(text(line.to_owned()).font_family(theme::MONO).text_size(theme::FOOTNOTE).line_height(px(18.0)).text_color(ink)))
    })
}

/// How the agent is now, under what it said: working (on what), or
/// asking something in its terminal: the question, as it shows there,
/// with the keys to answer it.
fn state(live: Live, address: String, status: Memo<&'static str>, doing: Memo<Option<String>>, name: String, answering: Stored<Option<Rc<Viewer>>>) -> impl IntoElement {
    dynamic(move || match status.get() {
        "working" => {
            answering.replace(None);
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(ui::glyph("spinner", 13.0, theme::WORKING).animate_spin())
                .child(text(t!("Working…")).flex_shrink(0.0).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
                .child(text(move || doing.get().unwrap_or_default()).flex_1().text_size(theme::CAPTION).font_family(theme::MONO).text_color(theme::DIM).number_of_lines(1))
                .into_element()
        }
        "blocked" => {
            let (asked, viewer) = crate::terminal::question(live.engine.clone(), &address);
            answering.replace(Some(viewer.clone()));
            let keys = [("↑", t!("Up"), Key::Up), ("↓", t!("Down"), Key::Down), ("←", t!("Left"), Key::Left), ("→", t!("Right"), Key::Right), ("␣", t!("Space bar"), Key::Char(' ')), ("Tab", t!("Tab key"), Key::Tab), ("Esc", "Esc", Key::Escape), ("⏎", t!("Enter"), Key::Enter)];
            let row = keys.into_iter().fold(view().flex_row().gap(px(6.0)), |row, (face, said, key)| {
                let viewer = viewer.clone();
                row.child(
                    view()
                        .flex_1()
                        .h(px(38.0))
                        .rounded(px(8.0))
                        .items_center()
                        .justify_center()
                        .bg(theme::RAISED)
                        .border_1()
                        .border_color(theme::BORDER)
                        .active(|s| s.bg(theme::HOVER).scale(ui::PRESSED))
                        .transition()
                        .duration(ui::QUICK)
                        .role(AccessibilityRole::Button)
                        .label(said)
                        .on_press(move || viewer.key(key, Mods::NONE))
                        .child(text(face).text_size(theme::CALLOUT).font_weight(FontWeight::MEDIUM).text_color(theme::FOREGROUND)),
                )
            });
            view()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(px(14.0))
                .bg(ui::faint(theme::BLOCKED))
                .test_id("asked")
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(ui::dot(|| Some("blocked")))
                        .child(text(t!("{} is asking", name.clone())).flex_1().text_size(theme::ROW).font_weight(FontWeight::SEMIBOLD).text_color(theme::BLOCKED).number_of_lines(1))
                        .child(
                            view()
                                .px(px(8.0))
                                .h(px(30.0))
                                .justify_center()
                                .rounded(px(8.0))
                                .active(|s| s.bg(theme::HOVER))
                                .role(AccessibilityRole::Button)
                                .on_press(|| {
                                    ui::smoothly();
                                    talking().set(Talking(false));
                                })
                                .child(text(t!("Open the terminal")).text_size(theme::FOOTNOTE).text_color(theme::ACCENT)),
                        ),
                )
                .child(asked)
                .child(row)
                .child(text(t!("The keys go to its terminal; what you write below answers there too.")).text_size(theme::CAPTION).text_color(theme::MUTED))
                .into_element()
        }
        _ => {
            answering.replace(None);
            view().into_element()
        }
    })
}

/// When something was said, in the phone's time: the time today,
/// yesterday's said so, the date before that.
fn time_of(at: u64) -> String {
    let local = |ms: u64| {
        let s = (ms / 1000) as i64;
        s + utc_offset(s)
    };
    let (then, now) = (local(at), local(now_ms()));
    let (day, today) = (then.div_euclid(86_400), now.div_euclid(86_400));
    let clock = format!("{:02}:{:02}", then.rem_euclid(86_400) / 3600, then.rem_euclid(3600) / 60);
    match today - day {
        0 => clock,
        1 => t!("Yesterday {}", clock),
        _ => {
            let (_, month, date) = civil(day);
            t!("{}/{} {}", month, date, clock)
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// The year, month and day of a day since 1970-01-01 (Howard Hinnant's).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, date)
}

/// The phone's offset from UTC at `secs`, in seconds (its time zone, as
/// the system keeps it).
#[cfg(target_os = "android")]
fn utc_offset(secs: i64) -> i64 {
    let at = secs as libc::time_t;
    // SAFETY: `localtime_r` writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&at, &mut tm) }.is_null() { 0 } else { tm.tm_gmtoff as i64 }
}

#[cfg(not(target_os = "android"))]
fn utc_offset(_: i64) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, at: u64) -> Line {
        Line { id: id.into(), said: Said::Stopped, at }
    }

    #[test]
    fn the_time_is_said_first_and_where_much_of_it_passed() {
        let hour = 3_600_000;
        let rows: Vec<String> = items(&[line("a", 10 * hour), line("b", 10 * hour + 60_000), line("c", 12 * hour), line("d", 0)]).iter().map(Item::key).collect();
        assert_eq!(rows, ["time:a", "a", "b", "time:c", "c", "d"]);
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_406), (2025, 11, 14));
        assert_eq!(civil(-1), (1969, 12, 31));
    }
}
