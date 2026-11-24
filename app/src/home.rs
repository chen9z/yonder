//! Home navigation between the main agent, the agents, and the spaces
//! grouped by machine.

use crate::live::Live;
use crate::nav::{self, Route};
use crate::pick::{self, Item};
use crate::{agents, lead, theme, ui, workspace, worktrees};
use herdc_engine::tools::Place;
use herdc_engine::SpaceView;
use nsh_core::prelude::*;
use serde_json::json;

/// A machine and its spaces, as the home page lists them.
#[derive(Clone, Debug, PartialEq)]
struct Group {
    name: String,
    key: String,
    /// What `space.new` calls it: empty for the core's own.
    machine: String,
    remote: bool,
    state: String,
    loading: bool,
    spaces: Vec<SpaceView>,
}

/// The home page's tabs: the main agent, the agents, the spaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Home {
    Lead,
    #[default]
    Agents,
    Spaces,
}

/// The tab shown: the main agent's as the app opens, when there is one
/// ([`crate::live`]), else the agents'.
pub fn shown() -> Signal<Home> {
    global::<Home>()
}

pub fn home_page(live: Live) -> impl IntoElement {
    let tab = shown();
    let world = live.view;
    // Agents that need you, else that are done, in the color of what they are.
    let waiting = computed(move || {
        world.with(|v| match v.agents.iter().filter(|a| a.status == "blocked").count() {
            0 => (v.agents.iter().filter(|a| a.status == "done").count(), "done"),
            blocked => (blocked, "blocked"),
        })
    });
    let spaces_live = live.clone();
    // Reordering the spaces: their handles show, and Done ends it.
    let reorder = signal(false);
    effect(move || if tab.get() != Home::Spaces { reorder.set(false) });
    let prefs = use_context::<crate::prefs::Prefs>().unwrap();
    let computer = live.computer.id();
    let lead_name = move || prefs.lead(&computer).map(|l| l.name()).filter(|n| !n.is_empty()).unwrap_or_else(|| t!("Main").into());
    let lead_waiting = lead::waiting(&live);
    let shown_live = live.clone();
    // The keyboard up (writing to the main agent), the tabs make room;
    // sideways, they are smaller (no words under them).
    let typing = computed(|| keyboard_height().0 > 0.0);
    let wide = computed(|| {
        let w = window();
        w.width.0 > w.height.0
    });
    // What the person looks at (read, and not told about): on the main
    // agent's tab, it; on the workspace, the shown tab's panes; on the
    // lists and the pages over the workspace, no terminal.
    {
        let (engine, found, routes) = (live.engine.clone(), lead::found(&live), nav::routes());
        let was = stored(None::<Vec<String>>);
        effect(move || {
            let looking = match routes.with(|r| r.0.last().cloned()) {
                None if tab.get() == Home::Lead => Some(found.with(|f| f.iter().map(|a| a.address.clone()).collect())),
                Some(Route::Workspace) => None,
                _ => Some(Vec::new()),
            };
            if was.replace(looking.clone()) != looking {
                engine.set_looking(looking);
            }
        });
    }
    let bar = live.clone();
    view()
        .flex_1()
        .bg(theme::CHROME)
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
        .child(top_bar(bar.clone(), tab, reorder, waiting))
        .child(workspace::link_banner(live.clone()))
        .child(dynamic(move || match tab.get() {
            Home::Lead => lead::lead_page(shown_live.clone()).into_element(),
            Home::Spaces => scroll().flex_1().child(view().pb(px(8.0)).child(spaces(spaces_live.clone(), reorder))).into_element(),
            // Scrolled, a row swiped open closes.
            Home::Agents => scroll().flex_1().on_scroll(agents::on_scrolled()).child(view().pb(px(8.0)).child(agents::agents_section(live.clone(), false))).into_element(),
        }))
        // The main agent's page is the whole screen: its way out is the
        // header's back arrow, not the tabs.
        .child(when(move || typing.get() || tab.get() == Home::Lead, || view().h(ui::bottom_inset)).otherwise(move || {
            let lead_name = lead_name.clone();
            view().items_center().pt(px(4.0)).pb(move || ui::bottom_inset() + px(if wide.get() { 2.0 } else { 6.0 })).child(
                view().flex_row().p(px(4.0)).rounded(px(32.0)).bg(theme::RAISED)
                    .border_1().border_color(theme::BORDER)
                    .child(home_tab(lead_name, "lead", tab, Home::Lead, Some(lead_waiting), wide))
                    .child(home_tab(|| t!("Agents").into(), "inbox", tab, Home::Agents, Some(waiting), wide))
                    .child(home_tab(|| t!("Spaces").into(), "spaces", tab, Home::Spaces, None, wide)),
            )
        }))
        // Back on the main agent's page (nothing over it) is Agents, as its
        // ‹ is; elsewhere back is as it was (listened to only here).
        .child(when(move || tab.get() == Home::Lead && nav::routes().with(|r| r.0.is_empty()), move || {
            on_back_press(move || {
                ui::smoothly();
                tab.set(Home::Agents);
                true
            });
            view()
        }))
}

