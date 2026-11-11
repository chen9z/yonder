//! The workspace: the shown space's tabs, and its panes as the phone can
//! show them. On a phone that is one pane at a time, the others a tap
//! away; on a tablet (or a phone turned sideways) the split as the desktop
//! lays it out. Spaces and agents are on the home page, back from here.

use crate::live::{waits_on, Live};
use crate::nav::{self, Route};
use crate::terminal::{terminal, Swipes};
use crate::{agents, menu, theme, ui};
use herdc_engine::{rects, Link, PaneView, Rect, TabView};
use nsh_core::prelude::*;

/// Wide enough to show a tab's split.
fn wide() -> bool {
    window().width.0 >= 720.0
}

pub fn workspace_page(live: Live) -> impl IntoElement {
    let world = live.view;
    let space = computed(move || world.with(|v| v.active_space().map(|s| (s.id.clone(), s.name.clone(), s.status))));
    let tab = computed(move || world.with(|v| v.active_tab().cloned()));
    let more_open = signal(false);
    view()
        .flex_1()
        .bg(theme::BACKGROUND)
        // Turned sideways, clear of the system's bar and the camera.
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
        .child(header(live.clone(), space, tab, more_open))
        .child(link_banner(live.clone()))
        .child(tab_strip(live.clone()))
        .child(
            // Above the keyboard: a terminal typed in fits what is left.
            view().flex_1().pb(ui::bottom_inset).child(
                when(move || tab.with(Option::is_some), {
                    let live = live.clone();
                    move || panes(live.clone(), tab)
                })
                .otherwise({
                    let live = live.clone();
                    move || empty(live.clone())
                }),
            ),
        )
        .child(menu::more_sheet(live.clone(), more_open))
}

fn header(
    live: Live,
    space: Memo<Option<(String, String, Option<&'static str>)>>,
    tab: Memo<Option<TabView>>,
    more_open: Signal<bool>,
) -> impl IntoElement {
    let world = live.view;
    // Agents elsewhere that need you or are done: said on the way back.
    let waiting = computed(move || world.with(|v| v.agents.iter().filter(|a| matches!(a.status, "blocked" | "done") && v.active_space().is_none_or(|s| s.id != a.space)).count()));
    ui::top_bar()
        .child(
            view()
                .child(ui::icon_button("‹", t!("Spaces and agents"), nav::home).pr(px(4.0)))
                .child(when(move || waiting.get() > 0, move || {
                    view()
                        .absolute()
                        .top(px(4.0))
                        .right(px(2.0))
                        .min_w(px(16.0))
                        .h(px(16.0))
                        .px(px(4.0))
                        .rounded(px(8.0))
                        .bg(theme::BLOCKED)
                        .items_center()
                        .justify_center()
                        .pointer_events(PointerEvents::None)
                        .child(text(move || waiting.get().to_string()).text_size(px(10.0)).text_color(rgb(0xffffff)))
                })),
        )
        .child(
            view()
                .flex_1()
                .gap(px(1.0))
                .on_press(nav::home)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(ui::dot(move || space.with(|s| s.as_ref().and_then(|s| s.2))))
                        .child(
                            text(move || space.with(|s| s.as_ref().map_or_else(|| "Yonder".to_owned(), |s| s.1.clone())))
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::FOREGROUND)
                                .number_of_lines(1),
                        ),
                )
                .child(
                    text({
                        // A machine's space says which, before the computer it's reached through.
                        let id = live.computer.id();
                        let prefs = use_context::<crate::prefs::Prefs>().unwrap();
                        let computer = live.computer.name.clone();
                        move || {
                        let machine = world.with(|v| v.active_space().and_then(|s| s.machine.clone())).map_or(computer.clone(), |m| format!("{} · {computer}", prefs.machine_name(&id, &m)));
                        tab.with(|t| t.as_ref().map_or(machine.clone(), |t| format!("{} · {machine}", label(t))))
                    }})
                    .text_size(px(12.0))
                    .text_color(theme::MUTED)
                    .number_of_lines(1),
                ),
        )
        .child(ui::icon_button("⋯", t!("More"), move || {
            dismiss_keyboard();
            more_open.set(true)
        }))
}

