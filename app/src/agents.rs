//! Agents: every agent in a pane, by priority or grouped by space, as
//! herdr-app's agents list orders them, each with the sub-agents it
//! started. A tap shows one; a long press pins or stops it. A sub-agent
//! without a pane is peeked at, as the desktop's peek overlay does.
//! A waiting agent is marked Needs you; tap its row to open its terminal.

use crate::live::Live;
use crate::nav::{self, Route};
use crate::i18n::tr;
use crate::theme;
use crate::ui;
use herdc_engine::AgentView;
use nsh_core::prelude::*;
use serde_json::{json, Value};

/// How much an agent's status asks of the person (the desktop's ranks).
fn rank(status: &str) -> u8 {
    match status {
        "blocked" => 4,
        "done" => 3,
        "working" => 2,
        "idle" => 1,
        _ => 0,
    }
}

/// A priority section's heading, and the status it's colored by.
fn section(rank: u8) -> (&'static str, Option<&'static str>) {
    match rank {
        4 => ("Needs you", Some("blocked")),
        3 => ("Done", Some("done")),
        2 => ("Working", Some("working")),
        _ => ("Idle", None),
    }
}

/// Idle agents shown before Show all, by priority.
const IDLE_SHOWN: usize = 5;

/// The agents with rows of their own: a kept sub-agent is under its
/// agent instead, unless it's pinned.
pub fn listed(agents: &[AgentView], terminals: &[Value]) -> Vec<AgentView> {
    // Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
    let nested: std::collections::HashSet<_> = agents.iter().flat_map(|a| subs(terminals, &a.address)).map(|s| s.address).collect();
    agents.iter().filter(|a| a.pinned || !nested.contains(&a.address)).cloned().collect()
}

/// A line of the list: a group's heading, an agent, or a sub-agent under it.
#[derive(Clone, Debug, PartialEq)]
enum Line {
    /// `word`: a heading the app says (in its language), else a space's `label`.
    Heading { id: String, label: String, word: Option<&'static str>, status: Option<&'static str>, count: usize, open: bool, top: Option<&'static str>, priority: bool },
    Agent { agent: Box<AgentView>, name: String, more: Option<String>, machine: Option<String>, subs: usize, open: bool, compact: bool, edge: Edge },
    Sub { sub: Sub, last: bool, compact: bool, edge: Edge },
    /// Under the first idle ones: Show all `count`, or (`open`) Show less.
    More { count: usize, open: bool, edge: Edge },
}

/// Where a row is on its group's card: its first, its last (or both).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Edge {
    top: bool,
    bottom: bool,
}

impl Line {
    fn key(&self) -> String {
        match self {
            Line::Heading { id, .. } => format!("h {id}"),
            Line::Agent { agent, .. } => format!("a {}", agent.address),
            Line::Sub { sub, .. } => format!("s {}", sub.address),
            Line::More { .. } => "more".into(),
        }
    }
}

