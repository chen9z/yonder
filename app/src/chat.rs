//! The main agent as a conversation, drawn rather than shown as its
//! terminal: Pi's session read as it grows ([`transcript`]), the
//! person's words on the right, the agent's as they read, the tools it
//! used folded between them. What is written goes in as `hc send` puts
//! it (no terminal held for it); what the agent asks in its terminal is
//! answered there.

use crate::live::{off, Live};
use crate::nav::{self, Route};
use crate::{theme, ui};
use herdc_engine::tools::transcript::{self, Call, Line, Said, Transcript};
use nsh_base::{text_view, TextViewLinks};
use nsh_core::prelude::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the session is read again: while the agent works, else.
const WORKING: Duration = Duration::from_secs(2);
const RESTING: Duration = Duration::from_secs(10);

/// Within this of the end, the list is at the end and stays there as
/// the conversation grows.
const NEAR_END: f32 = 48.0;

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

/// A message sent and not yet in the session.
#[derive(Clone, Debug, PartialEq)]
struct Sent {
    n: u64,
    text: String,
}

/// The conversation with the agent at `address`; `name` says whose it is.
pub fn chat(live: Live, address: String, name: String) -> impl IntoElement {
    let world = live.view;
    let lines = signal(Vec::<Line>::new());
    let cut = signal(false);
    let found = signal(Found::Looking);
    let sent = signal(Vec::<Sent>::new());
    // A read wanted now (a message went in).
    let again = signal(0u32);
    let status = {
        let address = address.clone();
        computed(move || world.with(|v| v.agents.iter().find(|a| a.address == address).map_or("idle", |a| a.status)))
    };
    {
        let (live, address) = (live.clone(), address.clone());
        spawn(async move {
            let mut read = Transcript::default();
            let mut path: Option<(String, String)> = None;
            let mut seen = (u64::MAX, String::new(), 0u32);
            let mut last = Instant::now() - RESTING;
            loop {
                let (label, cwd, turn, word, on) = world.with_untracked(|v| {
                    let t = v.terminal(&address);
                    let on = v.agents.iter().find(|a| a.address == address).and_then(|a| a.machine.clone());
                    t.map(|t| (t["labels"]["alma.session"].as_str().unwrap_or_default().to_owned(), t["cwd"].as_str().unwrap_or_default().to_owned(), t["turn"].as_u64().unwrap_or(0), t["status"].as_str().unwrap_or_default().to_owned(), on))
                        .unwrap_or_default()
                });
                let now = (turn, word.clone(), again.get_untracked());
                let every = if word == "working" { WORKING } else { RESTING };
                let due = now != seen || last.elapsed() >= every;
                let shown = lifecycle() == nsh_core::platform::Lifecycle::Active;
                if due && shown && let Ok(there) = live.there(on) {
                    seen = now;
                    last = Instant::now();
                    // Another session (`/new`, a restart): read afresh.
                    if path.as_ref().is_none_or(|(l, _)| *l != label) {
                        let (t, l, c) = (there.clone(), label.clone(), cwd.clone());
                        match off(async move { transcript::find(&t, Some(&l), &c).await }).await {
                            Some(Ok(Some(file))) => {
                                path = Some((label.clone(), file));
                                read = Transcript::default();
                            }
                            Some(Ok(None)) => found.set(Found::Not(t!("No Pi session found for it.").into())),
                            Some(Err(e)) => found.set(Found::Not(e)),
                            None => {}
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
                                    // What was sent and shows now is no longer waiting.
                                    sent.update(|s| s.retain(|m| !now.iter().rev().take(12).any(|l| matches!(&l.said, Said::You { text, .. } if text == m.text.trim()))));
                                    if first {
                                        lines.set(now);
                                    } else {
                                        ui::smoothly();
                                        lines.set(now);
                                    }
                                    cut.set(read.cut);
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
                off(tokio::time::sleep(Duration::from_millis(500))).await;
            }
        });
    }

    let list = element_ref();
    let stuck = stored(true);
    let to_end = move || {
        if stuck.get() {
            list.scroll_to_end(false);
        }
    };
    let send = {
        let (live, address) = (live.clone(), address.clone());
        move |text: String| {
            let n = sent.with_untracked(|s| s.last().map_or(0, |m| m.n + 1));
            stuck.replace(true);
            ui::smoothly();
            sent.update(|s| s.push(Sent { n, text: text.clone() }));
            let (live, address) = (live.clone(), address.clone());
            spawn(async move {
                let there = live.there(world.with_untracked(|v| v.agents.iter().find(|a| a.address == address).and_then(|a| a.machine.clone())));
                let words = text.clone();
                let went = match there {
                    Ok(there) => off(async move { transcript::send(&there, &address, &words).await }).await.unwrap_or_else(|| Err("Not connected".into())),
                    Err(e) => Err(e),
                };
                match went {
                    Ok(()) => again.update(|a| *a += 1),
                    // Not sent: back in the field, and why.
                    Err(why) => {
                        sent.update(|s| s.retain(|m| m.n != n));
                        crate::terminal::unsent(&text);
                        let why = if why == "blocked" { t!("It is waiting for an answer in its terminal.").to_owned() } else { why };
                        live.say(herdc_engine::Note::error(t!("Not sent"), why));
                    }
                }
            });
        }
    };

    let placeholder = t!("Message {}", name.clone());
    view()
        .flex_1()
        .child(dynamic(move || match found.get() {
            Found::Looking => view().flex_1().items_center().justify_center().child(ui::glyph("spinner", 18.0, theme::DIM).animate_spin()).into_element(),
            Found::Not(why) => missing(why).into_element(),
            Found::Read => scroll()
                .flex_1()
                .element_ref(&list)
                .keyboard_dismiss_mode(KeyboardDismissMode::OnDrag)
                .on_scroll(move |e: &ScrollEvent| {
                    stuck.replace(e.offset.y.0 + e.size.height.0 >= e.content_size.height.0 - NEAR_END);
                })
                // The keyboard up, the list shorter: still at the end.
                .on_layout(move |_: &Bounds<Points>| to_end())
                .child(
                    view()
                        // Grown (a reply, a tool opened): at the end, if it was.
                        .on_layout(move |_: &Bounds<Points>| to_end())
                        .px(px(16.0))
                        .pt(px(8.0))
                        .pb(px(16.0))
                        .gap(px(14.0))
                        .child(when(move || cut.get(), || {
                            text(t!("Earlier messages are in its terminal.")).self_center().text_size(theme::CAPTION).text_color(theme::DIM)
                        }))
                        .child(each(move || lines.get(), |l| l.id.clone(), |l, _| said(l)))
                        .child(each(move || sent.get(), |m| m.n, |m, _| you(move || m.with(|m| m.text.clone()), || 0, true)))
                        .child(state(status, name.clone())),
                )
                .into_element(),
        }))
        .child(crate::terminal::talk_bar(placeholder, send))
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

/// One thing said.
fn said(l: ReadSignal<Line>) -> impl IntoElement {
    let kind = computed(move || l.with(|l| std::mem::discriminant(&l.said)));
    dynamic(move || {
        let _ = kind.get();
        match l.with_untracked(|l| l.said.clone()) {
            Said::You { .. } => you(move || l.with(|l| match &l.said { Said::You { text, .. } => text.clone(), _ => String::new() }), move || l.with(|l| match &l.said { Said::You { images, .. } => *images, _ => 0 }), false).into_element(),
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
        }
    })
}

/// The person's words, on the right in a bubble; `waiting` while sent
/// and not yet in the session.
fn you(words: impl Fn() -> String + 'static, images: impl Fn() -> usize + 'static, waiting: bool) -> impl IntoElement {
    view()
        .flex_row()
        .justify_end()
        .pl(px(48.0))
        .opacity(if waiting { 0.55 } else { 1.0 })
        .child(
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
                        .child(ui::glyph("file", 13.0, theme::MUTED))
                        .child(text(t!("{} images", n)).text_size(theme::CAPTION).text_color(theme::MUTED))
                        .into_element(),
                }))
                .child(text(words).text_size(theme::ROW).line_height(px(21.0)).text_color(theme::FOREGROUND)),
        )
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
/// pressed shows what it gave back.
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
                        result(&calls()[0]);
                    } else {
                        ui::smoothly();
                        open.update(|o| *o = !*o);
                    }
                })
                .child(dynamic(move || ui::glyph(icon(&calls().first().map(|c| c.name.clone()).unwrap_or_default()), 14.0, theme::MUTED).into_element()))
                .child(text(summary).flex_1().text_size(theme::FOOTNOTE).font_family(theme::MONO).text_color(theme::MUTED).number_of_lines(1))
                .child(when(move || failed() > 0, || ui::dot(|| Some("blocked"))))
                .child(when(move || count() > 1, move || ui::chevron(move || open.get(), 14.0))),
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
        .h(px(32.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(move || c.with(|c| format!("{} {}", c.name, c.said)))
        .on_press(move || result(&c.get_untracked()))
        .child(dynamic(move || ui::glyph(icon(&c.with(|c| c.name.clone())), 13.0, if c.with(|c| c.error) { theme::BLOCKED } else { theme::DIM }).into_element()))
        .child(text(move || c.with(|c| c.name.clone())).flex_shrink(0.0).text_size(theme::CAPTION).font_family(theme::MONO).text_color(theme::MUTED))
        .child(text(move || c.with(|c| c.said.clone())).flex_1().text_size(theme::CAPTION).font_family(theme::MONO).text_color(theme::DIM).number_of_lines(1))
}

/// What a tool was given and gave back, on a page of its own.
fn result(c: &Call) {
    let said = format!("{} {}\n\n{}", c.name, c.said, c.result.clone().unwrap_or_else(|| t!("(no result yet)").into()));
    nav::push(Route::Text(Arc::new(said)));
}

/// How the agent is now, under what it said: working, or waiting for an
/// answer in its terminal.
fn state(status: Memo<&'static str>, name: String) -> impl IntoElement {
    dynamic(move || match status.get() {
        "working" => view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(ui::glyph("spinner", 13.0, theme::WORKING).animate_spin())
            .child(text(t!("Working…")).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
            .into_element(),
        "blocked" => view()
            .gap(px(10.0))
            .p(px(14.0))
            .rounded(px(14.0))
            .bg(ui::faint(theme::BLOCKED))
            .child(text(t!("{} needs you in its terminal", name)).text_size(theme::ROW).font_weight(FontWeight::SEMIBOLD).text_color(theme::BLOCKED))
            .child(text(t!("It asks something the conversation can't answer: a choice, a confirmation.")).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
            .child(ui::ghost_button(t!("Answer in the terminal"), || {
                ui::smoothly();
                talking().set(Talking(false));
            }))
            .into_element(),
        _ => view().into_element(),
    })
}