/// Said while the core is out of reach; a password refused is asked for
/// again from here.
pub fn link_banner(live: Live) -> impl IntoElement {
    let world = live.view;
    let host = live.computer.host.clone();
    let refused = computed(move || world.with(|v| matches!(&v.link, Link::Lost { error } if crate::link::Lost::of(error) == crate::link::Lost::WrongPassword)));
    let said = computed(move || {
        world.with(|v| match &v.link {
            Link::Connected { .. } => None,
            Link::Connecting => Some(t!("Connecting to {}…", host)),
            Link::Lost { error } => Some(crate::link::Lost::of(error).said(&host)),
        })
    });
    when(move || said.with(Option::is_some), move || {
        view()
            .px(px(16.0))
            .py(px(8.0))
            .bg(theme::RAISED)
            .border_b_1()
            .border_color(theme::BORDER)
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(text(move || said.get().unwrap_or_default()).flex_1().text_size(px(13.0)).text_color(theme::MUTED).number_of_lines(2))
            .child(when(move || refused.get(), {
                let live = live.clone();
                move || {
                    let live = live.clone();
                    ui::ghost_button(t!("Enter the password"), move || live.ask_password())
                }
            }))
    })
}

/// The shown space's tabs, the shown one marked, and a new one.
fn tab_strip(live: Live) -> impl IntoElement {
    let world = live.view;
    let tabs = computed(move || {
        world.with(|v| {
            v.active_space()
                .map(|s| s.tabs.iter().filter(|t| !t.settled || t.active).map(|t| (t.id.clone(), if t.zoomed.is_some() { format!("{} Z", label(t)) } else { label(t) }, t.active, t.status, program(t))).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let new_tab = {
        let live = live.clone();
        move || live.run("tab.new", serde_json::json!({}))
    };
    when(move || !tabs.with(Vec::is_empty), move || {
        let live = live.clone();
        let new_tab = new_tab.clone();
        view()
            .h(px(40.0))
            .bg(theme::CHROME)
            .border_b_1()
            .border_color(theme::BORDER)
            .flex_row()
            .child(
                scroll().horizontal().flex_1().scrollbar(false).child(
                    view().flex_row().items_center().px(px(6.0)).gap(px(4.0)).child(each(
                        move || tabs.get(),
                        |t| t.0.clone(),
                        move |t, _| {
                            let live = live.clone();
                            let (open, settle) = (live.clone(), live.clone());
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .h(px(30.0))
                                .px(px(12.0))
                                .rounded(px(8.0))
                                .bg(move || if t.with(|t| t.2) { theme::now(theme::SELECTED) } else { Hsla::transparent_black() })
                                .active(|s| s.bg(theme::HOVER))
                                .on_press(move || open.run("tab.select", serde_json::json!({ "id": t.get_untracked().0 })))
                                .on_long_press(move || {
                                    let (live, id) = (settle.clone(), t.get_untracked().0);
                                    spawn(async move {
                                        let picked = ui::choose(t!("Tab"), &[("rename", t!("Rename")), ("settle", t!("Put away")), ("close", t!("Close"))]).await;
                                        match picked.as_deref() {
                                            Some("rename") => live.run("tab.rename", serde_json::json!({ "id": id })),
                                            Some("settle") => live.run("tab.settle", serde_json::json!({ "id": id })),
                                            Some("close") => live.run("tab.close", serde_json::json!({ "id": id })),
                                            _ => {}
                                        }
                                    });
                                })
                                .child(ui::program_icon(move || t.with(|t| t.4.clone()), 15.0))
                                .child(
                                    text(move || t.with(|t| t.1.clone()))
                                        .text_size(px(14.0))
                                        .text_color(move || if t.with(|t| t.2) { theme::now(theme::FOREGROUND) } else { theme::now(theme::MUTED) })
                                        .number_of_lines(1)
                                        .max_w(px(200.0)),
                                )
                                .child(ui::tab_status(move || t.with(|t| t.3)))
                        },
                    )),
                ),
            )
            .child(ui::icon_button("+", t!("New tab"), new_tab.clone()).h(px(40.0)))
    })
}

/// What a tab is called on a phone: its name if it was given one, else
/// what its agent works on (a number says little among several).
fn label(t: &TabView) -> String {
    let agent = || t.panes.iter().find(|p| p.status.is_some()).map(|p| ui::title(&p.title)).filter(|s| !s.is_empty());
    if t.named { t.name.clone() } else { agent().unwrap_or_else(|| t.name.clone()) }
}

/// What a tab runs, as its icon says: its agent's program, else its
/// first pane's.
// Adapted from herdr-app defaults/mods/tabs/main.js, Apache-2.0.
fn program(t: &TabView) -> serde_json::Value {
    let agent = t.panes.iter().find(|p| p.program["agent"].as_bool().unwrap_or(false));
    agent.or(t.panes.first()).map(|p| p.program.clone()).unwrap_or_default()
}

/// The pane shown on a phone: the zoomed one, else the focused one.
fn shown(t: &TabView) -> Option<&PaneView> {
    let id = t.zoomed.as_ref().or(t.focused.as_ref());
    id.and_then(|id| t.panes.iter().find(|p| &p.pane == id)).or(t.panes.first())
}

/// What a pane shows: a terminal, or what a mod shows there.
#[derive(Clone, Debug, PartialEq)]
enum Shows {
    Terminal(String),
    Mod(String),
    Nothing,
}

fn shows(p: &PaneView) -> Shows {
    match (&p.shown, &p.terminal, &p.mod_name) {
        (Some(t), _, _) | (None, Some(t), None) => Shows::Terminal(t.clone()),
        (_, _, Some(m)) => Shows::Mod(m.clone()),
        _ => Shows::Nothing,
    }
}

fn panes(live: Live, tab: Memo<Option<TabView>>) -> impl IntoElement {
    when(wide, {
        let live = live.clone();
        move || split(live.clone(), tab)
    })
    .otherwise(move || one_pane(live.clone(), tab))
}

/// One pane at a time, with the others named above it.
fn one_pane(live: Live, tab: Memo<Option<TabView>>) -> impl IntoElement {
    let pane = computed(move || tab.with(|t| t.as_ref().and_then(shown).map(|p| (p.pane.clone(), shows(p)))));
    let others = computed(move || {
        tab.with(|t| {
            t.as_ref()
                .filter(|t| t.panes.len() > 1 && t.zoomed.is_none())
                .map(|t| t.panes.iter().map(|p| Chip { pane: p.pane.clone(), title: ui::title(&p.title), focused: p.focused, status: p.status, program: p.program.clone() }).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let engine = live.engine.clone();
    view()
        .flex_1()
        .child(when(move || !others.with(Vec::is_empty), {
            let live = live.clone();
            move || pane_strip(live.clone(), others)
        }))
        .child(dynamic(move || match pane.get() {
            Some((_, Shows::Terminal(address))) => terminal(engine.clone(), address, Swipes::Tabs).into_element(),
            Some((_, Shows::Mod(name))) => mod_pane(live.clone(), name).into_element(),
            _ => view().into_element(),
        }))
}

/// A pane as the strip names it.
#[derive(Clone, Debug, PartialEq)]
struct Chip {
    pane: String,
    title: String,
    focused: bool,
    status: Option<&'static str>,
    program: serde_json::Value,
}

fn pane_strip(live: Live, panes: Memo<Vec<Chip>>) -> impl IntoElement {
    scroll().horizontal().scrollbar(false).bg(theme::CANVAS).border_b_1().border_color(theme::BORDER).child(
        view().flex_row().items_center().px(px(6.0)).py(px(4.0)).gap(px(4.0)).child(each(
            move || panes.get(),
            |p| p.pane.clone(),
            move |p, _| {
                let live = live.clone();
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(move || if p.with(|p| p.focused) { theme::now(theme::ACCENT) } else { theme::now(theme::BORDER) })
                    .active(|s| s.bg(theme::HOVER))
                    .on_press(move || live.run("pane.focus", serde_json::json!({ "pane": p.get_untracked().pane })))
                    .child(ui::program_icon(move || p.with(|p| p.program.clone()), 14.0))
                    .child(text(move || p.with(|p| p.title.clone())).text_size(px(13.0)).text_color(theme::FOREGROUND).number_of_lines(1).max_w(px(160.0)))
                    .child(ui::tab_status(move || p.with(|p| p.status)))
            },
        )),
    )
}

/// A pane where the split puts it.
#[derive(Clone, Debug, PartialEq)]
struct Block {
    pane: String,
    shows: Shows,
    focused: bool,
    at: Rect,
}

/// The tab's split, as the desktop lays it out.
fn split(live: Live, tab: Memo<Option<TabView>>) -> impl IntoElement {
    let blocks = computed(move || {
        tab.with(|t| {
            let Some(t) = t else { return Vec::new() };
            let placed = match &t.zoomed {
                Some(z) => vec![(z.clone(), Rect::SQUARE)],
                None => rects(&t.root, Rect::SQUARE),
            };
            placed
                .into_iter()
                .filter_map(|(id, at)| t.panes.iter().find(|p| p.pane == id).map(|p| Block { pane: id, shows: shows(p), focused: p.focused, at }))
                .collect::<Vec<_>>()
        })
    });
    let seams = computed(move || tab.with(|t| t.as_ref().filter(|t| t.zoomed.is_none()).map(|t| herdc_engine::seams(&t.root, Rect::SQUARE)).unwrap_or_default()));
    let area = signal((0.0f32, 0.0f32));
    let engine = live.engine.clone();
    let resize = live.clone();
    view()
        .flex_1()
        .on_layout(move |b: &Bounds<Points>| area.set((b.size.width.0, b.size.height.0)))
        .child(each(
        move || blocks.get(),
        |b| (b.pane.clone(), format!("{:?}", b.shows)),
        move |b, _| {
            let (engine, live, focus) = (engine.clone(), live.clone(), live.clone());
            // Rects are in thousandths of the tab.
            let at = |f: fn(&Rect) -> f32| move || percentage(b.with(|b| f(&b.at)) / 1000.0);
            let shown = b.get_untracked();
            view()
                .absolute()
                .left(at(|r| r.x))
                .top(at(|r| r.y))
                .w(at(|r| r.w))
                .h(at(|r| r.h))
                .p(px(1.0))
                .child(
                    view()
                        .flex_1()
                        .border_1()
                        .border_color(move || if b.with(|b| b.focused) { theme::now(theme::ACCENT) } else { theme::now(theme::BORDER) })
                        .on_touch(move |_: &nsh_core::element::TouchEvent| {
                            if !b.with(|b| b.focused) {
                                focus.run("pane.focus", serde_json::json!({ "pane": b.get_untracked().pane }));
                            }
                        })
                        .child(match shown.shows {
                            Shows::Terminal(address) => terminal(engine, address, Swipes::Tabs).into_element(),
                            Shows::Mod(name) => mod_pane(live.clone(), name).into_element(),
                            Shows::Nothing => view().into_element(),
                        }),
                )
        },
    ))
    .child(each(move || seams.get(), |s| format!("{} {}", s.before, s.after), move |s, _| seam(resize.clone(), s, area)))
}

/// Where two panes meet, to drag: the split follows the finger, and is
/// shared anew (as on the desktop) when it lifts.
fn seam(live: Live, s: ReadSignal<herdc_engine::Seam>, area: Signal<(f32, f32)>) -> impl IntoElement {
    // How far it was dragged, in thousandths of the tab.
    let moved = signal(0.0f32);
    let row = s.with_untracked(|s| s.row);
    let per_point = move || {
        let (w, h) = area.get_untracked();
        1000.0 / if row { w } else { h }.max(1.0)
    };
    let at = move || {
        let s = s.get();
        let (lo, hi) = (s.from + (s.to - s.from) * 0.1, s.to - (s.to - s.from) * 0.1);
        (s.at + moved.get()).clamp(lo, hi)
    };
    let thick = 24.0;
    let place = move |along: f32, across: (f32, f32)| {
        let (w, h) = area.get();
        if row {
            (along / 1000.0 * w - thick / 2.0, across.0 / 1000.0 * h, thick, across.1 / 1000.0 * h)
        } else {
            (across.0 / 1000.0 * w, along / 1000.0 * h - thick / 2.0, across.1 / 1000.0 * w, thick)
        }
    };
    let frame = move || place(at(), s.with(|s| s.across));
    view()
        .absolute()
        .left(move || px(frame().0))
        .top(move || px(frame().1))
        .w(move || px(frame().2))
        .h(move || px(frame().3))
        .items_center()
        .justify_center()
        .role(AccessibilityRole::Adjustable)
        .label(t!("Pane size"))
        .claim_touches(TouchClaim::on_start())
        .on_responder_move(move |e: &ResponderEvent| {
            let d = if row { e.gesture.delta.x.0 } else { e.gesture.delta.y.0 };
            moved.set(d * per_point());
        })
        .on_responder_release(move |_: &ResponderEvent| {
            let s = s.get_untracked();
            let to = at();
            moved.set(0.0);
            if (to - s.at).abs() < 5.0 {
                return;
            }
            let (before, after) = s.weights(to);
            live.run("layout.resize", serde_json::json!({ "id": s.before, "weight": before }));
            live.run("layout.resize", serde_json::json!({ "id": s.after, "weight": after }));
        })
        .child(
            view()
                .rounded(px(2.0))
                .bg(move || if moved.get() != 0.0 { theme::now(theme::ACCENT) } else { theme::now(theme::BORDER) })
                .w(px(if row { 4.0 } else { 36.0 }))
                .h(px(if row { 36.0 } else { 4.0 })),
        )
}

/// A mod's pane: the built-in ones the phone has its own page for open
/// it; the rest are the desktop's.
fn mod_pane(live: Live, name: String) -> impl IntoElement {
    let place = live.view.with_untracked(|v| v.location());
    let page = match name.as_str() {
        "files" => place.map(Route::Files),
        "changes" => place.map(Route::Changes),
        _ => None,
    };
    if name == "agents" {
        return agents::agent_list(live).into_element();
    }
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .p(px(24.0))
        .bg(theme::CANVAS)
        .child(text(t!("The {} pane", name)).text_size(px(16.0)).text_color(theme::FOREGROUND))
        .child(text(t!("It shows on the desktop.")).text_size(px(13.0)).text_color(theme::MUTED))
        .children(page.map(|r| ui::ghost_button(t!("Open"), move || nav::push(r.clone()))))
        .into_element()
}

/// What the workspace is without a space: still reaching the computer or
/// reading its layout, or empty, with the way to start.
fn empty(live: Live) -> impl IntoElement {
    let world = live.view;
    // Its own machine's layout: spaces can be made once it is read.
    let said = computed(move || {
        world.with(|v| {
            let own = v.machines.iter().find(|m| m.name.is_none());
            match (v.connected(), own.map(|m| (m.layout, m.error.clone()))) {
                (false, _) => Err(t!("Not connected").to_owned()),
                (true, Some(("ready", _))) => Ok(t!("No spaces yet").to_owned()),
                (true, Some(("unavailable", error))) => Err(error.unwrap_or_else(|| t!("Herdr's layout is unavailable here").into())),
                _ => Err(t!("Loading spaces…").to_owned()),
            }
        })
    });
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(16.0))
        .p(px(24.0))
        .child(
            text(move || said.with(|s| s.clone().unwrap_or_else(|e| e)))
                .text_size(px(18.0))
                .text_center()
                .text_color(theme::FOREGROUND),
        )
        .child(when(move || said.with(Result::is_ok), move || {
            let live = live.clone();
            ui::button(t!("New space"), move || live.run("space.new", serde_json::json!({})))
        }))
}

/// Toasts over the page: newest at the bottom; a tap runs what they offer.
/// A waiting agent has a reminder until it is shown or no longer blocked.
pub fn notes(live: Live) -> impl IntoElement {
    let (all, world) = (live.notes, live.view);
    let notes = computed(move || {
        world.with(|v| {
            let shown = v.focused.as_ref().and_then(|f| f.terminal.clone());
            let waiting = |a: &String| v.terminals.iter().any(|t| t["address"] == a.as_str() && matches!(t["status"].as_str(), Some("blocked" | "error")));
            all.with(|all| all.iter().filter(|(_, n)| waits_on(n).is_none_or(|a| waiting(&a) && shown.as_ref() != Some(&a))).cloned().collect::<Vec<_>>())
        })
    });
    view()
        .absolute()
        .left(px(12.0))
        .right(px(12.0))
        .bottom(move || ui::bottom_inset() + px(56.0))
        .gap(px(8.0))
        .pointer_events(PointerEvents::BoxNone)
        .child(each(
            move || notes.get(),
            |(id, _)| *id,
            move |n, _| {
                let live = live.clone();
                view()
                    .px(px(14.0))
                    .py(px(10.0))
                    .rounded(px(12.0))
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(move || if n.with(|n| n.1.level == "error") { theme::now(theme::BLOCKED) } else { theme::now(theme::BORDER) })
                    .shadow_lg()
                    .gap(px(8.0))
                    .child(
                        view()
                            .flex_row()
                            .items_start()
                            .child(
                                view()
                                    .flex_1()
                                    .on_press({
                                        let live = live.clone();
                                        move || {
                                            let (id, note) = n.get_untracked();
                                            if let Some(action) = note.action {
                                                live.run(&action.name, action.args);
                                            }
                                            live.notes.update(|all| all.retain(|(i, _)| *i != id));
                                        }
                                    })
                                    .child(text(move || n.with(|n| n.1.title.clone())).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND))
                                    .child(text(move || n.with(|n| n.1.body.clone().unwrap_or_default())).text_size(px(13.0)).text_color(theme::MUTED).number_of_lines(3)),
                            )
                            .child({
                                let notes = live.notes;
                                ui::icon_button("✕", t!("Not now"), move || {
                                    let id = n.with_untracked(|n| n.0);
                                    notes.update(|all| all.retain(|(i, _)| *i != id));
                                })
                                .w(px(36.0))
                                .h(px(36.0))
                                .mt(px(-6.0))
                                .mr(px(-8.0))
                            }),
                    )
            },
        ))
}