fn home_tab(label: impl Fn() -> String + Clone + 'static, icon: &'static str, tab: Signal<Home>, mine: Home, badge: Option<Memo<(usize, &'static str)>>, wide: Memo<bool>) -> impl IntoElement {
    let active = move || tab.get() == mine;
    let ink = move || if active() { theme::ACCENT } else { theme::MUTED };
    let count = move || badge.map_or(0, |b| b.get().0);
    let color = move || badge.map_or(theme::BLOCKED, |b| theme::status(b.get().1));
    view().w(px(88.0)).h(move || px(if wide.get() { 38.0 } else { 50.0 })).rounded(px(25.0))
        .items_center().justify_center().gap(px(2.0))
        .role(AccessibilityRole::Tab)
        .label({
            let label = label.clone();
            move || if count() == 0 { label() } else { format!("{} · {}", label(), count()) }
        })
        .accessibility_state(move || AccessibilityState { selected: active(), ..Default::default() })
        .bg(move || if active() { theme::now(theme::CHROME) } else { Hsla::transparent_black() })
        .active(|s| s.scale(0.94))
        .transition()
        .duration(ui::QUICK)
        .on_press(move || if tab.get_untracked() != mine {
            ui::smoothly();
            tab.set(mine);
        })
        .child(dynamic(move || ui::glyph(icon, 22.0, ink()).into_element()))
        .child(when(move || !wide.get(), move || text(label.clone()).max_w(px(80.0)).text_size(theme::SMALL).font_weight(FontWeight::MEDIUM).text_color(move || theme::now(ink())).number_of_lines(1)))
        .child(when(move || count() > 0, move || {
            ui::badge(count, move || theme::now(color())).left(px(53.0)).top(px(1.0))
        }))
}

fn top_bar(live: Live, tab: Signal<Home>, reorder: Signal<bool>, waiting: Memo<(usize, &'static str)>) -> impl IntoElement {
    let name = live.computer.name.clone();
    let new_space = live.clone();
    let titled = live.clone();
    let talking = computed(|| crate::chat::talking().get().0);
    let readable = {
        let found = lead::found(&live);
        computed(move || found.with(|f| f.as_ref().is_some_and(crate::chat::readable)))
    };
    ui::top_bar()
        .border_b(px(0.0))
        // On the main agent's tab, the way to the others (no tabs there),
        // saying how many need you, as the workspace's does.
        .child(when(move || tab.get() == Home::Lead, move || {
            view()
                .child(ui::icon_button("chevron-left", t!("Agents"), move || {
                    ui::smoothly();
                    tab.set(Home::Agents);
                }))
                .child(when(move || waiting.get().0 > 0, move || {
                    ui::badge(move || waiting.get().0, move || theme::now(theme::status(waiting.get().1))).top(px(4.0)).right(px(-2.0))
                }))
        }))
        // The computer's name; on the main agent's tab, the agent.
        .child(dynamic(move || if tab.get() == Home::Lead {
            lead::title(titled.clone()).into_element()
        } else {
            text(name.clone()).flex_1().pl(px(12.0)).text_size(theme::TITLE).font_weight(FontWeight::BOLD).text_color(theme::FOREGROUND).number_of_lines(1).into_element()
        }))
        // On the main agent's tab: its conversation or its terminal.
        .child(when(move || tab.get() == Home::Lead && readable.get(), move || {
            dynamic(move || if talking.get() {
                ui::icon_button("terminal", t!("Show the terminal"), || { ui::smoothly(); crate::chat::talking().set(crate::chat::Talking(false)) }).into_element()
            } else {
                ui::icon_button("lead", t!("Show the conversation"), || { ui::smoothly(); crate::chat::talking().set(crate::chat::Talking(true)) }).into_element()
            })
        }))
        .child(when(move || !reorder.get(), || ui::icon_button("search", t!("Jump to a terminal"), || nav::push(Route::Search))))
        // New space where the spaces are; the rest (the agents' sort, the
        // computer's connection, machines, commands, leaving) in Settings.
        .child(when(move || tab.get() == Home::Spaces && !reorder.get(), move || new_space_button(new_space.clone())))
        .child(when(move || !reorder.get(), || ui::icon_button("settings", t!("Settings"), || nav::push(Route::Settings))))
        .child(when(move || reorder.get(), move || {
            view()
                .px(px(14.0))
                .h(px(40.0))
                .rounded(px(20.0))
                .items_center()
                .justify_center()
                .active(|s| s.bg(theme::HOVER))
                .role(AccessibilityRole::Button)
                .label(t!("Done"))
                .on_press(move || {
                    ui::smoothly();
                    reorder.set(false);
                })
                .child(text(t!("Done")).text_size(theme::ROW).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT))
        }))
}

/// A section's heading, with what it offers on the right.
pub fn section(title: &'static str, right: impl IntoElement) -> View {
    view()
        .flex_row()
        .items_center()
        .pl(px(16.0))
        .pr(px(4.0))
        .pt(px(10.0))
        .h(px(44.0))
        .child(text(title).flex_1().text_size(theme::FOOTNOTE).font_weight(FontWeight::MEDIUM).text_color(theme::DIM))
        .child(right)
}

fn new_space_button(live: Live) -> impl IntoElement {
    let world = live.view;
    ui::icon_button("plus", t!("New space"), move || {
        live.run("space.new", json!({}));
        nav::workspace();
    })
    .disabled(move || world.with(|v| !v.connected() || !v.machines.iter().any(|m| m.name.is_none() && m.layout == "ready")))
    .disabled_style(|s| s.opacity(0.4))
}

fn spaces(live: Live, reorder: Signal<bool>) -> impl IntoElement {
    let world = live.view;
    let prefs = use_context::<crate::prefs::Prefs>().unwrap();
    let (folded, colors) = (prefs.folded, prefs.colors);
    let computer = live.computer.id();
    let here = live.computer.name.clone();
    let groups = {
        let computer = computer.clone();
        computed(move || {
            world.with(|v| {
                let mut groups: Vec<Group> = v
                    .machines
                    .iter()
                    .map(|m| Group {
                        remote: m.name.is_some(),
                        key: serde_json::to_string(&(&computer, &m.name)).unwrap(),
                        machine: m.name.clone().unwrap_or_default(),
                        name: m.name.as_deref().map_or_else(|| here.clone(), |m| prefs.machine_name(&computer, m)),
                        state: m.state.clone(),
                        loading: m.layout == "loading" || m.state == "connecting",
                        spaces: v.spaces.iter().filter(|s| s.machine == m.name).cloned().collect(),
                    })
                    .collect();
                if groups.is_empty() {
                    groups.push(Group { remote: false, key: serde_json::to_string(&(&computer, None::<String>)).unwrap(), machine: String::new(), name: here.clone(), state: "connected".into(), loading: !v.connected(), spaces: v.spaces.clone() });
                }
                // Adapted from herdr-app defaults/mods/spaces/main.js, Apache-2.0.
                groups.sort_by_key(|g| (g.remote, g.name.clone()));
                groups
            })
        })
    };
    // Each space keeps the color it was given: the one least used then.
    // Adapted from herdr-app defaults/mods/spaces/main.js, Apache-2.0.
    let mine = computer.clone();
    effect(move || {
        let ids: Vec<String> = world.with(|v| v.spaces.iter().filter(|s| s.name != "~").map(|s| format!("{mine}\u{0}{}", s.id)).collect());
        if ids.iter().all(|id| colors.with_untracked(|c| c.contains_key(id))) {
            return;
        }
        colors.update(|c| {
            for id in &ids {
                if c.contains_key(id) {
                    continue;
                }
                let uses = |n: usize| ids.iter().filter(|i| c.get(*i) == Some(&n)).count();
                let least = (0..ui::MARKS.len()).min_by_key(|&n| uses(n)).unwrap_or(0);
                c.insert(id.clone(), least);
            }
        });
    });
    let several = computed(move || groups.with(|g| g.len() > 1));
    view()
        .child(each(
            move || groups.get(),
            |g| g.key.clone(),
            move |g, _| {
                let live = live.clone();
                let open = computed(move || !(several.get() && folded.with(|set| g.with(|g| set.contains(&g.key)))));
                let computer = computer.clone();
                let (empty, add) = (live.clone(), live.clone());
                // The space being dragged, and by how many rows: the others make room.
                let drag = signal(None::<(String, isize)>);
                view()
                    .child(when(move || several.get(), move || machine_heading(add.clone(), g, folded)))
                    // Its spaces on a card, as Agents' groups are.
                    .child(
                        view()
                            .mx(theme::INSET)
                            .mt(move || px(if several.get() { 0.0 } else { 10.0 }))
                            .rounded(theme::CARD_RADIUS)
                            .overflow_hidden()
                            .bg(theme::POPOVER)
                            .child(each(
                        move || if open.get() { g.with(|g| g.spaces.clone()) } else { Vec::new() },
                        |s| s.id.clone(),
                        move |s, at| {
                            let key = format!("{computer}\u{0}{}", s.with_untracked(|s| s.id.clone()));
                            space_row(live.clone(), s, g, drag, reorder, move || at.get() > 0, move || colors.with(|c| c.get(&key).copied().unwrap_or(0)))
                        },
                    ))
                    // A machine without spaces: loading them, else the way to make one.
                    .child(when(move || open.get() && g.with(|g| g.spaces.is_empty()), move || {
                        let live = empty.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(16.0))
                            .h(px(48.0))
                            .active(|s| s.bg(theme::HOVER))
                            .role(AccessibilityRole::Button)
                            .on_press(move || {
                                if several.get_untracked() && g.with_untracked(|g| !g.loading) {
                                    live.run("space.new", json!({ "machine": g.with_untracked(|g| g.machine.clone()) }));
                                    nav::workspace();
                                }
                            })
                            .child(when(move || several.get() && g.with(|g| !g.loading), || ui::glyph("plus", 13.0, theme::DIM)))
                            .child(text(move || if g.with(|g| g.loading) { t!("Loading…") } else if several.get() { t!("New space there") } else { t!("No spaces yet") }).text_size(theme::FOOTNOTE).text_color(theme::DIM))
                    })),
                    )
            },
        ))
}