/// The list as the desktop's agents mod orders it: pinned first, then by
/// priority (an agent ranks as its most urgent sub-agent, the latest
/// change first) under section headings, or grouped by space. `folded`:
/// the groups and agents folded; `all`: every idle one, else the first
/// few; `held`: the agent shown and the rank it had, so a done one looked
/// at stays with Done until left.
// Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
fn lines(v: &herdc_engine::View, grouped: bool, folded: &[String], all: bool, held: &mut Option<(String, u8)>) -> Vec<Line> {
    let listed = listed(&v.agents, &v.terminals);
    let shown = v.agents.iter().find(|a| a.focused);
    let same = shown.zip(held.as_ref()).is_some_and(|(s, h)| s.address == h.0);
    if !same || shown.is_some_and(|s| s.status != "idle") {
        *held = shown.map(|s| (s.address.clone(), rank(s.status)));
    }
    let saw_done = |a: &AgentView| held.as_ref().is_some_and(|h| h.0 == a.address && h.1 == 3) && a.status == "idle";
    let kids: std::collections::HashMap<&str, Vec<Sub>> = listed.iter().map(|a| (a.address.as_str(), listed_subs(&v.terminals, &v.agents, &a.address))).collect();
    let urgency = |a: &AgentView| kids[a.address.as_str()].iter().map(|s| s.status.map_or(0, rank)).fold(if saw_done(a) { 3 } else { rank(a.status) }, u8::max);
    let mut list: Vec<&AgentView> = listed.iter().collect();
    if !grouped {
        list.sort_by_key(|a| (std::cmp::Reverse(urgency(a)), std::cmp::Reverse(a.since), std::cmp::Reverse(a.seq)));
    }
    list.sort_by_key(|a| !a.pinned);
    // Groups in list order: neighbours of one space or section make one:
    // id, label, word, status and members.
    type Group<'a> = (String, String, Option<&'static str>, Option<&'static str>, Vec<&'a AgentView>);
    let mut groups: Vec<Group> = Vec::new();
    for a in list {
        let (id, label, word, status) = if a.pinned {
            ("pinned".to_owned(), String::new(), Some("Pinned"), None)
        } else if grouped {
            (format!("sp-{}", a.space), a.space_name.clone(), None, None)
        } else {
            let r = urgency(a).max(1);
            let (word, status) = section(r);
            (format!("sec-{r}"), String::new(), Some(word), status)
        };
        match groups.last_mut() {
            Some(g) if g.0 == id => g.4.push(a),
            _ => groups.push((id, label, word, status, vec![a])),
        }
    }
    let mut out = Vec::new();
    for (id, label, word, status, members) in groups {
        let open = !folded.contains(&id);
        let top = members.iter().map(|a| a.status).max_by_key(|s| rank(s));
        let pinned = id == "pinned";
        out.push(Line::Heading { id, label, word, status, count: members.len(), open, top, priority: !grouped });
        if !open {
            continue;
        }
        // Many idle ones: the first few, and the way to the rest.
        let count = members.len();
        let more = id_is_idle(&out, grouped) && count > IDLE_SHOWN + 1;
        for a in members.into_iter().take(if more && !all { IDLE_SHOWN } else { usize::MAX }) {
            let (name, more) = parts(a, v, &listed, !grouped || a.pinned);
            let subs = &kids[a.address.as_str()];
            let open = !folded.contains(&a.address);
            let compact = !grouped && !pinned;
            out.push(Line::Agent { agent: Box::new(a.clone()), name, more, machine: machine(a), subs: subs.len(), open, compact, edge: Edge::default() });
            if open {
                for (i, sub) in subs.iter().enumerate() {
                    // The last of its siblings: none at its depth before the tree goes up.
                    let last = subs[i + 1..].iter().find(|s| s.depth <= sub.depth).is_none_or(|s| s.depth < sub.depth);
                    out.push(Line::Sub { sub: sub.clone(), last, compact: !grouped, edge: Edge::default() });
                }
            }
        }
        if more {
            out.push(Line::More { count, open: all, edge: Edge::default() });
        }
    }
    // A group's rows make one card: under its heading, until the next.
    let heading = |l: Option<&Line>| l.is_none_or(|l| matches!(l, Line::Heading { .. }));
    for at in 0..out.len() {
        let (top, bottom) = (heading(at.checked_sub(1).and_then(|b| out.get(b))), heading(out.get(at + 1)));
        if let Line::Agent { edge, .. } | Line::Sub { edge, .. } | Line::More { edge, .. } = &mut out[at] {
            *edge = Edge { top, bottom };
        }
    }
    out
}

/// Whether the group just headed in `out` is Idle, by priority.
fn id_is_idle(out: &[Line], grouped: bool) -> bool {
    !grouped && matches!(out.last(), Some(Line::Heading { id, .. }) if id == "sec-1")
}

/// A sub-agent: its depth under the agent, address, name and status word.
#[derive(Clone, Debug, PartialEq)]
pub struct Sub {
    pub depth: usize,
    pub address: String,
    pub name: String,
    pub status: Option<&'static str>,
    pub program: Value,
}

