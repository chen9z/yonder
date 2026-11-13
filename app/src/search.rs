//! Jump to a terminal: every terminal on every machine, those that need
//! you first, found by typing its space, what it does, its program, tab,
//! folder or machine. A tap shows it in its pane, or peeks at one without.

use crate::live::Live;
use crate::nav::{self, Route};
use crate::{theme, ui};
use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{json, Value};

/// A terminal as the list says it.
#[derive(Clone, Debug, PartialEq)]
struct Found {
    address: String,
    /// Its space's id and name ("" for one in none).
    space: String,
    space_name: String,
    /// What it does, its space's name off.
    doing: String,
    /// Its tab (unless an agent's helper) and its machine.
    under: String,
    helper: bool,
    status: Option<&'static str>,
    program: Value,
    in_pane: bool,
    /// What a search looks in, lowercase.
    hay: String,
}

/// How much a status asks of the person: who needs you first.
fn rank(status: Option<&str>) -> u8 {
    match status {
        Some("blocked") => 4,
        Some("done") => 3,
        Some("working") => 2,
        _ => 1,
    }
}

/// The space a terminal is in: as labelled, else the one its `where`
/// names (on its machine), else its agent's (a helper's).
fn space_of<'a>(v: &'a herdc_engine::View, t: &Value, depth: usize) -> Option<&'a herdc_engine::SpaceView> {
    if let Some(s) = t.pointer("/labels/herdr.space").and_then(Value::as_str).and_then(|id| v.space(id)) {
        return Some(s);
    }
    let machine = t["address"].as_str().and_then(|a| a.split_once('/')).map(|(m, _)| m);
    let named = t["where"].as_str().or(t.pointer("/labels/where").and_then(Value::as_str)).and_then(|w| w.split_once(" / ")).map(|(space, _)| space);
    if let Some(s) = named.and_then(|n| v.spaces.iter().filter(|s| s.name == n).min_by_key(|s| s.machine.as_deref() != machine)) {
        return Some(s);
    }
    let parent = t["parent"].as_str().filter(|p| !p.is_empty() && depth < 8)?;
    space_of(v, v.terminals.iter().find(|o| o["address"] == parent)?, depth + 1)
}