/// A machine's heading: a tap folds its spaces; `+` makes one there.
// Adapted from herdr-app defaults/mods/spaces/main.js, Apache-2.0.
fn machine_heading(live: Live, g: ReadSignal<Group>, folded: Signal<Vec<String>>) -> impl IntoElement {
    let rename = live.clone();
    let prefs = use_context::<crate::prefs::Prefs>().unwrap();
    let connected = computed(move || g.with(|g| g.state == "connected"));
    let is_folded = computed(move || folded.with(|set| g.with(|g| set.contains(&g.key))));
    ui::group_heading(move || !is_folded.get(), move || g.with(|g| g.name.clone()), || theme::now(theme::DIM), move || g.with(|g| g.spaces.len()))
        .label(move || g.with(|g| t!("Fold or unfold {}", g.name)))
        .on_long_press(move || {
            let live = rename.clone();
            let machine = g.with_untracked(|g| g.machine.clone());
            if !machine.is_empty() { spawn(async move { prefs.rename_machine(&live, &machine).await; }); }
        })
        .on_press(move || {
            ui::smoothly();
            folded.update(|set| {
                let key = g.with_untracked(|g| g.key.clone());
                if set.contains(&key) { set.retain(|k| k != &key); } else { set.push(key); }
            })
        })
        .child(when(move || !connected.get(), move || {
            text(move || g.with(|g| match g.state.as_str() { "connecting" => t!("Connecting…"), "needs_login" => t!("Needs login"), "needs_update" => t!("Needs update"), _ => t!("Not connected") }))
                .text_size(theme::CAPTION)
                .text_color(theme::DIM)
        }))
        .child(when(move || connected.get(), move || {
            let live = live.clone();
            view()
                .w(px(36.0))
                .h(px(36.0))
                .rounded(px(8.0))
                .items_center()
                .justify_center()
                .active(|s| s.bg(theme::HOVER))
                .role(AccessibilityRole::Button)
                .label(move || g.with(|g| t!("New space on {}", g.name)))
                .on_press(move || {
                    live.run("space.new", json!({ "machine": g.with_untracked(|g| g.machine.clone()) }));
                    nav::workspace();
                })
                .child(ui::glyph("plus", 16.0, theme::MUTED))
        }))
}

