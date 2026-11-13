//! Worktrees: a space on a new checkout of a branch, the checkouts a repo
//! has to open, and removing one with what it would lose said first, as
//! the desktop's worktree mod.

use crate::live::{off, Live};
use crate::nav;
use crate::ui::{self, Prompt};
use crate::i18n::tr_string;
use crate::theme;
use herdc_engine::tools::worktree::{self, Checkout, Repo};
use herdc_engine::tools::{Place, There};
use herdc_engine::Note;
use nsh_base::input;
use nsh_core::element::Screen;
use nsh_core::prelude::*;
use serde_json::json;

/// What the page shows once read: the repo, its checkouts, the home they
/// are shown under.
#[derive(Clone, Debug, PartialEq)]
struct Read {
    repo: Repo,
    all: Vec<Checkout>,
    home: String,
}

async fn read(there: There, folder: String) -> Result<Read, String> {
    let home = there.home().await?;
    let repo = worktree::repo_of(&there, &folder).await?.ok_or("Not in a git repository")?;
    let all = worktree::checkouts(&there, &repo).await?;
    Ok(Read { repo, all, home })
}

fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

pub fn worktrees_page(live: Live, place: Place) -> Screen {
    let found = signal(None::<Result<Read, String>>);
    let filter = signal(String::new());
    {
        let (live, place) = (live.clone(), place.clone());
        spawn(async move {
            let read = match live.there(place.on.clone()) {
                Ok(there) => off(read(there, place.folder.clone())).await,
                Err(e) => Some(Err(e)),
            };
            found.set(read);
        });
    }
    let shown = computed(move || {
        let f = filter.get().trim().to_lowercase();
        found.with(|r| match r {
            Some(Ok(r)) => r.all.iter().filter(|c| f.is_empty() || format!("{} {}", c.branch.as_deref().unwrap_or(""), c.path).to_lowercase().contains(&f)).cloned().collect(),
            _ => Vec::new(),
        })
    });
    let world = live.view;
    let title = computed(move || found.with(|r| match r {
        Some(Ok(r)) => t!("Worktrees of {}", r.repo.name),
        _ => t!("Worktrees").to_owned(),
    }));
    let new = {
        let (live, place) = (live.clone(), place.clone());
        move || {
            let (live, place) = (live.clone(), place.clone());
            spawn(async move { new_worktree(live, place).await });
        }
    };
    ui::page(move || title.get())
        .header_right(ui::icon_button("plus", t!("New worktree"), new))
        .child(
            view()
                .flex_1()
                .bg(theme::CANVAS)
                // Worth a filter only when there are many.
                .child(when(move || filter.with(|f| !f.is_empty()) || found.with(|r| r.as_ref().and_then(|r| r.as_ref().ok()).is_some_and(|r| r.all.len() > 6)), move || {
                    view().p(px(12.0)).child(
                        input()
                            .value(move || filter.get())
                            .on_change(move |t: &str| filter.set(t.to_owned()))
                            .placeholder(t!("filter worktrees"))
                            .placeholder_color(move || theme::now(theme::DIM))
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
                    )
                }))
                .child(
                    virtual_list(move || shown.with(Vec::len), move |at| {
                        let (live, place) = (live.clone(), place.clone());
                        let checkout = move || shown.with(|s| s.get(at.index()).cloned());
                        let home = move || found.with(|r| r.as_ref().and_then(|r| r.as_ref().ok()).map(|r| r.home.clone()).unwrap_or_default());
                        let on = place.on.clone();
                        let state = move || {
                            let c = checkout()?;
                            let all = shown.get();
                            Some(if c.main {
                                t!("main checkout").to_owned()
                            } else if world.with(|v| worktree::space_of(&c.path, &all, on.as_deref(), &v.spaces).is_some()) {
                                t!("open").to_owned()
                            } else if c.at > 0 {
                                worktree::ago(now() - c.at)
                            } else {
                                String::new()
                            })
                        };
                        // Its branch over its folder; how it is (open, when last used) after.
                        let open_now = {
                            let state = state.clone();
                            move || state() == Some(t!("open").to_owned())
                        };
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(14.0))
                            .px(px(16.0))
                            .min_h(px(60.0))
                            .py(px(8.0))
                            .active(|s| s.bg(theme::HOVER))
                            .child(ui::glyph("branch", 20.0, theme::MUTED))
                            .child(
                                view()
                                    .flex_1()
                                    .gap(px(2.0))
                                    .child(text(move || checkout().map(|c| c.branch.unwrap_or_else(|| t!("detached").into())).unwrap_or_default()).text_size(px(16.0)).text_color(theme::FOREGROUND).number_of_lines(1))
                                    .child(text(move || checkout().map(|c| tilde(&c.path, &home())).unwrap_or_default()).text_size(px(12.0)).text_color(theme::MUTED).number_of_lines(1)),
                            )
                            .child(text(move || state().unwrap_or_default()).text_size(px(12.0)).text_color(move || if open_now() { theme::now(theme::ACCENT) } else { theme::now(theme::DIM) }))
                        .on_press(move || {
                            let Some(c) = checkout() else { return };
                            let all = found.with_untracked(|r| r.as_ref().and_then(|r| r.as_ref().ok()).map(|r| r.all.clone()).unwrap_or_default());
                            open(&live, &place, &c, &all);
                        })
                    })
                    .estimated_item_size(px(60.0))
                    .empty(
                        view().p(px(32.0)).items_center().child(
                            text(move || match found.get() {
                                None => t!("Reading worktrees…").to_owned(),
                                Some(Err(e)) => tr_string(&e),
                                Some(Ok(_)) => t!("No worktree matches").to_owned(),
                            })
                            .text_size(px(15.0))
                            .text_color(theme::MUTED),
                        ),
                    )
                    .keyboard_taps(KeyboardTaps::Handled)
                    .pb(move || px(safe_area().bottom.0 + keyboard_height().0))
                    .flex_1(),
                ),
        )
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Shows a checkout: its space if one has it, else a new one.
fn open(live: &Live, place: &Place, c: &Checkout, all: &[Checkout]) {
    let id = live.view.with_untracked(|v| worktree::space_of(&c.path, all, place.on.as_deref(), &v.spaces).map(|s| s.id.clone()));
    match id {
        Some(id) => live.run("space.select", json!({ "id": id })),
        None => live.run("space.new", json!({ "folder": c.path, "machine": place.on.as_deref().unwrap_or("") })),
    }
    nav::workspace();
}

