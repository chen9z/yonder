//! The new-menu (the header's `+`): a terminal, an agent the space's
//! machine has, a space, or a worktree.

use crate::live::{off, Live};
use crate::nav;
use crate::pick::{self, Icon, Item, Section};
use herdc_engine::tools::agents;
use nsh_core::prelude::*;
use serde_json::json;

/// The agents each machine had (None: the core's own), as last looked.
type Known = std::collections::HashMap<Option<String>, Vec<agents::Known>>;

thread_local! {
    static KNOWN: std::cell::RefCell<Known> = Default::default();
}

/// The agents machine `on` has: as last looked, at once (looked again for
/// the next time), else looked for now.
async fn known_agents(live: &Live, on: Option<String>) -> Vec<agents::Known> {
    let look = |there| async move { off(async move { agents::known(&there).await }).await.and_then(Result::ok) };
    let Ok(there) = live.there(on.clone()) else { return Vec::new() };
    if let Some(kept) = KNOWN.with(|k| k.borrow().get(&on).cloned()) {
        spawn(async move {
            if let Some(found) = look(there).await {
                KNOWN.with(|k| k.borrow_mut().insert(on, found));
            }
        });
        return kept;
    }
    let found = look(there).await.unwrap_or_default();
    KNOWN.with(|k| k.borrow_mut().insert(on, found.clone()));
    found
}

/// The new-menu: a terminal, an agent the space's machine has, a space,
/// or a worktree.
pub async fn new_terminal(live: Live) {
    let (space, place) = live.view.with_untracked(|v| (v.active_space().map(|s| s.id.clone()), v.location()));
    let on = place.as_ref().and_then(|p| p.on.clone());
    let known = known_agents(&live, on).await;
    let installed: Vec<&agents::Known> = known.iter().filter(|k| k.installed).collect();
    // What runs in the space, then other places to run it.
    let mut here = vec![Item::new("terminal", t!("Terminal"), Icon::Glyph("terminal"))];
    here.extend(installed.iter().map(|k| Item::new(format!("agent:{}", k.key()), k.name(), Icon::Program(k.program.clone()))));
    let mut elsewhere = vec![Item::new("space", t!("New space"), Icon::Glyph("layout-grid"))];
    if space.is_some() {
        elsewhere.push(Item::new("worktree", t!("New worktree…"), Icon::Glyph("branch")));
        elsewhere.push(Item::new("worktrees", t!("Open worktree…"), Icon::Glyph("folder")));
    }
    let sections = vec![Section { title: None, items: here }, Section { title: None, items: elsewhere }];
    let Some(picked) = pick::menu(t!("New"), sections).await else { return };
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