/// A space's row: a tap opens it, a long press its menu (where reordering
/// starts: then each row has a handle to drag).
fn space_row(live: Live, s: ReadSignal<SpaceView>, g: ReadSignal<Group>, drag: Signal<Option<(String, isize)>>, reorder: Signal<bool>, below: impl Fn() -> bool + 'static, color: impl Fn() -> usize + 'static) -> impl IntoElement {
    let world = live.view;
    let (press, hold, mover) = (live.clone(), live.clone(), live);
    // How far its handle is dragged, and a row's height to count rows by.
    let dragged = signal(0.0f32);
    let tall = stored(48.0f32);
    // Another being dragged past it: a row up or down, to make its room.
    let room = move || {
        drag.with(|d| {
            let (other, rows) = d.as_ref()?;
            let me = s.with(|s| s.id.clone());
            g.with(|g| {
                let from = g.spaces.iter().position(|o| o.id == *other)? as isize;
                let at = g.spaces.iter().position(|o| o.id == me)? as isize;
                let to = (from + rows).clamp(0, g.spaces.len() as isize - 1);
                Some(if from < at && at <= to { -1.0 } else if to <= at && at < from { 1.0 } else { 0.0 })
            })
        })
        .unwrap_or(0.0)
    };
    let rows = move |by: f32| (by / tall.get().max(1.0)).round() as isize;
    view()
        .on_layout(move |b: &Bounds<Points>| { tall.replace(b.size.height.0); })
        .translate_y(move || px(if dragged.get() != 0.0 { dragged.get() } else { room() * tall.get() }))
        .z_index(move || i32::from(dragged.get() != 0.0))
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .pl(px(12.0))
        .pr(move || px(if reorder.get() { 0.0 } else { 12.0 }))
        .h(px(52.0))
        .bg(move || if dragged.get() != 0.0 { theme::now(theme::HOVER) } else if s.with(|s| s.active) { theme::now(theme::SELECTED) } else { Hsla::transparent_black() })
        .active(|st| st.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .on_press(move || {
            press.run("space.select", json!({ "id": s.get_untracked().id }));
            nav::workspace();
        })
        .on_long_press(move || {
            let (live, space) = (hold.clone(), s.get_untracked());
            let (id, place) = (space.id.clone(), Place { on: space.machine.clone(), folder: space.place() });
            spawn(async move {
                // The desktop's space menu: worktrees where the folder is in
                // git. Asking git takes a while on another machine: known
                // from before, or briefly waited for, else the menu shows at
                // once with what may be (the pages say when it isn't git).
                let known = match worktrees::known_kind(&place) {
                    Some(kind) => Some(kind),
                    None => {
                        let mut looked = std::pin::pin!(worktrees::kind(&live, &place));
                        let mut waited = std::pin::pin!(timer(std::time::Duration::from_millis(350)));
                        let found = std::future::poll_fn(|cx| match looked.as_mut().poll(cx) {
                            std::task::Poll::Ready(kind) => std::task::Poll::Ready(Some(kind)),
                            std::task::Poll::Pending => waited.as_mut().poll(cx).map(|_| None),
                        })
                        .await;
                        if found.is_none() {
                            // Found for the next time.
                            let (live, place) = (live.clone(), place.clone());
                            spawn(async move { worktrees::kind(&live, &place).await; });
                        }
                        found
                    }
                };
                let mut items = vec![Item::glyph("rename", t!("Rename"), "pencil"), Item::glyph("tab", t!("New tab here"), "terminal")];
                if g.with_untracked(|g| g.spaces.len() > 1) {
                    items.push(Item::glyph("reorder", t!("Reorder spaces"), "grip-vertical"));
                }
                let (files, changes) = (Item::glyph("files", t!("Files"), "folder"), Item::glyph("changes", t!("Changes"), "file-diff"));
                match known {
                    Some(Some(linked)) => {
                        items.extend([Item::glyph("new", t!("New worktree…"), "branch"), Item::glyph("open", t!("Open worktree…"), "layers")]);
                        if linked {
                            items.push(Item::glyph("remove", t!("Remove worktree…"), "trash-2").danger());
                        }
                        items.extend([files, changes]);
                    }
                    Some(None) => items.push(files),
                    None => items.extend([Item::glyph("open", t!("Worktrees…"), "branch"), files, changes]),
                }
                items.push(Item::glyph("close", t!("Close"), "x").danger());
                match pick::one(&space.name, items).await.as_deref() {
                    Some("rename") => live.run("space.rename", json!({ "id": id })),
                    Some("tab") => {
                        live.run("tab.new", json!({ "space": id }));
                        nav::workspace();
                    }
                    Some("reorder") => {
                        ui::smoothly();
                        reorder.set(true);
                    }
                    Some("new") => worktrees::new_worktree(live, place).await,
                    Some("open") => nav::push(Route::Worktrees(place)),
                    Some("remove") => worktrees::remove_worktree(live, id, place).await,
                    Some("files") => nav::push(Route::Files(place)),
                    Some("changes") => nav::push(Route::Changes(place)),
                    Some("close") => live.run("space.close", json!({ "id": id })),
                    _ => {}
                }
            });
        })
        .child(ui::space_mark(move || s.with(|s| s.name.clone()), color))
        .child(text(move || s.with(|s| s.name.clone())).flex_shrink(0.0).max_w(percentage(0.55)).text_size(theme::ROW).text_color(theme::FOREGROUND).number_of_lines(1))
        // The home folder by name: `~` alone reads as a stray mark.
        .child(text(move || s.with(|s| g.with(|g| world.with(|v| Some(aside(s, &g.spaces, &v.home)).filter(|a| a != "~").unwrap_or_else(|| t!("Home folder").into()))))).flex_1().text_right().text_size(theme::CAPTION).text_color(theme::DIM).number_of_lines(1))
        .child(when(below, ui::row_line))
        .child(ui::status_mark(move || s.with(|s| s.status), || true))
        .child(when(move || reorder.get(), move || {
            let live = mover.clone();
            view()
                .w(px(40.0))
                .h_full()
                .items_center()
                .justify_center()
                .child(ui::glyph("grip-vertical", 16.0, theme::DIM).opacity(0.6))
                .role(AccessibilityRole::Adjustable)
                .label(move || t!("Move {}", s.with(|s| s.name.clone())))
                .claim_touches(TouchClaim::on_start())
                .on_responder_move(move |e: &ResponderEvent| {
                    dragged.set(e.gesture.delta.y.0);
                    let moved = (s.with_untracked(|s| s.id.clone()), rows(e.gesture.delta.y.0));
                    if drag.with_untracked(|d| d.as_ref() != Some(&moved)) {
                        drag.set(Some(moved));
                    }
                })
                .on_responder_terminate(move |_: &ResponderEvent| {
                    dragged.set(0.0);
                    drag.set(None);
                })
                .on_responder_release(move |_: &ResponderEvent| {
                    let rows = rows(dragged.get_untracked());
                    dragged.set(0.0);
                    drag.set(None);
                    let id = s.with_untracked(|s| s.id.clone());
                    let to = g.with_untracked(|g| {
                        let from = g.spaces.iter().position(|o| o.id == id)?;
                        let to = (from as isize + rows).clamp(0, g.spaces.len() as isize - 1) as usize;
                        (to != from).then(|| g.spaces[to].index)
                    });
                    if let Some(index) = to {
                        live.run("space.move", json!({ "id": id, "index": index }));
                    }
                })
        }))
}

/// What tells a space apart, on its right: where it is, or else what runs
/// in it, when another of its machine has its name; else its branch.
fn aside(space: &SpaceView, siblings: &[SpaceView], home: &str) -> String {
    let twins: Vec<&SpaceView> = siblings.iter().filter(|o| o.id != space.id && o.name.eq_ignore_ascii_case(&space.name)).collect();
    if twins.is_empty() {
        return branch(space).unwrap_or_default().to_owned();
    }
    if twins.iter().all(|o| o.folder != space.folder) {
        return worktrees::tilde(&space.folder, home);
    }
    let mut runs: Vec<&str> = Vec::new();
    for p in space.tabs.iter().flat_map(|t| &t.panes) {
        let name = p.program["name"].as_str().filter(|n| !n.is_empty()).or(p.app.as_deref());
        if let Some(name) = name.filter(|n| !runs.contains(n)) {
            runs.push(name);
        }
    }
    if runs.is_empty() { branch(space).unwrap_or_default().to_owned() } else { runs.join(" · ") }
}

/// Only a branch that adds something to the space's name.
// Adapted from herdr-app defaults/mods/spaces/main.js, Apache-2.0.
fn branch(space: &SpaceView) -> Option<&str> {
    let branch = space.branch.as_deref()?;
    let repo = space.folder.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    (!branch.is_empty() && !matches!(branch, "main" | "master") && !branch.eq_ignore_ascii_case(&space.name) && !branch.eq_ignore_ascii_case(repo)).then_some(branch)
}

#[cfg(test)]
mod tests {
    use herdc_engine::SpaceView;
    use serde_json::json;

    fn space(id: &str, name: &str, folder: &str, programs: &[&str]) -> SpaceView {
        let panes: Vec<_> = programs.iter().enumerate().map(|(i, p)| json!({ "pane": format!("p{i}"), "terminal": format!("t{i}"), "mod": null, "props": null,
            "title": "", "cwd": null, "branch": null, "status": null, "app": null, "program": { "name": p }, "focused": false, "zoomed": false, "shown": null, "live": true })).collect();
        let root = herdc_model::model::Tab::new("tab", "1", "p0", "t0").root;
        serde_json::from_value(json!({ "id": id, "index": 1, "name": name, "named": true, "folder": folder, "machine": null, "branch": "main",
            "active": false, "status": null, "tabs": [{ "id": format!("{id}-tab"), "index": 1, "name": "1", "named": false, "active": true, "status": null,
                "settled": false, "settled_at": null, "changed_at": null, "working_since": null, "focused": "p0", "zoomed": null, "root": root, "panes": panes }] })).unwrap()
    }

    #[test]
    fn spaces_of_one_name_say_where_they_are_or_what_runs_there() {
        let (a, b, c) = (space("s1", "nova", "/home/me/nova", &["Pi", "Codex", "Pi"]), space("s2", "nova", "/home/me/nova", &["Codex"]), space("s3", "notes", "/home/me/notes", &["Pi"]));
        let all = [a.clone(), b.clone(), c.clone()];
        assert_eq!(super::aside(&a, &all, "/home/me"), "Pi · Codex");
        assert_eq!(super::aside(&b, &all, "/home/me"), "Codex");
        // Alone with its name: nothing (main says nothing).
        assert_eq!(super::aside(&c, &all, "/home/me"), "");
        // In another folder: the folder.
        let d = space("s4", "nova", "/srv/nova", &["Pi"]);
        assert_eq!(super::aside(&d, &[a, d.clone()], "/home/me"), "/srv/nova");
    }
}