/// Asks for a branch and opens a space on a new checkout of it.
pub async fn new_worktree(live: Live, place: Place) {
    let there = match live.there(place.on.clone()) {
        Ok(there) => there,
        Err(e) => return live.say(Note::error("New worktree", e)),
    };
    let (looked, folder) = (there.clone(), place.folder.clone());
    let Some(found) = off(async move { Ok::<_, String>((looked.home().await?, worktree::repo_of(&looked, &folder).await?)) }).await else { return };
    let (home, repo) = match found {
        Ok((home, Some(repo))) => (home, repo),
        Ok((_, None)) => return live.say(Note::error("New worktree", "Not in a git repository")),
        Err(e) => return live.say(Note::error("New worktree", e)),
    };
    let prompt = Prompt {
        title: t!("New worktree of {}", repo.name),
        placeholder: t!("branch name").into(),
        value: String::new(),
        done: t!("Create"),
        masked: false,
    };
    let Some(branch) = live.words(prompt).await else { return };
    live.say(Note::info("Creating worktree…", tilde(&worktree::path(&home, &repo, &branch), &home)));
    let made = off(async move { worktree::create(&there, &repo, &home, &branch).await }).await;
    match made {
        // Unnamed, so the space follows the checkout's folder.
        Some(Ok(folder)) => {
            live.run("space.new", json!({ "folder": folder, "machine": place.on.as_deref().unwrap_or("") }));
            nav::workspace();
        }
        Some(Err(e)) => live.say(Note::error("New worktree", e)),
        None => {}
    }
}

