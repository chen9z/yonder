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
    // The space shown closed (its last pane, from a menu): home, not on to
    // whichever space the core shows next, maybe on another machine.
    let was = stored(None::<String>);
    effect(move || {
        let (now, gone) = world.with(|v| {
            let now = v.active_space().map(|s| s.id.clone());
            let gone = v.connected() && was.with(|w| w.as_ref().is_some_and(|w| Some(w) != now.as_ref() && !v.spaces.iter().any(|s| &s.id == w)));
            (now, gone)
        });
        was.replace(now);
        if gone && nav::routes().with_untracked(|r| r.0.last() == Some(&Route::Workspace)) {
            nav::home();
        }
    });
    view()
        .flex_1()
        .bg(theme::BACKGROUND)
        // Turned sideways, clear of the system's bar and the camera.
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
        .child(header(live.clone(), space, tab, more_open))
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
    // The tabs, as dots under the name, while there are several.
    let many = computed(move || world.with(|v| has_strip(v)));
    let computer = live.computer.id();
    let color = move || space.with(|s| s.as_ref().and_then(|s| prefs.colors.with(|c| c.get(&format!("{computer}\u{0}{}", s.0)).copied())).unwrap_or(0));
    // A machine's space says which, before the computer it's reached through.
    let machine = {
        let id = live.computer.id();
        let computer = live.computer.name.clone();
        computed(move || world.with(|v| v.active_space().and_then(|s| s.machine.clone())).map_or(computer.clone(), |m| format!("{} · {computer}", prefs.machine_name(&id, &m))))
    };
    ui::top_bar()
        .child(
            view()
                .child(ui::icon_button("chevron-left", t!("Spaces and agents"), nav::home).pr(px(4.0)))
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
                        .child(text(move || waiting.get().0.to_string()).text_size(theme::SMALL).text_color(rgb(0xffffff)))
                })),
        )
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    // The space and where it is: a tap goes to another, a hold
                    // offers the shown tab's menu.
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .min_h(px(24.0))
                        .hit_slop(EdgeInsets::all(px(8.0)))
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
                        .child(when(move || space.with(Option::is_some), move || {
                            ui::space_mark(move || space.with(|s| s.as_ref().map(|s| s.1.clone()).unwrap_or_default()), color.clone())
                        }))
                        .child(
                            text(move || space.with(|s| s.as_ref().map_or_else(|| "Yonder".to_owned(), |s| s.1.clone())))
                                .flex_shrink(1.0)
                                .text_size(theme::BODY)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::FOREGROUND)
                                .number_of_lines(1),
                        )
                        // The status after where it is: a spare gap when there is none.
                        .child(text(move || format!("· {}", machine.get())).flex_shrink(1.0).text_size(theme::FOOTNOTE).text_color(theme::MUTED).number_of_lines(1))
                        .child(ui::tab_status(move || space.with(|s| s.as_ref().and_then(|s| s.2)))),
                )
                .child(when(move || many.get(), {
                    let live = live.clone();
                    move || tab_dots(live.clone())
                })),
        )
        // The new-menu: a terminal, an agent, a space or a worktree.
        .child(ui::icon_button("plus", t!("New"), {
            let live = live.clone();
            move || {
                spawn(crate::palette::new_terminal(live.clone()));
            }
        }))
        .child(ui::icon_button("ellipsis", t!("More"), move || {
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
        use crate::pick::{self, Item};
        let items = vec![Item::glyph("rename", t!("Rename"), "pencil"), Item::glyph("settle", t!("Put away"), "archive"), Item::glyph("close", t!("Close"), "x").danger()];
        let picked = pick::one(t!("Tab"), items).await;
        let command = match picked.as_deref() {
            Some("rename") => "tab.rename",
            Some("settle") => "tab.settle",
            Some("close") => {
                let panes = live.view.with_untracked(|v| v.tab(&id).map(|t| t.panes.clone()).unwrap_or_default());
                if !ui::close_ok(&panes, true).await {
                    return;
                }
                "tab.close"
            }
            _ => return,
        };
        live.run(command, serde_json::json!({ "id": id }));
    });
}