/// Every live terminal, as the list says it, in its order.
fn found(v: &herdc_engine::View, computer: &str) -> Vec<Found> {
    let mut out: Vec<Found> = v
        .terminals
        .iter()
        .filter(|t| t["state"] != "finished")
        .filter_map(|t| {
            let address = t["address"].as_str()?.to_owned();
            let (space, space_name) = space_of(v, t, 0).map(|s| (s.id.clone(), s.name.clone())).unwrap_or_default();
            let program = t["program"]["name"].as_str().or(t["agent"].as_str()).unwrap_or_default();
            let title = ui::without(&ui::title(t["title"].as_str().unwrap_or_default()), std::slice::from_ref(&space_name));
            // A shell's `me@host:~/a` says less than its program's name.
            let telling = title.chars().count() > 2 && !title.contains('@');
            let doing = if telling { title } else { t["name"].as_str().map_or_else(|| program.to_owned(), str::to_owned) };
            let tab = t["where"].as_str().or(t.pointer("/labels/where").and_then(Value::as_str)).and_then(|w| w.split_once(" / ")).map(|(_, tab)| tab.to_owned());
            let helper = t["parent"].as_str().is_some_and(|p| !p.is_empty());
            let machine = address.split_once('/').map_or(computer, |(m, _)| m).to_owned();
            let under = [tab.filter(|_| !helper), Some(machine)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let status = match t["status"].as_str() {
                Some("working") => Some("working"),
                Some("blocked" | "error") => Some("blocked"),
                Some("done") => Some("done"),
                Some("idle") => Some("idle"),
                _ => None,
            };
            let in_pane = v.spaces.iter().flat_map(|s| &s.tabs).flat_map(|t| &t.panes).any(|p| [&p.terminal, &p.shown].iter().any(|t| t.as_deref() == Some(address.as_str())));
            let cwd = t["cwd"].as_str().unwrap_or_default();
            let hay = format!("{space_name} {doing} {program} {under} {cwd} {}", t["name"].as_str().unwrap_or_default()).to_lowercase();
            Some(Found { address, space, space_name, doing, under, helper, status, program: t["program"].clone(), in_pane, hay })
        })
        .collect();
    // Who needs you first; else as the spaces are listed.
    let at = |f: &Found| v.spaces.iter().position(|s| s.id == f.space).unwrap_or(usize::MAX);
    out.sort_by_key(|f| (std::cmp::Reverse(rank(f.status)), at(f)));
    out
}

/// Whether every word of `query` is in what `f` is.
fn matches(f: &Found, query: &str) -> bool {
    query.to_lowercase().split_whitespace().all(|w| f.hay.contains(w))
}

pub fn search_page(live: Live) -> nsh_core::element::Screen {
    let query = signal(String::new());
    let computer = live.computer.name.clone();
    let all = {
        let world = live.view;
        computed(move || world.with(|v| found(v, &computer)))
    };
    let shown = computed(move || query.with(|q| all.with(|a| a.iter().filter(|f| matches(f, q)).cloned().collect::<Vec<_>>())));
    let prefs = use_context::<crate::prefs::Prefs>().expect("prefs");
    let id = live.computer.id();
    ui::page(t!("Jump to a terminal")).child(
        view()
            .flex_1()
            .bg(theme::CANVAS)
            .child(
                view().px(px(12.0)).pt(px(12.0)).pb(px(8.0)).child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .h(px(44.0))
                        .rounded(px(12.0))
                        .bg(theme::POPOVER)
                        .child(ui::glyph("search", 16.0, theme::DIM))
                        .child(
                            input()
                                .test_id("search")
                                .flex_1()
                                .value(move || query.get())
                                .on_change(move |t: &str| query.set(t.to_owned()))
                                .placeholder(t!("A space, an agent, a folder…"))
                                .placeholder_color(move || theme::now(theme::DIM))
                                .auto_focus(true)
                                .auto_correct(false)
                                .auto_capitalize(AutoCapitalize::None)
                                .return_key_type(ReturnKeyType::Search)
                                .text_size(theme::BODY)
                                .text_color(theme::FOREGROUND),
                        ),
                ),
            )
            .child(
                scroll().flex_1().keyboard_taps(KeyboardTaps::Handled).child(
                    view()
                        .pb(move || ui::bottom_inset() + px(16.0))
                        .child(when(move || shown.with(Vec::is_empty), || {
                            text(t!("No terminal is called that")).px(px(24.0)).py(px(16.0)).text_size(theme::CALLOUT).text_color(theme::DIM)
                        }))
                        .child(
                            view().mx(theme::INSET).rounded(theme::CARD_RADIUS).overflow_hidden().bg(theme::POPOVER).child(each(
                                move || shown.get(),
                                |f| f.address.clone(),
                                move |f, at| {
                                    let live = live.clone();
                                    let id = id.clone();
                                    let color = move || f.with(|f| prefs.colors.with(|c| c.get(&format!("{id}\u{0}{}", f.space)).copied()).unwrap_or(0));
                                    view()
                                        .child(when(move || at.get() > 0, || view().h(px(1.0)).ml(px(46.0)).bg(theme::BORDER)))
                                        .child(
                                            view()
                                                .flex_row()
                                                .items_center()
                                                .gap(px(12.0))
                                                .px(px(12.0))
                                                .min_h(px(58.0))
                                                .py(px(8.0))
                                                .active(|s| s.bg(theme::HOVER))
                                                .test_id(&format!("jump-{}", f.with_untracked(|f| f.address.clone())))
                                                .role(AccessibilityRole::Button)
                                                .label(move || f.with(|f| [f.space_name.as_str(), f.doing.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ")))
                                                .on_press(move || {
                                                    let f = f.get_untracked();
                                                    if f.in_pane {
                                                        live.run("agent.focus", json!({ "terminal": f.address }));
                                                        nav::workspace();
                                                    } else {
                                                        nav::push(Route::Peek(f.address));
                                                    }
                                                })
                                                .child(ui::space_badge(move || f.with(|f| f.space_name.clone()), color, move || f.with(|f| f.program.clone())))
                                                .child(
                                                    view()
                                                        .flex_1()
                                                        .gap(px(2.0))
                                                        .child(
                                                            view()
                                                                .flex_row()
                                                                .items_center()
                                                                .gap(px(6.0))
                                                                .child(text(move || f.with(|f| f.space_name.clone())).flex_shrink(0.0).max_w(px(180.0)).text_size(theme::ROW).font_weight(FontWeight::MEDIUM).text_color(theme::FOREGROUND).number_of_lines(1))
                                                                .child(text(move || f.with(|f| f.doing.clone())).flex_1().text_size(theme::CALLOUT).text_color(theme::MUTED).number_of_lines(1)),
                                                        )
                                                        .child(text(move || f.with(|f| if f.helper { format!("{} · {}", t!("helper"), f.under) } else { f.under.clone() })).text_size(theme::CAPTION).text_color(theme::DIM).number_of_lines(1)),
                                                )
                                                .child(ui::status_mark(move || f.with(|f| f.status), || false)),
                                        )
                                },
                            )),
                        ),
                ),
            ),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn terminals_are_found_by_what_they_are_and_who_needs_you_leads() {
        let view: herdc_engine::View = serde_json::from_value(json!({
            "link": {"state": "connecting"}, "home": "", "machines": [], "agents": [], "focused": null, "spaces": [],
            "terminals": [
                {"address": "t1", "state": "live", "title": "me@pc:~/a", "agent": "zsh", "cwd": "/home/me/a", "where": "a / 1"},
                {"address": "vps/t2", "state": "live", "status": "blocked", "title": "Fix the build", "program": {"name": "Codex"}, "where": "b / 2"},
                {"address": "t3", "state": "finished", "title": "gone"},
            ]
        }))
        .unwrap();
        // Unlabelled, a terminal is in the space its `where` names, a helper in its agent's.
        let space = json!({"id": "s1", "index": 1, "name": "b", "named": true, "folder": "", "machine": "vps", "branch": null, "active": false, "status": null, "tabs": []});
        let mut with_spaces = view.clone();
        with_spaces.spaces = vec![serde_json::from_value(space).unwrap()];
        with_spaces.terminals.push(json!({"address": "vps/t4", "parent": "vps/t2", "state": "live", "title": "Check it | b"}));
        let named: Vec<_> = super::found(&with_spaces, "pc").into_iter().map(|f| (f.address, f.space_name, f.doing)).collect();
        assert!(named.contains(&("vps/t2".into(), "b".into(), "Fix the build".into())), "{named:?}");
        assert!(named.contains(&("vps/t4".into(), "b".into(), "Check it".into())), "{named:?}");
        let found = super::found(&view, "pc");
        assert_eq!(found.iter().map(|f| (f.address.as_str(), f.doing.as_str(), f.under.as_str())).collect::<Vec<_>>(), [("vps/t2", "Fix the build", "2 · vps"), ("t1", "zsh", "1 · pc")]);
        assert!(super::matches(&found[1], "home/me ZSH"));
        assert!(!super::matches(&found[1], "codex"));
    }
}
