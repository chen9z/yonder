//! The workspace: the shown space's tabs, and its panes as the phone can
//! show them. On a phone that is one pane at a time, the others a tap
//! away; on a tablet (or a phone turned sideways) the split as the desktop
//! lays it out. Spaces and agents are on the home page, back from here.

use crate::live::{waits_on, Live};
use crate::nav::{self, Route};
use crate::terminal::{terminal, Swipes};
use crate::{agents, menu, theme, ui};
use herdc_engine::{rects, AgentView, Link, PaneView, Rect, TabView};
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
        .child(tab_strip(live.clone()))
        .child(
            // Above the keyboard: a terminal typed in fits what is left.
            view()
                .flex_1()
                .pb(ui::bottom_inset)
                .child(
                    when(move || tab.with(Option::is_some), {
                        let live = live.clone();
                        move || panes(live.clone(), tab)
                    })
                    .otherwise({
                        let live = live.clone();
                        move || empty(live.clone())
                    }),
                )
                // Over the panes, not above them: coming and going, it
                // would resize every terminal shown (and the desktop's).
                .child(view().absolute().top(px(0.0)).left(px(0.0)).right(px(0.0)).pointer_events(PointerEvents::BoxNone).child(link_banner(live.clone())))
                .child(tab_said(live.clone())),
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
    let prefs = use_context::<crate::prefs::Prefs>().unwrap();
    // Agents elsewhere that need you, else that are done: said on the way
    // back, in the color of what they are.
    let waiting = computed(move || world.with(|v| waiting_elsewhere(v)));
    ui::top_bar()
        .child(
            view()
                .child(ui::icon_button("‹", t!("Spaces and agents"), nav::home).pr(px(4.0)))
                .child(when(move || waiting.get().0 > 0, move || {
                    view()
                        .absolute()
                        .top(px(4.0))
                        .right(px(2.0))
                        .min_w(px(16.0))
                        .h(px(16.0))
                        .px(px(4.0))
                        .rounded(px(8.0))
                        .bg(move || theme::now(theme::status(waiting.get().1)))
                        .items_center()
                        .justify_center()
                        .pointer_events(PointerEvents::None)
                        .child(text(move || waiting.get().0.to_string()).text_size(px(10.0)).text_color(rgb(0xffffff)))
                })),
        )
        .child(
            view()
                .flex_1()
                .gap(px(1.0))
                .role(AccessibilityRole::Button)
                .label(t!("Switch space"))
                .on_press({
                    let live = live.clone();
                    move || switch_space(live.clone(), prefs)
                })
                .on_long_press({
                    let live = live.clone();
                    move || {
                        if let Some(id) = tab.with(|t| t.as_ref().map(|t| t.id.clone())) {
                            tab_menu(live.clone(), id);
                        }
                    }
                })
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
        // The new-menu: a terminal, an agent, a space or a worktree.
        .child(ui::icon_button("+", t!("New"), {
            let live = live.clone();
            move || {
                dismiss_keyboard();
                spawn(crate::palette::new_terminal(live.clone()));
            }
        }))
        .child(ui::icon_button("⋯", t!("More"), move || {
            dismiss_keyboard();
            more_open.set(true)
        }))
}

/// How many agents in other spaces wait for the person, and what they
/// wait as: needing you before done.
fn waiting_elsewhere(v: &herdc_engine::View) -> (usize, &'static str) {
    let elsewhere = |a: &&AgentView| v.active_space().is_none_or(|s| s.id != a.space);
    let count = |status: &str| v.agents.iter().filter(elsewhere).filter(|a| a.status == status).count();
    match count("blocked") {
        0 => (count("done"), "done"),
        blocked => (blocked, "blocked"),
    }
}

/// A tab's menu, as the desktop's tab bar offers it.
fn tab_menu(live: Live, id: String) {
    spawn(async move {
        let picked = ui::choose(t!("Tab"), &[("rename", t!("Rename")), ("settle", t!("Put away")), ("close", t!("Close"))]).await;
        let command = match picked.as_deref() {
            Some("rename") => "tab.rename",
            Some("settle") => "tab.settle",
            Some("close") => "tab.close",
            _ => return,
        };
        live.run(command, serde_json::json!({ "id": id }));
    });
}

/// The spaces to go to, from the title: this computer's first, the shown
/// one checked; or all of them, at home.
fn switch_space(live: Live, prefs: crate::prefs::Prefs) {
    let computer = live.computer.id();
    let mut spaces: Vec<(bool, String, String, String)> = live.view.with_untracked(|v| {
        v.spaces
            .iter()
            .map(|s| {
                let machine = s.machine.as_deref().map(|m| prefs.machine_name(&computer, m)).unwrap_or_default();
                let mut name = if machine.is_empty() { s.name.clone() } else { format!("{} · {machine}", s.name) };
                if s.active {
                    name.push_str(" ✓");
                }
                (s.machine.is_some(), machine, s.id.clone(), name)
            })
            .collect()
    });
    spaces.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    spawn(async move {
        let mut actions: Vec<(&str, &str)> = spaces.iter().map(|(_, _, id, name)| (id.as_str(), name.as_str())).collect();
        actions.push(("\0home", t!("All spaces and agents")));
        match ui::choose(t!("Spaces"), &actions).await.as_deref() {
            Some("\0home") => nav::home(),
            Some(id) => live.run("space.select", serde_json::json!({ "id": id })),
            None => {}
        }
    });
}

/// A tab as the strip shows it.
#[derive(Clone, Debug, PartialEq)]
struct TabChip {
    id: String,
    label: String,
    status: Option<&'static str>,
    active: bool,
}

/// The tabs the space shows (a tab put away, unless it is the shown one).
fn shown_tabs(v: &herdc_engine::View) -> Vec<&TabView> {
    v.active_space().map(|s| s.tabs.iter().filter(|t| !t.settled || t.active).collect()).unwrap_or_default()
}

/// The space's tabs put away (the shown one aside): id and label.
fn settled_tabs(v: &herdc_engine::View) -> Vec<(String, String)> {
    v.active_space().map(|s| s.tabs.iter().filter(|t| t.settled && !t.active).map(|t| (t.id.clone(), label(t))).collect()).unwrap_or_default()
}

/// The space's tabs, while it has more than one: a tap shows one, a hold
/// offers its menu. The shown one is kept in view. Tabs put away are
/// counted at the end, a tap bringing one back.
fn tab_strip(live: Live) -> impl IntoElement {
    let world = live.view;
    let tabs = computed(move || world.with(|v| shown_tabs(v).into_iter().map(|t| TabChip { id: t.id.clone(), label: label(t), status: t.status, active: t.active }).collect::<Vec<_>>()));
    let settled = computed(move || world.with(|v| settled_tabs(v)));
    when(move || tabs.with(|t| t.len() > 1) || settled.with(|s| !s.is_empty()), move || {
        let live = live.clone();
        let back = live.clone();
        let strip = element_ref();
        // Where each chip is along the strip, to bring the shown one in.
        let at = stored(std::collections::HashMap::<String, f32>::new());
        let width = stored(0.0f32);
        let active = computed(move || tabs.with(|t| t.iter().find(|t| t.active).map(|t| t.id.clone())));
        // Brought in once each time another is shown (when it is laid out,
        // if not yet): a strip scrolled by hand stays where it was put.
        let pending = stored(false);
        let bring = move || {
            if !pending.get() {
                return;
            }
            if let Some(x) = active.with_untracked(|a| a.as_ref().and_then(|a| at.with(|at| at.get(a).copied()))) {
                pending.replace(false);
                strip.scroll_to(point(px((x - width.get() / 3.0).max(0.0)), px(0.0)), true);
            }
        };
        effect(move || {
            active.with(|_| ());
            pending.replace(true);
            bring();
        });
        scroll()
            .horizontal()
            .scrollbar(false)
            .element_ref(&strip)
            .on_layout(move |b: &Bounds<Points>| { width.replace(b.size.width.0); })
            .bg(theme::CHROME)
            .border_b_1()
            .border_color(theme::BORDER)
            .child(view().flex_row().items_center().px(px(6.0)).h(px(40.0)).gap(px(2.0)).child(each(
                move || tabs.get(),
                |t| t.id.clone(),
                move |t, _| {
                    let (press, hold) = (live.clone(), live.clone());
                    let id = t.with_untracked(|t| t.id.clone());
                    view()
                        .on_layout(move |b: &Bounds<Points>| {
                            at.with(|at| at.insert(id.clone(), b.origin.x.0));
                            bring();
                        })
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(34.0))
                        .px(px(12.0))
                        .rounded(px(8.0))
                        .bg(move || if t.with(|t| t.active) { theme::now(theme::SELECTED) } else { Hsla::transparent_black() })
                        .active(|s| s.bg(theme::HOVER))
                        .role(AccessibilityRole::Tab)
                        .label(move || t.with(|t| t.label.clone()))
                        .accessibility_state(move || AccessibilityState { selected: t.with(|t| t.active), ..Default::default() })
                        .on_press(move || {
                            let t = t.get_untracked();
                            if !t.active {
                                press.run("tab.select", serde_json::json!({ "id": t.id }));
                            }
                        })
                        .on_long_press(move || tab_menu(hold.clone(), t.with_untracked(|t| t.id.clone())))
                        .child(ui::tab_status(move || t.with(|t| t.status)))
                        .child(
                            text(move || t.with(|t| t.label.clone()))
                                .max_w(px(180.0))
                                .number_of_lines(1)
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || if t.with(|t| t.active) { theme::now(theme::FOREGROUND) } else { theme::now(theme::MUTED) }),
                        )
                },
            ))
            .child(when(move || settled.with(|s| !s.is_empty()), move || {
                let back = back.clone();
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .h(px(34.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .active(|s| s.bg(theme::HOVER))
                    .role(AccessibilityRole::Button)
                    .label(move || t!("{} put away", settled.with(Vec::len)))
                    .on_press(move || {
                        let (live, tabs) = (back.clone(), settled.get_untracked());
                        spawn(async move {
                            let actions: Vec<(&str, &str)> = tabs.iter().map(|(id, label)| (id.as_str(), label.as_str())).collect();
                            // Shown, it comes back to the lists.
                            if let Some(id) = ui::choose(t!("Put away"), &actions).await {
                                live.run("tab.select", serde_json::json!({ "id": id }));
                            }
                        });
                    })
                    .child(ui::glyph("archive", 14.0, theme::DIM))
                    .child(text(move || settled.with(Vec::len).to_string()).text_size(px(13.0)).text_color(theme::DIM))
            })))
    })
}

/// Said a moment when the shown tab changes (a swipe says nothing else):
/// which of how many, and its name.
fn tab_said(live: Live) -> impl IntoElement {
    let world = live.view;
    let now = computed(move || {
        world.with(|v| {
            let tabs = shown_tabs(v);
            let at = tabs.iter().position(|t| t.active)?;
            let space = v.active_space()?.id.clone();
            Some((space, tabs[at].id.clone(), format!("{}/{} · {}", at + 1, tabs.len(), label(tabs[at]))))
        })
    });
    let said = signal(None::<String>);
    let before = stored(None::<(String, String)>);
    let turn = stored(0u32);
    // The page's, not the effect's: a rerun must not cancel the one going.
    let owner = Owner::current();
    effect(move || {
        let now = now.get();
        let was = before.replace(now.as_ref().map(|(s, t, _)| (s.clone(), t.clone())));
        let Some((space, tab, words)) = now else { return };
        // Another tab of the same space: not the first look, nor another space.
        if !was.is_some_and(|(s, t)| s == space && t != tab) {
            return;
        }
        let mine = turn.get().wrapping_add(1);
        turn.replace(mine);
        said.set(Some(words));
        owner.run(|| set_timeout(std::time::Duration::from_millis(1200), move || if turn.get() == mine { said.set(None) }));
    });
    when(move || said.with(Option::is_some), move || {
        view()
            .absolute()
            .top(px(12.0))
            .left(px(0.0))
            .right(px(0.0))
            .items_center()
            .pointer_events(PointerEvents::None)
            .child(
                view()
                    .px(px(14.0))
                    .h(px(32.0))
                    .justify_center()
                    .rounded(px(16.0))
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(theme::BORDER)
                    .shadow_lg()
                    .child(text(move || said.get().unwrap_or_default()).text_size(px(13.0)).text_color(theme::FOREGROUND).number_of_lines(1).max_w(px(280.0))),
            )
    })
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

/// What a tab is called on a phone: its name if it was given one, else
/// what its agent works on (a number says little among several).
fn label(t: &TabView) -> String {
    let agent = || t.panes.iter().find(|p| p.status.is_some()).map(|p| ui::title(&p.title)).filter(|s| !s.is_empty());
    if t.named { t.name.clone() } else { agent().unwrap_or_else(|| t.name.clone()) }
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

/// Toasts over the page: newest at the bottom, the last two of them (the
/// rest a tap away); a tap runs what they offer. While typing they are
/// at the top, clear of the line typed in. A waiting agent has a reminder
/// until it is shown or no longer blocked.
pub fn notes(live: Live) -> impl IntoElement {
    let (all, world) = (live.notes, live.view);
    let notes = computed(move || {
        world.with(|v| {
            let shown = v.focused.as_ref().and_then(|f| f.terminal.clone());
            let waiting = |a: &String| v.terminals.iter().any(|t| t["address"] == a.as_str() && matches!(t["status"].as_str(), Some("blocked" | "error")));
            all.with(|all| all.iter().filter(|(_, n)| waits_on(n).is_none_or(|a| waiting(&a) && shown.as_ref() != Some(&a))).cloned().collect::<Vec<_>>())
        })
    });
    let all_shown = signal(false);
    let typing = computed(|| keyboard_height().0 > 0.0);
    // While typing, the newest only: the room above the keyboard is small.
    let most = move || if typing.get() { 1 } else { SHOWN_NOTES };
    let shown = computed(move || notes.with(|n| if all_shown.get() || n.len() <= most() { n.clone() } else { n[n.len() - most()..].to_vec() }));
    let hidden = computed(move || notes.with(Vec::len) - shown.with(Vec::len));
    // At the top, under the workspace's tab strip when it has one.
    let strip = computed(move || {
        nav::routes().with(|r| r.0.last() == Some(&Route::Workspace)) && world.with(|v| shown_tabs(v).len() > 1 || !settled_tabs(v).is_empty())
    });
    dynamic(move || {
        let stack = view().absolute().left(px(12.0)).right(px(12.0)).gap(px(8.0)).pointer_events(PointerEvents::BoxNone);
        let stack = if typing.get() { stack.top(move || px(safe_area().top.0 + if strip.get() { 96.0 } else { 56.0 })) } else { stack.bottom(move || ui::bottom_inset() + px(56.0)) };
        let live = live.clone();
        stack
            .child(when(move || hidden.get() > 0 || (all_shown.get() && notes.with(Vec::len) > most()), move || {
                view()
                    .self_end()
                    .px(px(12.0))
                    .h(px(32.0))
                    .justify_center()
                    .rounded(px(16.0))
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(theme::BORDER)
                    .active(|s| s.bg(theme::HOVER))
                    .role(AccessibilityRole::Button)
                    .on_press(move || all_shown.update(|a| *a = !*a))
                    .child(text(move || if hidden.get() > 0 { t!("{} more", hidden.get()) } else { t!("Fewer").to_owned() }).text_size(px(13.0)).text_color(theme::MUTED))
            }))
            .child(each(move || shown.get(), |(id, _)| *id, move |n, _| note(live.clone(), n)))
            .into_element()
    })
}

/// How many notes show at once.
const SHOWN_NOTES: usize = 2;

fn note(live: Live, n: ReadSignal<(u64, herdc_engine::Note)>) -> impl IntoElement {
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
}
