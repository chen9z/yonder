//! The palette: every command herdr-app has, found by typing, plus the
//! spaces, tabs and agents to jump to; and the new-terminal menu.

use crate::live::{off, Live};
use crate::ui::{self, Prompt};
use crate::i18n::tr;
use crate::{nav, theme};
use herdc_engine::{shown_title, COMMANDS};
use herdc_engine::tools::agents;
use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{json, Value};

/// One thing the palette can do.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    title: String,
    detail: String,
    /// The command it runs, its arguments so far.
    command: &'static str,
    args: Value,
}

fn entries(live: &Live) -> Vec<Entry> {
    let mut out = Vec::new();
    live.view.with(|v| {
        for s in &v.spaces {
            out.push(Entry { title: s.name.clone(), detail: t!("Space").into(), command: "space.select", args: json!({ "id": s.id }) });
            for t in s.tabs.iter().filter(|t| !t.settled) {
                out.push(Entry { title: format!("{} › {}", s.name, t.name), detail: t!("Tab").into(), command: "tab.select", args: json!({ "id": t.id }) });
            }
        }
        for a in &v.agents {
            out.push(Entry {
                title: shown_title(&a.title).to_owned(),
                detail: t!("Agent · {} · {}", tr(a.status), a.space_name),
                command: "agent.focus",
                args: json!({ "terminal": a.address }),
            });
        }
    });
    for c in COMMANDS {
        out.push(Entry { title: c.name.to_owned(), detail: tr(c.about).to_owned(), command: c.name, args: json!({}) });
    }
    out
}

/// Whether every word of `query` is in the entry.
fn matches(e: &Entry, query: &str) -> bool {
    let hay = format!("{} {}", e.title, e.command).to_lowercase();
    query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

pub fn commands_page(live: Live) -> nsh_core::element::Screen {
    let query = signal(String::new());
    let all = {
        let live = live.clone();
        computed(move || entries(&live))
    };
    let shown = computed(move || query.with(|q| all.with(|a| a.iter().filter(|e| matches(e, q)).cloned().collect::<Vec<_>>())));
    ui::page(t!("Commands")).child(
        view()
            .flex_1()
            .bg(theme::CANVAS)
            .child(
                view().p(px(12.0)).child(
                    input()
                        .value(move || query.get())
                        .on_change(move |t: &str| query.set(t.to_owned()))
                        .placeholder(t!("Search commands, spaces, agents"))
                        .placeholder_color(move || theme::now(theme::DIM))
                        .auto_focus(true)
                        .auto_correct(false)
                        .auto_capitalize(AutoCapitalize::None)
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(theme::INPUT)
                        .bg(theme::BACKGROUND)
                        .px(px(12.0))
                        .h(px(44.0))
                        .text_size(px(16.0))
                        .text_color(theme::FOREGROUND),
                ),
            )
            .child(
                virtual_list(move || shown.with(Vec::len), move |at| {
                    let live = live.clone();
                    let entry = move || shown.with(|s| s.get(at.index()).cloned());
                    ui::row(move || entry().map(|e| e.title).unwrap_or_default(), move || entry().map(|e| e.detail).unwrap_or_default())
                        .on_press(move || {
                            if let Some(e) = entry() {
                                let live = live.clone();
                                spawn(async move { run(live, e).await });
                            }
                        })
                })
                .estimated_item_size(px(56.0))
                .keyboard_taps(KeyboardTaps::Handled)
                .flex_1(),
            ),
    )
}

/// Runs an entry, asking for the arguments it needs first.
async fn run(live: Live, e: Entry) {
    let mut args = e.args.clone();
    if let Some(c) = COMMANDS.iter().find(|c| c.name == e.command) {
        for arg in c.args.iter().filter(|a| a.required && e.args.get(a.name).is_none()) {
            let prompt = Prompt { title: format!("{} — {}", c.name, arg.name), placeholder: arg.name.to_owned(), value: String::new(), done: t!("Next"), masked: false };
            let Some(words) = live.words(prompt).await else { return };
            // Numbers and booleans as such, the rest as words.
            args[arg.name] = serde_json::from_str::<Value>(&words).ok().filter(|v| !v.is_string() && !v.is_object()).unwrap_or(json!(words));
        }
    }
    nav::workspace();
    live.run(e.command, args);
}

/// The new-menu: a terminal, an agent the space's machine has, a space,
/// or a worktree.
pub async fn new_terminal(live: Live) {
    let (space, place) = live.view.with_untracked(|v| (v.active_space().map(|s| s.id.clone()), v.location()));
    let on = place.as_ref().and_then(|p| p.on.clone());
    let known = match live.there(on) {
        Ok(there) => off(async move { agents::known(&there).await }).await.and_then(Result::ok).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let installed: Vec<&agents::Known> = known.iter().filter(|k| k.installed).collect();
    let mut actions: Vec<(String, String)> = vec![("terminal".into(), t!("Terminal").into())];
    actions.extend(installed.iter().map(|k| (format!("agent:{}", k.key()), k.name().to_owned())));
    actions.push(("space".into(), t!("New space").into()));
    if space.is_some() {
        actions.push(("worktree".into(), t!("New worktree…").into()));
        actions.push(("worktrees".into(), t!("Open worktree…").into()));
    }
    let shown: Vec<(&str, &str)> = actions.iter().map(|(k, n)| (k.as_str(), n.as_str())).collect();
    let Some(picked) = ui::choose(t!("New"), &shown).await else { return };
    match (picked.as_str(), picked.strip_prefix("agent:")) {
        (_, Some(key)) => {
            let Some(k) = installed.iter().find(|k| k.key() == key) else { return };
            live.run("tab.new", json!({ "name": k.name(), "space": space, "argv": k.run() }));
        }
        ("terminal", _) => live.run("tab.new", json!({ "space": space })),
        ("space", _) => live.run("space.new", json!({})),
        ("worktree", _) => {
            if let Some(place) = place {
                crate::worktrees::new_worktree(live, place).await;
            }
        }
        ("worktrees", _) => {
            if let Some(place) = place {
                nav::push(nav::Route::Worktrees(place));
            }
        }
        _ => {}
    }
}