/// Removes the checkout a space is on, and closes the space; what would be
/// lost is said first, and again if more is at risk by then.
pub async fn remove_worktree(live: Live, space: String, place: Place) {
    let there = match live.there(place.on.clone()) {
        Ok(there) => there,
        Err(e) => return live.say(Note::error("Remove worktree", e)),
    };
    let (looked, folder) = (there.clone(), place.folder.clone());
    let found = off(async move {
        let repo = worktree::repo_of(&looked, &folder).await?.ok_or("Not in a git repository")?;
        let risk = if repo.linked { worktree::risk(&looked, &repo.top).await? } else { Default::default() };
        Ok::<_, String>((repo, risk))
    })
    .await;
    let (repo, mut seen) = match found {
        Some(Ok(found)) => found,
        Some(Err(e)) => return live.say(Note::error("Remove worktree", e)),
        None => return,
    };
    if !repo.linked {
        return live.say(Note::error("Remove worktree", "This is the repository's main checkout, not a worktree."));
    }
    let mut said = t!("Closes the space and deletes {}. The branch stays.", repo.top);
    if seen.dirty {
        said.push_str(t!("\n\nIt has uncommitted changes: they will be lost."));
    }
    if !seen.unpushed.is_empty() {
        said.push_str(&t!("\n\nSubmodule {} has commits that aren't pushed anywhere: they will be lost.", seen.unpushed.join(", ")));
    }
    let name = live.view.with_untracked(|v| v.space(&space).map(|s| s.name.clone())).unwrap_or_default();
    if !ui::confirm(&t!("Remove worktree {}?", name), &said, t!("Remove")).await {
        return;
    }
    let (looked, top) = (there.clone(), repo.top.clone());
    match off(async move { worktree::risk(&looked, &top).await }).await {
        Some(Ok(now)) if seen.grew(now.clone()) => {
            if !ui::confirm(t!("It changed since this opened"), t!("There is more to lose now. Remove it anyway?"), t!("Remove")).await {
                return;
            }
        }
        Some(Ok(_)) => {}
        Some(Err(e)) => return live.say(Note::error("Remove worktree", e)),
        None => return,
    }
    // Terminals first: a shell sitting in the folder keeps it busy on Windows.
    live.run("space.close", json!({ "id": space, "confirmed": true }));
    match off(async move { worktree::remove(&there, &repo, &seen).await }).await {
        Some(Err(e)) => live.say(Note::error("Remove worktree", e)),
        _ => nav::workspace(),
    }
}

/// What [`kind`] found, by machine and folder.
type Kinds = std::collections::HashMap<(Option<String>, String), Option<bool>>;

thread_local! {
    /// What [`kind`] found: a space's menu is shown at once the next time.
    static KINDS: std::cell::RefCell<Kinds> = Default::default();
}

/// What [`kind`] found for the folder before, if it looked.
pub fn known_kind(place: &Place) -> Option<Option<bool>> {
    KINDS.with(|k| k.borrow().get(&(place.on.clone(), place.folder.clone())).copied())
}

/// Whether a folder's repo is the main checkout or a linked worktree;
/// None outside git (or out of reach).
pub async fn kind(live: &Live, place: &Place) -> Option<bool> {
    let there = live.there(place.on.clone()).ok()?;
    let folder = place.folder.clone();
    let read = off(async move { worktree::repo_of(&there, &folder).await }).await?;
    let linked = match read {
        Ok(repo) => repo.map(|r| r.linked),
        Err(_) => return None,
    };
    KINDS.with(|k| k.borrow_mut().insert((place.on.clone(), place.folder.clone()), linked));
    linked
}
