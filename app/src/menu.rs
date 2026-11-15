//! The "more" sheet of the workspace: what the desktop's pane header and
//! menus offer, grouped, from the bottom of the screen where a thumb is.
//! Closing the pane sits apart, in red.

use crate::live::Live;
use crate::nav::{self, Route};
use crate::i18n::tr;
use crate::{theme, ui};
use nsh_base::sheet;
use nsh_core::prelude::*;
use serde_json::json;

/// An action of the sheet: id, glyph (`assets/chrome`), label.
type Action = (&'static str, &'static str, &'static str);

// New terminals are the header's `+`; the connection's own actions are in
// Settings (and the home menu).
const PANE: [Action; 4] = [("right", "columns-2", "Split right"), ("down", "rows-2", "Split down"), ("zoom", "maximize-2", "Zoom"), ("rename", "pencil", "Rename tab")];
const HERE: [Action; 4] = [("files", "folder", "Files"), ("changes", "file-diff", "Changes"), ("worktrees", "branch", "Worktrees"), ("settle", "archive", "Put away tab")];
const APP: [Action; 2] = [("commands", "command", "Commands"), ("settings", "settings", "Settings")];

pub fn more_sheet(live: Live, open: Signal<bool>) -> impl IntoElement {
    let zoomed = {
        let world = live.view;
        computed(move || world.with(|v| v.active_tab().is_some_and(|t| t.zoomed.is_some())))
    };
    let pick = move |id: &'static str| {
        let live = live.clone();
        move || {
            open.set(false);
            let live = live.clone();
            spawn(async move { act(live, id).await });
        }
    };
    let group = {
        let pick = pick.clone();
        move |actions: &'static [Action]| {
            let pick = pick.clone();
            view().flex_row().flex_wrap().px(px(8.0)).children(actions.iter().map(move |&(id, glyph, label)| {
                let label = move || if id == "zoom" && zoomed.get() { t!("Unzoom").to_owned() } else { tr(label).to_owned() };
                let glyph = move || if id == "zoom" && zoomed.get() { "minimize-2" } else { glyph };
                tile(glyph, label, pick(id))
            }))
        }
    };
    sheet()
        .open(move || Some(open.get()))
        .close_on_swipe(true)
        .backdrop_class("bg-black/40")
        .on_close(move || open.set(false))
        .on_cancel(move || open.set(false))
        .child(
            view().w_full().items_center().child(
                view()
                    .w_full()
                    .max_w(px(560.0))
                    .rounded_t(px(18.0))
                    .bg(theme::CHROME)
                    .pt(px(8.0))
                    .pb(move || px(safe_area().bottom.0 + 8.0))
                    .child(view().w(px(36.0)).h(px(4.0)).rounded(px(2.0)).bg(theme::BORDER).self_center().mb(px(8.0)))
                    .child(group(&PANE))
                    .child(group(&HERE))
                    .child(group(&APP))
                    .child(view().h(px(1.0)).mx(px(16.0)).my(px(6.0)).bg(theme::BORDER))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(52.0))
                            .px(px(20.0))
                            .active(|s| s.bg(theme::HOVER))
                            .role(AccessibilityRole::Button)
                            .label(t!("Close pane"))
                            .on_press(pick("close"))
                            .child(text(t!("Close pane")).text_size(theme::BODY).text_color(theme::BLOCKED)),
                    ),
            ),
        )
}

/// One action: its glyph over its name, a quarter of a row.
fn tile(glyph: impl Fn() -> &'static str + 'static, label: impl Fn() -> String + Clone + 'static, press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .w(percentage(0.25))
        .h(px(76.0))
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(12.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(label.clone())
        .on_press(press)
        .child(dynamic(move || ui::glyph(glyph(), 22.0, theme::FOREGROUND).into_element()))
        .child(text(label).text_size(theme::CAPTION).text_color(theme::MUTED).number_of_lines(1))
}

async fn act(live: Live, picked: &str) {
    match picked {
        "right" | "down" => live.run("pane.split", json!({ "direction": picked })),
        "zoom" => live.run("pane.zoom", json!({})),
        "close" => {
            let pane = live.view.with_untracked(|v| v.active_tab().and_then(|t| t.panes.iter().find(|p| p.focused).cloned()));
            if !ui::close_ok(pane.as_slice(), false).await {
                return;
            }
            live.run("pane.close", json!({}))
        }
        "rename" => live.run("tab.rename", json!({})),
        "settle" => live.run("tab.settle", json!({})),
        "files" | "changes" | "worktrees" => {
            let Some(place) = live.view.with_untracked(|v| v.location()) else { return };
            nav::push(match picked {
                "files" => Route::Files(place),
                "changes" => Route::Changes(place),
                _ => Route::Worktrees(place),
            });
        }
        "commands" => nav::push(Route::Commands),
        "settings" => nav::push(Route::Settings),
        _ => {}
    }
}