/// The spaces to go to, from the title, as home lists them: by machine,
/// this computer's first, each in its color with its status, the shown one
/// checked; or all of them, at home.
fn switch_space(live: Live, prefs: crate::prefs::Prefs) {
    use crate::pick::{Icon, Item, Section};
    let computer = live.computer.id();
    let mut sections: Vec<(bool, String, Section)> = Vec::new();
    live.view.with_untracked(|v| {
        for s in &v.spaces {
            let machine = s.machine.as_deref().map_or_else(|| live.computer.name.clone(), |m| prefs.machine_name(&computer, m));
            let color = prefs.colors.with_untracked(|c| c.get(&format!("{computer}\u{0}{}", s.id)).copied().unwrap_or(0));
            let item = Item { status: s.status, checked: s.active, ..Item::new(s.id.clone(), s.name.clone(), Icon::Space(s.name.clone(), color)) };
            match sections.iter_mut().find(|(_, m, _)| *m == machine) {
                Some((_, _, section)) => section.items.push(item),
                None => sections.push((s.machine.is_some(), machine.clone(), Section { title: Some(machine), items: vec![item] })),
            }
        }
    });
    sections.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let mut sections: Vec<Section> = sections.into_iter().map(|(_, _, s)| s).collect();
    if let [only] = sections.as_mut_slice() {
        only.title = None;
    }
    sections.push(Section { title: None, items: vec![Item::new("\0home", t!("All spaces and agents"), Icon::Glyph("layout-grid"))] });
    spawn(async move {
        match crate::pick::menu(t!("Spaces"), sections).await.as_deref() {
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

/// The tabs are shown (as dots): more than one, or some put away.
fn has_strip(v: &herdc_engine::View) -> bool {
    shown_tabs(v).len() > 1 || !settled_tabs(v).is_empty()
}

/// The space's tabs put away (the shown one aside): id and label.
fn settled_tabs(v: &herdc_engine::View) -> Vec<(String, String)> {
    let names = space_names(v);
    v.active_space().map(|s| s.tabs.iter().filter(|t| t.settled && !t.active).map(|t| (t.id.clone(), label(t, &names))).collect()).unwrap_or_default()
}

/// The space's tabs as dots under its name, while it has more than one
/// (a swipe across goes between them): the shown one a bar, one that
/// works, needs you or is done in its color. A tap shows one, a hold
/// offers its menu; tabs put away are counted after, a tap bringing one
/// back.
fn tab_dots(live: Live) -> impl IntoElement {
    let world = live.view;
    let tabs = computed(move || {
        world.with(|v| {
            let names = space_names(v);
            shown_tabs(v).into_iter().map(|t| TabChip { id: t.id.clone(), label: label(t, &names), status: t.status, active: t.active }).collect::<Vec<_>>()
        })
    });
    let settled = computed(move || world.with(|v| settled_tabs(v)));
    let back = live.clone();
    view()
        .flex_row()
        .items_center()
        // The first dot under the name's first letter (past the space's mark).
        .pl(px(24.0))
        .child(each(
            move || tabs.get(),
            |t| t.id.clone(),
            move |t, _| {
                let (press, hold) = (live.clone(), live.clone());
                view()
                    .w(px(18.0))
                    .h(px(14.0))
                    .items_center()
                    .justify_center()
                    .hit_slop(EdgeInsets::all(px(6.0)))
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
                    .child(
                        view()
                            .h(px(6.0))
                            .w(move || px(if t.with(|t| t.active) { 14.0 } else { 6.0 }))
                            .rounded(px(3.0))
                            .bg(move || {
                                t.with(|t| match t.status {
                                    _ if t.active => theme::now(theme::FOREGROUND),
                                    Some(word @ ("working" | "blocked" | "done")) => theme::now(theme::status(word)),
                                    _ => theme::now(theme::DIM),
                                })
                            }),
                    )
            },
        ))
        .child(when(move || settled.with(|s| !s.is_empty()), move || {
            let back = back.clone();
            view()
                .flex_row()
                .items_center()
                .gap(px(3.0))
                .ml(px(6.0))
                .h(px(14.0))
                .hit_slop(EdgeInsets::all(px(8.0)))
                .role(AccessibilityRole::Button)
                .label(move || t!("{} put away", settled.with(Vec::len)))
                .on_press(move || {
                    let (live, tabs) = (back.clone(), settled.get_untracked());
                    spawn(async move {
                        let items = tabs.iter().map(|(id, label)| crate::pick::Item::glyph(id.clone(), label.clone(), "archive")).collect();
                        // Shown, it comes back to the dots.
                        if let Some(id) = crate::pick::one(t!("Put away"), items).await {
                            live.run("tab.select", serde_json::json!({ "id": id }));
                        }
                    });
                })
                .child(ui::glyph("archive", 11.0, theme::DIM))
                .child(text(move || settled.with(Vec::len).to_string()).text_size(theme::SMALL).text_color(theme::DIM))
        }))
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
            Some((space, tabs[at].id.clone(), format!("{}/{} · {}", at + 1, tabs.len(), label(tabs[at], &space_names(v)))))
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
                    .child(text(move || said.get().unwrap_or_default()).text_size(theme::FOOTNOTE).text_color(theme::FOREGROUND).number_of_lines(1).max_w(px(280.0))),
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
            .child(text(move || said.get().unwrap_or_default()).flex_1().text_size(theme::FOOTNOTE).text_color(theme::MUTED).number_of_lines(2))
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
/// what its agent works on (a number says little among several), without
/// the space's name the bar says (`π - nova`, `Fix it | nova`); with
/// little more than that, the agent's name.
fn label(t: &TabView, space: &[String]) -> String {
    if t.named {
        return t.name.clone();
    }
    let Some(agent) = t.panes.iter().find(|p| p.status.is_some()) else { return t.name.clone() };
    let title = ui::without(&ui::title(&agent.title), space);
    if title.chars().count() > 2 {
        return title;
    }
    let name = agent.program["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned);
    name.or_else(|| (!title.is_empty()).then_some(title)).unwrap_or_else(|| t.name.clone())
}

/// What the shown space is called, as an agent's title may say it: its
/// name, and its folder's.
fn space_names(v: &herdc_engine::View) -> Vec<String> {
    let Some(s) = v.active_space() else { return Vec::new() };
    let folder = s.folder.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_owned();
    vec![s.name.clone(), folder]
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
                .map(|t| {
                    let titles = chip_titles(t.panes.iter().map(|p| p.title.as_str()));
                    t.panes.iter().zip(titles).map(|(p, title)| Chip { pane: p.pane.clone(), title, focused: p.focused, status: p.status, program: p.program.clone() }).collect::<Vec<_>>()
                })
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

/// The strip's names of panes titled `raw`: a shell's `me@host:~/a/b` as
/// its folder (`b`), and a name more than one has numbered by where it is.
fn chip_titles<'a>(raw: impl Iterator<Item = &'a str>) -> Vec<String> {
    let short: Vec<String> = raw
        .map(|raw| {
            let title = ui::title(raw);
            match title.split_once(':') {
                Some((who, path)) if who.contains('@') && !who.contains(' ') && !path.is_empty() => {
                    path.trim_end_matches('/').rsplit('/').next().filter(|f| !f.is_empty()).unwrap_or(path).to_owned()
                }
                _ => title,
            }
        })
        .collect();
    short.iter().enumerate().map(|(at, t)| if short.iter().filter(|o| *o == t).count() > 1 { format!("{t} {}", at + 1) } else { t.clone() }).collect()
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
        // As tall as the tabs' chips, for a thumb; outlined, not filled, to
        // tell the two strips apart.
        view().flex_row().items_center().px(px(6.0)).py(px(5.0)).gap(px(6.0)).child(each(
            move || panes.get(),
            |p| p.pane.clone(),
            move |p, _| {
                let live = live.clone();
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(34.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(move || if p.with(|p| p.focused) { theme::now(theme::ACCENT) } else { theme::now(theme::BORDER) })
                    .active(|s| s.bg(theme::HOVER))
                    .role(AccessibilityRole::Button)
                    .label(move || p.with(|p| p.title.clone()))
                    .accessibility_state(move || AccessibilityState { selected: p.with(|p| p.focused), ..Default::default() })
                    .on_press(move || live.run("pane.focus", serde_json::json!({ "pane": p.get_untracked().pane })))
                    .child(ui::program_icon(move || p.with(|p| p.program.clone()), 14.0))
                    .child(text(move || p.with(|p| p.title.clone())).text_size(theme::FOOTNOTE).text_color(theme::FOREGROUND).number_of_lines(1).max_w(px(120.0)))
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
        .child(text(t!("The {} pane", name)).text_size(theme::BODY).text_color(theme::FOREGROUND))
        .child(text(t!("It shows on the desktop.")).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
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
                .text_size(theme::HEADLINE)
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
    dynamic(move || {
        let stack = view().absolute().left(px(12.0)).right(px(12.0)).gap(px(8.0)).pointer_events(PointerEvents::BoxNone);
        let stack = if typing.get() { stack.top(move || px(safe_area().top.0 + 56.0)) } else { stack.bottom(move || ui::bottom_inset() + px(56.0)) };
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
                    .child(text(move || if hidden.get() > 0 { t!("{} more", hidden.get()) } else { t!("Fewer").to_owned() }).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
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
                        .child(text(move || n.with(|n| n.1.title.clone())).text_size(theme::CALLOUT).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND))
                        .child(text(move || n.with(|n| n.1.body.clone().unwrap_or_default())).text_size(theme::FOOTNOTE).text_color(theme::MUTED).number_of_lines(3)),
                )
                .child({
                    let notes = live.notes;
                    ui::icon_button("x", t!("Not now"), move || {
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

#[cfg(test)]
mod tests {
    #[test]
    fn panes_on_the_strip_are_told_apart() {
        let named = |raw: &[&str]| super::chip_titles(raw.iter().copied());
        assert_eq!(named(&["me@pc:~/workspace/nova", "me@pc:~/workspace/nova"]), ["nova 1", "nova 2"]);
        assert_eq!(named(&["me@pc:~", "me@pc:/", "Fix the build"]), ["~", "/", "Fix the build"]);
        assert_eq!(named(&["see: this", "me@pc:~/a/b/"]), ["see: this", "b"]);
    }
}
