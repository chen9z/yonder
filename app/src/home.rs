//! The home page: what the desktop's sidebar shows, the spaces by machine
//! above and the agents below. A tap on either opens the workspace on it;
//! a long press on a space renames or closes it, its handle drags it up
//! or down among its machine's.

use crate::live::{self, Live};
use crate::nav::{self, Route};
use crate::{agents, theme, ui, workspace, worktrees};
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

pub fn home_page(live: Live) -> impl IntoElement {
    view()
        .flex_1()
        .bg(theme::CHROME)
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
        .child(top_bar(live.clone()))
        .child(workspace::link_banner(live.clone()))
        .child(
            scroll().flex_1().child(
                view()
                    .pb(move || px(safe_area().bottom.0 + 16.0))
                    .child(spaces(live.clone()))
                    .child(agents::agents_section(live, false)),
            ),
        )
}

fn top_bar(live: Live) -> impl IntoElement {
    let name = live.computer.name.clone();
    ui::top_bar()
        .border_b(px(0.0))
        .child(text(name).flex_1().pl(px(12.0)).text_size(px(20.0)).font_weight(FontWeight::BOLD).text_color(theme::FOREGROUND).number_of_lines(1))
        .child(ui::icon_button("⋯", t!("Menu"), move || {
            let live = live.clone();
            spawn(async move {
                let actions = [("commands", t!("Commands")), ("machine", t!("Add a machine…")), ("settings", t!("Settings")), ("leave", t!("Leave"))];
                match ui::choose(&live.computer.name, &actions).await.as_deref() {
                    Some("commands") => nav::push(Route::Commands),
                    Some("machine") => nav::push(Route::AddMachine),
                    Some("settings") => nav::push(Route::Settings),
                    Some("leave") => live::close(),
                    _ => {}
                }
            });
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
        .child(text(title).flex_1().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::DIM))
        .child(right)
}

fn spaces(live: Live) -> impl IntoElement {
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
    // Spaces can be made once its own machine's layout is read.
    let ready = computed(move || world.with(|v| v.connected() && v.machines.iter().any(|m| m.name.is_none() && m.layout == "ready")));
    let new_space = live.clone();
    view()
        .child(section(t!("Spaces"), ui::icon_button("+", t!("New space"), move || {
            new_space.run("space.new", json!({}));
            nav::workspace();
        })
        .disabled(move || !ready.get())
        .disabled_style(|s| s.opacity(0.4))))
        .child(each(
            move || groups.get(),
            |g| g.name.clone(),
            move |g, _| {
                let live = live.clone();
                let open = computed(move || !(several.get() && folded.with(|set| g.with(|g| set.contains(&g.key)))));
                let computer = computer.clone();
                let (empty, add) = (live.clone(), live.clone());
                view()
                    .child(when(move || several.get(), move || machine_heading(add.clone(), g, folded)))
                    .child(each(
                        move || if open.get() { g.with(|g| g.spaces.clone()) } else { Vec::new() },
                        |s| s.id.clone(),
                        move |s, _| {
                            let key = format!("{computer}\u{0}{}", s.with_untracked(|s| s.id.clone()));
                            space_row(live.clone(), s, g, move || colors.with(|c| c.get(&key).copied().unwrap_or(0)))
                        },
                    ))
                    // A machine without spaces: loading them, else the way to make one.
                    .child(when(move || open.get() && g.with(|g| g.spaces.is_empty()), move || {
                        let live = empty.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .mx(px(8.0))
                            .px(px(12.0))
                            .h(px(40.0))
                            .rounded(px(10.0))
                            .active(|s| s.bg(theme::HOVER))
                            .on_press(move || {
                                if several.get_untracked() && g.with_untracked(|g| !g.loading) {
                                    live.run("space.new", json!({ "machine": g.with_untracked(|g| g.machine.clone()) }));
                                    nav::workspace();
                                }
                            })
                            .child(when(move || several.get() && g.with(|g| !g.loading), || ui::glyph("plus", 13.0, theme::DIM)))
                            .child(text(move || if g.with(|g| g.loading) { t!("Loading…") } else if several.get() { t!("New space there") } else { t!("No spaces yet") }).text_size(px(13.0)).text_color(theme::DIM))
                    }))
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
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .mx(px(8.0))
        .pl(px(10.0))
        .pr(px(4.0))
        .h(px(40.0))
        .rounded(px(8.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(move || g.with(|g| t!("Fold or unfold {}", g.name)))
        .on_long_press(move || {
            let live = rename.clone();
            let machine = g.with_untracked(|g| g.machine.clone());
            if !machine.is_empty() { spawn(async move { prefs.rename_machine(&live, &machine).await; }); }
        })
        .on_press(move || folded.update(|set| {
            let key = g.with_untracked(|g| g.key.clone());
            if set.contains(&key) { set.retain(|k| k != &key); } else { set.push(key); }
        }))
        .child(dynamic(move || ui::glyph(if g.with(|g| g.remote) { "server" } else { "monitor" }, 14.0, theme::DIM).into_element()))
        .child(text(move || g.with(|g| g.name.clone())).flex_1().text_size(px(13.0)).text_color(theme::DIM).number_of_lines(1))
        .child(when(move || !connected.get(), move || {
            text(move || g.with(|g| match g.state.as_str() { "connecting" => t!("Connecting…"), "needs_login" => t!("Needs login"), "needs_update" => t!("Needs update"), _ => t!("Not connected") }))
                .text_size(px(12.0))
                .text_color(theme::DIM)
        }))
        .child(when(move || connected.get(), move || {
            let live = live.clone();
            view()
                .w(px(32.0))
                .h(px(32.0))
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
                .child(ui::glyph("plus", 14.0, theme::DIM))
        }))
        .child(view().w(px(6.0)).h(px(6.0)).rounded(px(3.0)).bg(move || if connected.get() { theme::now(theme::ADDED) } else { theme::now(theme::DIM) }))
        .child(dynamic(move || ui::glyph(if is_folded.get() { "chevron-right" } else { "chevron-down" }, 13.0, theme::DIM).mx(px(6.0)).into_element()))
}

fn space_row(live: Live, s: ReadSignal<SpaceView>, g: ReadSignal<Group>, color: impl Fn() -> usize + 'static) -> impl IntoElement {
    let (press, hold) = (live.clone(), live.clone());
    // How far its handle is dragged, and a row's height to count rows by.
    let dragged = signal(0.0f32);
    let tall = stored(48.0f32);
    view()
        .on_layout(move |b: &Bounds<Points>| { tall.replace(b.size.height.0); })
        .translate_y(move || px(dragged.get()))
        .z_index(move || i32::from(dragged.get() != 0.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .mx(px(8.0))
        .pl(px(10.0))
        .h(px(48.0))
        .rounded(px(10.0))
        .bg(move || if dragged.get() != 0.0 { theme::now(theme::HOVER) } else if s.with(|s| s.active) { theme::now(theme::SELECTED) } else { Hsla::transparent_black() })
        .active(|st| st.bg(theme::HOVER))
        .on_press(move || {
            press.run("space.select", json!({ "id": s.get_untracked().id }));
            nav::workspace();
        })
        .on_long_press(move || {
            let (live, space) = (hold.clone(), s.get_untracked());
            let (id, place) = (space.id.clone(), Place { on: space.machine.clone(), folder: space.place() });
            spawn(async move {
                // The desktop's space menu: worktrees where the folder is in git.
                let mut actions = vec![("rename", t!("Rename")), ("tab", t!("New tab here"))];
                match worktrees::kind(&live, &place).await {
                    Some(linked) => {
                        actions.extend([("new", t!("New worktree…")), ("open", t!("Open worktree…"))]);
                        if linked {
                            actions.push(("remove", t!("Remove worktree…")));
                        }
                        actions.extend([("files", t!("Files")), ("changes", t!("Changes"))]);
                    }
                    None => actions.push(("files", t!("Files"))),
                }
                actions.push(("close", t!("Close")));
                match ui::choose(&space.name, &actions).await.as_deref() {
                    Some("rename") => live.run("space.rename", json!({ "id": id })),
                    Some("tab") => {
                        live.run("tab.new", json!({ "space": id }));
                        nav::workspace();
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
        .child(text(move || s.with(|s| s.name.clone())).flex_shrink(0.0).max_w(percentage(0.55)).text_size(px(15.0)).text_color(theme::FOREGROUND).number_of_lines(1))
        .child(text(move || s.with(|s| branch(s).unwrap_or_default().to_owned())).flex_1().text_right().text_size(px(12.0)).text_color(theme::DIM).number_of_lines(1))
        .child(
            text("≡")
                .px(px(10.0))
                .text_size(px(20.0))
                .text_color(theme::DIM)
                .role(AccessibilityRole::Adjustable)
                .label(move || t!("Move {}", s.with(|s| s.name.clone())))
                .claim_touches(TouchClaim::on_start())
                .on_responder_move(move |e: &ResponderEvent| dragged.set(e.gesture.delta.y.0))
                .on_responder_release(move |_: &ResponderEvent| {
                    let rows = (dragged.get_untracked() / tall.get().max(1.0)).round() as isize;
                    dragged.set(0.0);
                    let id = s.with_untracked(|s| s.id.clone());
                    let to = g.with_untracked(|g| {
                        let from = g.spaces.iter().position(|o| o.id == id)?;
                        let to = (from as isize + rows).clamp(0, g.spaces.len() as isize - 1) as usize;
                        (to != from).then(|| g.spaces[to].index)
                    });
                    if let Some(index) = to {
                        live.run("space.move", json!({ "id": id, "index": index }));
                    }
                }),
        )
}

/// Only a branch that adds something to the space's name.
// Adapted from herdr-app defaults/mods/spaces/main.js, Apache-2.0.
fn branch(space: &SpaceView) -> Option<&str> {
    let branch = space.branch.as_deref()?;
    let repo = space.folder.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    (!branch.is_empty() && !matches!(branch, "main" | "master") && !branch.eq_ignore_ascii_case(&space.name) && !branch.eq_ignore_ascii_case(repo)).then_some(branch)
}
