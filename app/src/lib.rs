//! herdc: Herdr on the phone. It reaches the computer running Herdr over
//! SSH (the LAN, or Tailscale from anywhere), talks to its herdr-core as
//! the desktop app does, and shows the same spaces, tabs, panes and agents
//! in a phone's own way.

#[macro_use]
mod i18n;
mod keybar;
mod agents;
mod changes;
mod connect;
mod files;
mod home;
mod link;
mod live;
mod machines;
mod menu;
mod nav;
mod palette;
mod pick;
mod prefs;
mod settings;
mod terminal;
mod theme;
mod ui;
mod vault;
mod workspace;
mod worktrees;

use live::Live;
use nav::Route;
use nsh_base::{nav_page, nav_stack};
use nsh_core::element::Screen;
use nsh_core::prelude::*;

/// The app's UI set (`nsh.json` `"rust"`).
#[native_module("app/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
    // The programs' icons (herdr-app's) and the lists' glyphs.
    registry.assets(assets!());
}

/// The app: the computers until one is open, then its workspace with the
/// pages pushed over it. It opens again on the computer it was on.
#[component]
pub fn App() -> impl IntoElement {
    prefs::Prefs::provide();
    let current = live::current();
    if current.with_untracked(|c| c.0.is_none())
        && let Some(computer) = link::reopen()
    {
        live::open(computer, None);
    }
    // What's on screen counts as seen only while the app is in front, and
    // back in front, the line is checked. Out of sight, terminals stream
    // nothing.
    let was = stored(true);
    effect(move || {
        let now = lifecycle();
        let active = now == nsh_core::platform::Lifecycle::Active;
        let back = active && !was.replace(active);
        if let Some(live) = current.with(|c| c.0.clone()) {
            live.engine.set_active(active);
            live.engine.set_shown(now != nsh_core::platform::Lifecycle::Background);
            if now == nsh_core::platform::Lifecycle::Background {
                live.keep();
            }
            if back {
                let engine = live.engine.clone();
                spawn(async move { live::off(async move { engine.check().await }).await; });
            }
        }
    });
    view()
        .flex_1()
        .bg(theme::BACKGROUND)
        // Made again in another language (most words are said once).
        .child(dynamic(move || {
            let _ = i18n::chinese();
            match current.get().0 {
                Some(live) => connected(live).into_element(),
                None => connect::computers_page().into_element(),
            }
        }))
        // The sheet of choices, over every page (the computers' too).
        .child(pick::host())
}

fn connected(live: Live) -> impl IntoElement {
    let routes = nav::routes();
    let pages = live.clone();
    view()
        .flex_1()
        .child(
            nav_stack()
                .flex_1()
                .child(nav_page().title(live.computer.name.clone()).header_visible(false).child(home::home_page(live.clone())))
                .child(each(
                    move || routes.with(|r| r.0.clone()),
                    Route::key,
                    move |route, depth| page(pages.clone(), route.get_untracked()).on_dismiss(move || nav::truncate(depth.get_untracked())),
                )),
        )
        // Over every page: toasts, and the words a command asks for.
        .child(workspace::notes(live.clone()))
        .child(ui::prompt_dialog(live.asking, {
            let live = live.clone();
            move |words| live.answered(words)
        }))
}

fn page(live: Live, route: Route) -> Screen {
    match route {
        Route::Workspace => nav_page().title(live.computer.name.clone()).header_visible(false).child(workspace::workspace_page(live)),
        Route::EditConnection => connect::edit_current_page(live),
        Route::Settings => settings::settings_page(live),
        Route::KeyBar => keybar::keybar_page(),
        Route::Commands => palette::commands_page(live),
        Route::Files(place) => files::files_page(live, place),
        Route::File { place, path, line } => files::file_page(live, place, path, line),
        Route::Changes(place) => changes::changes_page(live, place),
        Route::Diff { place, changes, file } => changes::diff_page(live, place, changes, file),
        Route::Worktrees(place) => worktrees::worktrees_page(live, place),
        Route::Peek(address) => agents::peek_page(live, address),
        Route::AddMachine => machines::add_machine_page(live),
        Route::Text(text) => terminal::text_page(live, text),
    }
}