/// The live terminals `parent` started, depth first (the desktop's tree).
pub fn subs(terminals: &[Value], parent: &str) -> Vec<Sub> {
    fn walk(terminals: &[Value], parent: &str, depth: usize, out: &mut Vec<Sub>) {
        for t in terminals.iter().filter(|t| t["parent"] == parent && t["state"] != "finished") {
            let Some(address) = t["address"].as_str() else { continue };
            let name = ["name", "template"].iter().find_map(|k| t[*k].as_str()).or(t["program"]["name"].as_str()).unwrap_or(address);
            let status = match t["status"].as_str() {
                Some("working") => Some("working"),
                Some("blocked" | "error") => Some("blocked"),
                Some("done" | "idle") => Some("idle"),
                _ => None,
            };
            out.push(Sub { depth, address: address.to_owned(), name: name.to_owned(), status, program: t["program"].clone() });
            if depth < 8 {
                walk(terminals, address, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(terminals, parent, 0, &mut out);
    out
}

/// Kept children use their listed status before done becomes idle.
// Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
fn listed_subs(terminals: &[Value], agents: &[AgentView], parent: &str) -> Vec<Sub> {
    let mut children = subs(terminals, parent);
    for child in &mut children {
        if let Some(kept) = agents.iter().find(|a| a.address == child.address) {
            child.status = Some(if kept.status == "done" { "idle" } else { kept.status });
        }
    }
    children
}

/// Priority rows name the space and a telling tab or agent detail.
// Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
fn row_detail(a: &AgentView, view: &herdc_engine::View, listed: &[AgentView]) -> String {
    let many = listed.iter().filter(|other| other.tab == a.tab).count() > 1;
    let tab = view.tab(&a.tab);
    // First agent, else focused terminal, else first terminal, as on desktop.
    let lead = tab.and_then(|t| t.panes.iter().filter(|p| p.terminal.is_some()).min_by_key(|p| if p.program["agent"].as_bool().unwrap_or(false) { 0 } else if p.focused { 1 } else { 2 }));
    let (title, program, app) = if !many && let Some(p) = lead { (&p.title, &p.program, &p.app) } else { (&a.title, &a.program, &a.app) };
    let program_name = program["name"].as_str().filter(|s| !s.is_empty()).or(app.as_deref()).unwrap_or_default();
    let title = ui::title(title);
    let live = title.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
    // Pi's own title, `π - <its folder>`, says no more than Pi.
    let live = if title.starts_with("π - ") { "" } else { live };
    let telling = !live.is_empty() && live != program_name && !title.starts_with(['~', '/']) && !live.contains('@') && !live.contains(":\\") && !program["key"].as_str().is_some_and(|k| live.eq_ignore_ascii_case(k));
    let named = tab.is_some_and(|t| t.named) && !many;
    if (!named || a.tab_name == program_name) && telling { live.into() } else if live.is_empty() && !named && !program_name.is_empty() { program_name.into() } else { a.tab_name.clone() }
}

fn numeric(detail: &str) -> bool {
    !detail.is_empty() && detail.chars().all(|c| c.is_ascii_digit())
}

/// An agent's row text: its name and the detail after it. `full`: with
/// no space heading above (by priority, pinned), the name is its space's.
// Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
fn parts(a: &AgentView, view: &herdc_engine::View, listed: &[AgentView], full: bool) -> (String, Option<String>) {
    let program = a.program["name"].as_str().or(a.app.as_deref()).unwrap_or_default();
    // `π - nova` in nova says Pi; `nova` alone, the agent's name too.
    let detail = row_detail(a, view, listed);
    let stripped = ui::without(&detail, std::slice::from_ref(&a.space_name));
    let label = if stripped != detail && stripped.chars().count() <= 2 && !program.is_empty() { program.to_owned() } else { stripped };
    // An automatic tab number says nothing, unless two rows would read the same.
    let ambiguous = listed.iter().filter(|other| other.space_name == a.space_name && numeric(&row_detail(other, view, listed))).count() > 1;
    let more = (!label.is_empty() && (!numeric(&label) || ambiguous)).then(|| label.clone());
    let named = || if label.is_empty() || numeric(&label) { if program.is_empty() { label.clone() } else { program.to_owned() } } else { label.clone() };
    match (full, a.space_name.is_empty()) {
        (true, false) => (a.space_name.clone(), more),
        (true, true) => (named(), None),
        (false, _) => (more.unwrap_or_else(named), None),
    }
}

/// The machine of an agent on another one (`vps/t7`).
fn machine(a: &AgentView) -> Option<String> {
    a.machine.clone().or_else(|| a.address.split_once('/').map(|(machine, _)| machine.to_owned()))
}

#[cfg(test)]
fn label(a: &AgentView, view: &herdc_engine::View) -> String {
    let (name, more) = parts(a, view, &listed(&view.agents, &view.terminals), true);
    [Some(name), more, machine(a)].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// Time since a status change, in the desktop's short form.
// Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
fn elapsed(since: u64, now: u64) -> String {
    if since == 0 { return String::new() }
    let minutes = now.saturating_sub(since) / 60_000;
    match minutes {
        0 => t!("<1m").into(),
        1..60 => t!("{}m", minutes),
        60..1440 => t!("{}h", minutes / 60),
        _ => t!("{}d", minutes / 1440),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// The agents in a pane of their own, where showing one stays on the page.
pub fn agent_list(live: Live) -> impl IntoElement {
    scroll().flex_1().bg(theme::CANVAS).child(agents_section(live, true))
}

/// The agents section: its heading, with the sort, and the list.
/// `inline`: in a pane, where showing one stays on the page.
pub fn agents_section(live: Live, inline: bool) -> impl IntoElement {
    let world = live.view;
    let grouped = use_context::<crate::prefs::Prefs>().unwrap().grouped;
    let folded = signal(Vec::<String>::new());
    let held = stored(None::<(String, u8)>);
    let all = signal(false);
    let now = signal(now_ms());
    set_interval(std::time::Duration::from_secs(30), move || now.set(now_ms()));
    let lines = computed(move || {
        let (grouped, folded, all) = (grouped.get(), folded.get(), all.get());
        world.with(|v| held.with(|held| lines(v, grouped, &folded, all, held)))
    });
    let fold = move |id: String| folded.update(|set| if set.contains(&id) { set.retain(|k| k != &id) } else { set.push(id) });
    view()
        .child(when(move || inline, || crate::home::section(t!("Agents"), view())))
        .child(when(move || lines.with(Vec::is_empty), || {
            text(t!("No agent status yet. Agents show up here when they report their status. Older versions and agents inside tmux may not."))
                .px(px(16.0))
                .py(px(8.0))
                .text_size(theme::FOOTNOTE)
                .text_color(theme::DIM)
        }))
        .child(each(move || lines.get(), Line::key, move |l, _| match l.get_untracked() {
            Line::Heading { .. } => heading(l, fold).into_element(),
            Line::Agent { .. } => agent_row(live.clone(), l, now, inline, fold).into_element(),
            Line::Sub { .. } => sub_row(live.clone(), l, inline).into_element(),
            Line::More { .. } => more_row(l, all).into_element(),
        }))
        .child(view().h(px(if inline { 8.0 } else { 0.0 })))
}

/// A group's heading: a tap folds the group; folded, it says how many and
/// the most urgent status in it.
// Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
fn heading(l: ReadSignal<Line>, fold: impl Fn(String) + Copy + 'static) -> impl IntoElement {
    let head = computed(move || match l.get() {
        Line::Heading { id, label, word, status, count, open, top, priority } => (id, word.map_or(label, |w| tr(w).to_owned()), status, count, open, top, priority),
        _ => Default::default(),
    });
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .mx(theme::INSET)
        .mt(px(6.0))
        .px(px(12.0))
        .h(px(36.0))
        .rounded(px(8.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(move || head.with(|h| t!("Fold or unfold {}", h.1)))
        .on_press(move || fold(head.with_untracked(|h| h.0.clone())))
        .child(
            text(move || head.with(|h| h.1.clone()))
                .flex_shrink(1.0)
                .text_size(theme::CAPTION)
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || head.with(|h| h.2).map_or(theme::now(theme::DIM), |s| theme::now(theme::status(s))))
                .number_of_lines(1),
        )
        .child(when(move || head.with(|h| h.6 || !h.4), move || text(move || head.with(|h| h.3.to_string())).text_size(theme::CAPTION).text_color(theme::DIM)))
        .child(view().flex_1())
        .child(when(move || head.with(|h| !h.4), move || {
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(ui::status_mark(move || head.with(|h| h.5.filter(|s| !h.6 && *s != "idle")), || true))
                .child(ui::glyph("chevron-right", 12.0, theme::DIM))
        }))
}

/// A row's piece of its group's card: the card's color, rounded where
/// the card starts and ends, a line above it from the words on.
fn on_card(edge: impl Fn() -> Edge + Copy + 'static, line: bool) -> View {
    view()
        .mx(theme::INSET)
        .bg(theme::POPOVER)
        .overflow_hidden()
        .rounded_t(move || if edge().top { theme::CARD_RADIUS } else { px(0.0) })
        .rounded_b(move || if edge().bottom { theme::CARD_RADIUS } else { px(0.0) })
        .child(when(move || line && !edge().top, || view().h(px(1.0)).ml(px(46.0)).bg(theme::BORDER)))
}

/// An agent's row: its space's mark (what runs on its corner), the space
/// and what it does, where it is under them, how long since its status
/// changed and that status. A tap shows it; a long press pins or stops it.
fn agent_row(live: Live, l: ReadSignal<Line>, now: Signal<u64>, inline: bool, fold: impl Fn(String) + Copy + 'static) -> impl IntoElement {
    let agent = move || l.with(|l| match l { Line::Agent { agent, .. } => Some((**agent).clone()), _ => None });
    let row = computed(move || match l.get() {
        Line::Agent { agent, name, more, machine, subs, open, compact, .. } => Some((agent.address, agent.status, agent.focused, agent.pinned, name, more, machine, subs, open, compact)),
        _ => None,
    });
    let edge = move || l.with(|l| match l { Line::Agent { edge, .. } => *edge, _ => Edge::default() });
    let prefs = use_context::<crate::prefs::Prefs>().unwrap();
    let computer = live.computer.id();
    let here = live.computer.name.clone();
    let color = {
        let computer = computer.clone();
        move || l.with(|l| match l { Line::Agent { agent, .. } => prefs.colors.with(|c| c.get(&format!("{computer}\u{0}{}", agent.space)).copied()).unwrap_or(0), _ => 0 })
    };
    let (press, hold) = (live.clone(), live);
    on_card(edge, true)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .px(px(12.0))
                .min_h(px(58.0))
                .py(px(8.0))
                .bg(move || if row.with(|r| r.as_ref().is_some_and(|r| r.2)) { theme::now(theme::SELECTED) } else { Hsla::transparent_black() })
                .active(|s| s.bg(theme::HOVER))
                .on_press(move || {
                    if let Some(a) = agent() {
                        press.run("agent.focus", json!({ "terminal": a.address }));
                        if !inline {
                            nav::workspace();
                        }
                    }
                })
                .on_long_press(move || {
                    let Some(a) = agent() else { return };
                    let live = hold.clone();
                    spawn(async move {
                        use crate::pick::{self, Item};
                        let pin = if a.pinned { Item::glyph("unpin", t!("Unpin"), "pin-off") } else { Item::glyph("pin", t!("Pin to the top"), "pin") };
                        match pick::one(&ui::title(&a.title), vec![pin, Item::glyph("kill", t!("Stop it"), "square").danger()]).await.as_deref() {
                            Some("pin") => live.run("terminal.pin", json!({ "terminal": a.address, "pinned": true })),
                            Some("unpin") => live.run("terminal.pin", json!({ "terminal": a.address, "pinned": false })),
                            Some("kill") if ui::confirm(t!("Stop this agent?"), t!("It ends with its sub-agents."), t!("Stop")).await => {
                                live.run("terminal.kill", json!({ "terminal": a.address }));
                            }
                            _ => {}
                        }
                    });
                })
                .child(ui::space_badge(move || agent().map(|a| a.space_name).unwrap_or_default(), color, move || agent().map(|a| a.program).unwrap_or_default()))
                .child(
                    view()
                        .flex_1()
                        .gap(px(2.0))
                        // The space (or what it does, under its space's heading), then what it does, quieter.
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    text(move || row.with(|r| r.as_ref().map(|r| r.4.clone()).unwrap_or_default()))
                                        .flex_shrink(0.0)
                                        .max_w(px(180.0))
                                        .text_size(theme::ROW)
                                        .font_weight(FontWeight::MEDIUM)
                                        // An idle agent not shown is quieter.
                                        .text_color(move || if row.with(|r| r.as_ref().is_some_and(|r| r.1 == "idle" && !r.2)) { theme::now(theme::MUTED) } else { theme::now(theme::FOREGROUND) })
                                        .number_of_lines(1),
                                )
                                .child(
                                    text(move || row.with(|r| r.as_ref().and_then(|r| r.5.clone()).unwrap_or_default()))
                                        .flex_1()
                                        .text_size(theme::CALLOUT)
                                        .text_color(theme::MUTED)
                                        .number_of_lines(1),
                                ),
                        )
                        // Where: its machine, else the computer.
                        .child(
                            text(move || row.with(|r| r.as_ref().map(|r| r.6.as_ref().map_or_else(|| here.clone(), |m| prefs.machine_name(&computer, m))).unwrap_or_default()))
                                .text_size(theme::CAPTION)
                                .text_color(theme::DIM)
                                .number_of_lines(1),
                        ),
                )
                .child(when(move || row.with(|r| r.as_ref().is_some_and(|r| r.7 > 0)), move || {
                    // The sub-agents' fold; folded, how many.
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(3.0))
                        .px(px(6.0))
                        .h(px(28.0))
                        .rounded(px(6.0))
                        .active(|s| s.bg(theme::HOVER))
                        .role(AccessibilityRole::Button)
                        .label(t!("Fold or unfold its sub-agents"))
                        .on_press(move || if let Some(r) = row.get_untracked() { fold(r.0) })
                        .child(dynamic(move || ui::glyph(if row.with(|r| r.as_ref().is_some_and(|r| r.8)) { "chevron-down" } else { "chevron-right" }, 12.0, theme::DIM).into_element()))
                        .child(text(move || row.with(|r| r.as_ref().filter(|r| !r.8).map(|r| r.7.to_string()).unwrap_or_default())).text_size(theme::SMALL).text_color(theme::DIM))
                }))
                .child(text(move || agent().map(|a| elapsed(a.since, now.get())).unwrap_or_default()).text_size(theme::CAPTION).text_color(theme::DIM))
                .child(when(move || row.with(|r| r.as_ref().is_some_and(|r| r.3)), || ui::glyph("pin", 13.0, theme::DIM)))
                .child(ui::status_mark(move || row.with(|r| r.as_ref().map(|r| r.1)), move || row.with(|r| r.as_ref().is_some_and(|r| r.9)))),
        )
}

/// Show all the idle ones, or Show less, in line with the rows' words.
fn more_row(l: ReadSignal<Line>, all: Signal<bool>) -> impl IntoElement {
    let more = computed(move || match l.get() {
        Line::More { count, open, edge } => (count, open, edge),
        _ => Default::default(),
    });
    let said = move || more.with(|m| if m.1 { t!("Show less").to_owned() } else { t!("Show all {}", m.0) });
    on_card(move || more.with(|m| m.2), true).child(
        view()
            .flex_row()
            .items_center()
            .pl(px(46.0))
            .h(px(44.0))
            .active(|s| s.bg(theme::HOVER))
            .role(AccessibilityRole::Button)
            .label(said)
            .on_press(move || all.update(|a| *a = !*a))
            .child(text(said).text_size(theme::CALLOUT).text_color(theme::ACCENT)),
    )
}

/// A sub-agent under its agent, on the tree's lines: a tap shows its
/// pane, or peeks at it.
fn sub_row(live: Live, l: ReadSignal<Line>, inline: bool) -> impl IntoElement {
    let sub = computed(move || match l.get() {
        Line::Sub { sub, last, compact, .. } => Some((sub, last, compact)),
        _ => None,
    });
    let edge = move || l.with(|l| match l { Line::Sub { edge, .. } => *edge, _ => Edge::default() });
    let address = move || sub.with_untracked(|s| s.as_ref().map(|s| s.0.address.clone()).unwrap_or_default());
    on_card(edge, false).child(view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .pl(move || px(24.0 + 16.0 * sub.with(|s| s.as_ref().map_or(0, |s| s.0.depth)) as f32))
        .pr(px(12.0))
        .h(px(40.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(move || sub.with(|s| s.as_ref().map(|s| t!("{}, sub-agent", s.0.name)).unwrap_or_default()))
        .on_press(move || {
            let address = address();
            let shown = live.view.with_untracked(|v| v.agents.iter().any(|a| a.address == address));
            if shown {
                live.run("agent.focus", json!({ "terminal": address }));
                if !inline {
                    nav::workspace();
                }
            } else {
                nav::push(Route::Peek(address));
            }
        })
        .child(text(move || if sub.with(|s| s.as_ref().is_some_and(|s| s.1)) { "└" } else { "├" }).text_size(theme::FOOTNOTE).text_color(theme::DIM))
        .child(ui::program_icon(move || sub.with(|s| s.as_ref().map(|s| s.0.program.clone()).unwrap_or_default()), 15.0))
        .child(text(move || sub.with(|s| s.as_ref().map(|s| s.0.name.clone()).unwrap_or_default())).flex_1().text_size(theme::CALLOUT).text_color(theme::MUTED).number_of_lines(1))
        .child(ui::status_mark(move || sub.with(|s| s.as_ref().and_then(|s| s.0.status)), move || sub.with(|s| s.as_ref().is_some_and(|s| s.2)))))
}

/// A terminal without a pane, shown as it is: keep it (a pane of its own,
/// in a new tab) or go to the pane that shows it.
pub fn peek_page(live: Live, address: String) -> nsh_core::element::Screen {
    let world = live.view;
    let name = {
        let address = address.clone();
        move || {
            world.with(|v| {
                v.terminal(&address)
                    .and_then(|t| ["name", "template"].iter().find_map(|k| t[*k].as_str()).or(t["program"]["name"].as_str()).map(str::to_owned))
                    .unwrap_or_else(|| address.clone())
            })
        }
    };
    let in_pane = {
        let address = address.clone();
        computed(move || world.with(|v| v.spaces.iter().flat_map(|s| &s.tabs).flat_map(|t| &t.panes).any(|p| p.terminal.as_deref() == Some(&address))))
    };
    let act = {
        let (live, address) = (live.clone(), address.clone());
        move || {
            if in_pane.get_untracked() {
                live.run("pane.focus", json!({ "terminal": address }));
            } else {
                live.run("terminal.keep", json!({ "terminal": address }));
            }
            nav::workspace();
        }
    };
    ui::page(name)
        .header_right(
            view()
                .px(px(12.0))
                .h(px(40.0))
                .justify_center()
                .role(AccessibilityRole::Button)
                .on_press(act)
                .child(text(move || if in_pane.get() { t!("Go to") } else { t!("Keep") }).text_size(theme::BODY).text_color(theme::ACCENT)),
        )
        .child(crate::terminal::terminal(live.engine.clone(), address, crate::terminal::Swipes::Nothing))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    fn agent() -> herdc_engine::AgentView {
        serde_json::from_value(json!({ "address": "t1", "pane": "p1", "space": "s1", "space_name": "Project", "machine": null,
            "tab": "tab", "tab_index": 1, "tab_name": "1", "status": "working", "app": null, "program": {},
            "title": "", "seq": 1, "pinned": false, "focused": false })).unwrap()
    }

    #[test]
    fn numeric_details_only_disambiguate_and_kept_status_wins() {
        let mut a = agent();
        let mut view = herdc_engine::View { agents: vec![a.clone()], ..Default::default() };
        assert_eq!(super::label(&a, &view), "Project");
        let mut b = a.clone();
        b.address = "t2".into();
        b.tab_name = "2".into();
        b.status = "blocked";
        view.agents.push(b.clone());
        assert_eq!(super::label(&a, &view), "Project · 1");
        assert_eq!(super::label(&b, &view), "Project · 2");
        view.agents[1].title = "Review".into();
        assert_eq!(super::label(&a, &view), "Project");
        view.agents[1].title.clear();
        let terminals = [json!({"address": "t2", "parent": "t1", "state": "live", "status": "working"})];
        assert_eq!(super::listed_subs(&terminals, &view.agents, "t1")[0].status, Some("blocked"));
        view.agents[1].status = "done";
        assert_eq!(super::listed_subs(&terminals, &view.agents, "t1")[0].status, Some("idle"));
        a.title = "~/project".into();
        view.agents = vec![a.clone()];
        assert_eq!(super::label(&a, &view), "Project");
    }

    #[test]
    fn named_tabs_win_and_splits_use_each_agent_title() {
        let mut a = agent();
        a.title = "Fix tests".into();
        let root = herdc_model::model::Tab::new("tab", "Review", "p1", "t1").root;
        let mut view: herdc_engine::View = serde_json::from_value(json!({
            "link": {"state": "connecting"}, "home": "", "machines": [], "agents": [a], "terminals": [], "focused": null,
            "spaces": [{"id": "s1", "index": 1, "name": "Project", "named": true, "folder": "", "machine": null,
                "branch": null, "active": true, "status": null, "tabs": [{"id": "tab", "index": 1, "name": "Review", "named": true,
                    "active": true, "status": null, "settled": false, "settled_at": null, "changed_at": null, "working_since": null,
                    "focused": "p1", "zoomed": null, "root": root, "panes": []}]}]
        })).unwrap();
        view.agents[0].tab_name = "Review".into();
        assert_eq!(super::label(&view.agents[0], &view), "Project · Review");
        let mut other = view.agents[0].clone();
        other.address = "t2".into();
        other.title = "Check code".into();
        view.agents.push(other);
        assert_eq!(super::label(&view.agents[0], &view), "Project · Fix tests");
        assert_eq!(super::label(&view.agents[1], &view), "Project · Check code");
        view.agents[1].title.clear();
        view.agents[1].app = Some("Claude".into());
        assert_eq!(super::label(&view.agents[1], &view), "Project · Claude");
    }

    #[test]
    fn rows_say_the_space_once() {
        let mut a = agent();
        (a.space_name, a.title, a.program) = ("nova".into(), "π - nova".into(), json!({"name": "Pi"}));
        let mut b = a.clone();
        (b.address, b.title) = ("t2".into(), "Fix the build | nova".into());
        let mut c = a.clone();
        (c.address, c.space, c.space_name, c.title) = ("t3".into(), "s2".into(), "graph".into(), "π - yonder".into());
        let view = herdc_engine::View { agents: vec![a, b, c], ..Default::default() };
        assert_eq!(super::label(&view.agents[0], &view), "nova · Pi");
        assert_eq!(super::label(&view.agents[1], &view), "nova · Fix the build");
        // Pi in another folder: still Pi.
        assert_eq!(super::label(&view.agents[2], &view), "graph · Pi");
    }

    /// The list's lines, shortened: headings as `# label count`, agents
    /// by address, sub-agents as `- address`.
    fn shown(view: &herdc_engine::View, grouped: bool, folded: &[&str], held: &mut Option<(String, u8)>) -> Vec<String> {
        let folded: Vec<String> = folded.iter().map(|s| s.to_string()).collect();
        super::lines(view, grouped, &folded, false, held)
            .into_iter()
            .map(|l| match l {
                super::Line::Heading { label, word, count, .. } => format!("# {} {count}", word.map_or(label, str::to_owned)),
                super::Line::Agent { agent, .. } => agent.address.clone(),
                super::Line::Sub { sub, .. } => format!("- {}", sub.address),
                super::Line::More { count, open, .. } => format!("+ {count} {open}"),
            })
            .collect()
    }

    #[test]
    fn many_idle_agents_show_the_first_few_then_all() {
        let idle = |n: u64| at(&format!("t{n}"), "A", "idle", n, n);
        let view = herdc_engine::View { agents: (1..=8).map(idle).collect(), ..Default::default() };
        let addresses = |all: bool| super::lines(&view, false, &[], all, &mut None).into_iter().map(|l| match l {
            super::Line::Agent { agent, .. } => agent.address.clone(),
            super::Line::More { count, open, edge } => format!("+ {count} {open} {}", edge.bottom),
            _ => "#".into(),
        }).collect::<Vec<_>>();
        assert_eq!(addresses(false), ["#", "t8", "t7", "t6", "t5", "t4", "+ 8 false true"]);
        assert_eq!(addresses(true).len(), 10);
        // One past the few shows too; grouped by space, all show.
        let view = herdc_engine::View { agents: (1..=6).map(idle).collect(), ..Default::default() };
        assert_eq!(super::lines(&view, false, &[], false, &mut None).len(), 7);
        let view = herdc_engine::View { agents: (1..=8).map(idle).collect(), ..Default::default() };
        assert_eq!(super::lines(&view, true, &[], false, &mut None).len(), 9);
    }

    fn at(address: &str, space: &str, status: &'static str, since: u64, seq: u64) -> herdc_engine::AgentView {
        let mut a = agent();
        (a.address, a.space, a.space_name, a.status, a.since, a.seq) = (address.into(), space.into(), space.into(), status, since, seq);
        a
    }

    #[test]
    fn a_parent_ranks_as_its_most_urgent_sub_agent_and_the_latest_change_goes_first() {
        let view = herdc_engine::View {
            agents: vec![at("t1", "A", "idle", 5, 1), at("t2", "B", "working", 1, 2), at("t3", "C", "working", 9, 3)],
            terminals: vec![json!({"address": "t9", "parent": "t1", "state": "live", "status": "blocked"})],
            ..Default::default()
        };
        assert_eq!(shown(&view, false, &[], &mut None), ["# Needs you 1", "t1", "- t9", "# Working 2", "t3", "t2"]);
        // Folded, an agent's sub-agents go; a folded group shows only its heading.
        assert_eq!(shown(&view, false, &["t1", "sec-2"], &mut None), ["# Needs you 1", "t1", "# Working 2"]);
    }

    #[test]
    fn a_done_agent_looked_at_stays_with_done_until_left() {
        let mut view = herdc_engine::View { agents: vec![at("t1", "A", "done", 5, 1), at("t2", "B", "idle", 1, 2)], ..Default::default() };
        view.agents[0].focused = true;
        let mut held = None;
        assert_eq!(shown(&view, false, &[], &mut held), ["# Done 1", "t1", "# Idle 1", "t2"]);
        // Shown, it turns idle: still with Done.
        view.agents[0].status = "idle";
        assert_eq!(shown(&view, false, &[], &mut held), ["# Done 1", "t1", "# Idle 1", "t2"]);
        // Left, it goes to Idle.
        (view.agents[0].focused, view.agents[1].focused) = (false, true);
        assert_eq!(shown(&view, false, &[], &mut held), ["# Idle 2", "t1", "t2"]);
    }

    #[test]
    fn grouped_by_space_keeps_the_order_and_pinned_ones_lead() {
        let mut view = herdc_engine::View { agents: vec![at("t1", "A", "idle", 5, 1), at("t2", "A", "blocked", 1, 2), at("t3", "B", "done", 9, 3)], ..Default::default() };
        view.agents[2].pinned = true;
        assert_eq!(shown(&view, true, &[], &mut None), ["# Pinned 1", "t3", "# A 2", "t1", "t2"]);
        assert_eq!(shown(&view, false, &[], &mut None), ["# Pinned 1", "t3", "# Needs you 1", "t2", "# Idle 1", "t1"]);
    }

    #[test]
    fn sub_agents_come_depth_first_and_finished_ones_go() {
        let terminals = [
            json!({"address": "t2", "parent": "t1", "name": "helper", "status": "working", "state": "live"}),
            json!({"address": "t3", "parent": "t2", "template": "reviewer", "status": "error", "state": "live"}),
            json!({"address": "t4", "parent": "t1", "program": {"name": "Codex"}, "state": "live"}),
            json!({"address": "t5", "parent": "t1", "name": "gone", "state": "finished"}),
        ];
        let shown: Vec<_> = super::subs(&terminals, "t1").into_iter().map(|s| (s.depth, s.name, s.status)).collect();
        assert_eq!(
            shown,
            [(0, "helper".into(), Some("working")), (1, "reviewer".into(), Some("blocked")), (0, "Codex".into(), None)]
        );
    }
}
