//! Notes over every page: a banner under the status bar, where typing
//! and the bars at the bottom never are. The newest shows; the others
//! wait under it, a tap away. A tap goes where it says, a swipe up or ✕
//! puts it away. A waiting agent's stays until it is shown or no longer
//! blocked.

use crate::home::{self, Home};
use crate::live::{waits_on, Live};
use crate::prefs::Prefs;
use crate::{nav, theme, ui};
use herdc_engine::Note;
use nsh_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Pulled up this far, a note goes.
const PUT_AWAY: f32 = 28.0;

/// Which agents' news comes over the page (Settings → Notes); the rest
/// shows on the Agents tab's badge and in its list only. What went wrong
/// with something the person did always shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notify {
    /// An agent waiting for the person, and the main agent done.
    #[default]
    Important,
    /// Every agent waiting or done too.
    All,
    /// None.
    Off,
}

impl Notify {
    /// Whether `note` comes over the page; `lead`: the main agent's
    /// terminal.
    pub fn shows(self, note: &Note, lead: Option<&str>) -> bool {
        match (note.alert, self) {
            (None, _) => true,
            (Some(_), Notify::Off) => false,
            (Some(_), Notify::All) | (Some("blocked"), _) => true,
            (Some(_), Notify::Important) => lead.is_some_and(|l| about(note).as_deref() == Some(l)),
        }
    }
}

pub fn notes(live: Live) -> impl IntoElement {
    let (all, world, routes) = (live.notes, live.view, nav::routes());
    let lead = crate::lead::found(&live);
    // Newest first; a waiting agent's only while it waits and isn't shown
    // (in the workspace, or the main agent on its tab).
    let prefs = use_context::<Prefs>().unwrap();
    let notes = computed(move || {
        let wanted = prefs.notify.get();
        let lead = lead.with(|l| l.as_ref().map(|l| l.address.clone()));
        let on_lead = routes.with(|r| r.0.is_empty()) && home::shown().get() == Home::Lead;
        let on_workspace = routes.with(|r| r.0.last() == Some(&nav::Route::Workspace));
        world.with(|v| {
            let shown = if on_workspace {
                v.focused.as_ref().and_then(|f| f.terminal.clone())
            } else if on_lead {
                lead.clone()
            } else {
                None
            };
            let waiting = |a: &String| v.terminals.iter().any(|t| t["address"] == a.as_str() && matches!(t["status"].as_str(), Some("blocked" | "error")));
            all.with(|all| all.iter().rev().filter(|(_, n)| wanted.shows(n, lead.as_deref()))
                .filter(|(_, n)| waits_on(n).is_none_or(|a| waiting(&a) && shown.as_ref() != Some(&a))).cloned().collect::<Vec<_>>())
        })
    });
    let opened = signal(false);
    // While typing, the newest only: the room above the keyboard is small.
    let typing = computed(|| keyboard_height().0 > 0.0);
    let open = computed(move || opened.get() && !typing.get() && notes.with(Vec::len) > 1);
    let shown = computed(move || notes.with(|n| if open.get() { n.clone() } else { n.iter().take(1).cloned().collect() }));
    let more = computed(move || notes.with(Vec::len).saturating_sub(1));
    view()
        .absolute()
        .top(move || px(safe_area().top.0 + 6.0))
        .left(move || px(safe_area().left.0 + 10.0))
        .right(move || px(safe_area().right.0 + 10.0))
        .gap(px(6.0))
        .pointer_events(PointerEvents::BoxNone)
        .child(each(move || shown.get(), |(id, _)| *id, {
            let live = live.clone();
            move |n, _| note(live.clone(), prefs, n)
        }))
        // The others, under the newest: a card's edge saying how many.
        .child(when(move || !open.get() && more.get() > 0, move || {
            view()
                .mx(px(14.0))
                .mt(px(-6.0))
                .h(px(22.0))
                .hit_slop(ui::reach(22.0))
                .rounded_b(px(12.0))
                .bg(theme::RAISED)
                .border_l_1()
                .border_r_1()
                .border_b_1()
                .border_color(theme::BORDER)
                .items_center()
                .justify_center()
                .role(AccessibilityRole::Button)
                .on_press(move || {
                    ui::smoothly();
                    opened.set(true);
                })
                .child(text(move || t!("{} more", more.get())).text_size(theme::SMALL).text_color(theme::MUTED))
        }))
        .child(when(move || open.get(), move || {
            view()
                .self_center()
                .px(px(14.0))
                .h(px(30.0))
                .hit_slop(ui::reach(30.0))
                .justify_center()
                .rounded(px(15.0))
                .bg(theme::POPOVER)
                .border_1()
                .border_color(theme::BORDER)
                .active(|s| s.bg(theme::HOVER))
                .role(AccessibilityRole::Button)
                .on_press(move || {
                    ui::smoothly();
                    opened.set(false);
                })
                .child(text(t!("Fewer")).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
        }))
}

/// The agent a note is about: the one a tap shows.
fn about(note: &Note) -> Option<String> {
    note.action.as_ref().and_then(|a| about_action(&a.name, &a.args))
}

/// Puts note `id` away.
fn put_away(live: &Live, id: u64) {
    ui::smoothly();
    live.notes.update(|all| all.retain(|(i, _)| *i != id));
}

/// Goes where a note says: the main agent on its own tab, another agent
/// in the workspace, else what it offers run.
fn go(live: &Live, prefs: Prefs, note: Note) {
    let Some(action) = note.action else { return };
    let Some(address) = about_action(&action.name, &action.args) else { return live.run(&action.name, action.args) };
    let lead = live.view.with_untracked(|v| prefs.lead(&live.computer.id()).and_then(|l| l.find(&v.agents).map(|a| a.address.clone())));
    if lead.as_deref() == Some(address.as_str()) {
        nav::truncate(0);
        ui::smoothly();
        home::shown().set(Home::Lead);
    } else {
        live.run(&action.name, action.args);
        nav::workspace();
    }
}

/// The agent an action shows, if it shows one.
fn about_action(name: &str, args: &Value) -> Option<String> {
    (name == "agent.focus").then(|| args["terminal"].as_str().map(str::to_owned)).flatten()
}

fn note(live: Live, prefs: Prefs, n: ReadSignal<(u64, Note)>) -> impl IntoElement {
    let id = n.with_untracked(|n| n.0);
    let alert = n.with_untracked(|n| n.1.alert);
    let error = n.with_untracked(|n| n.1.level == "error");
    let program = {
        let (world, address) = (live.view, n.with_untracked(|n| about(&n.1)));
        computed(move || address.as_ref().and_then(|a| world.with(|v| v.agents.iter().find(|x| &x.address == a).map(|x| x.program.clone()))))
    };
    // Pulled up by a finger: it follows, and goes past `PUT_AWAY`.
    let pulled = signal(0.0f32);
    let (tapped, closed, released) = (live.clone(), live.clone(), live.clone());
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .pl(px(14.0))
        .pr(px(4.0))
        .min_h(px(56.0))
        .py(px(8.0))
        .rounded(px(14.0))
        .bg(theme::POPOVER)
        .border_1()
        .border_color(move || if alert == Some("blocked") || error { theme::now(theme::BLOCKED) } else { theme::now(theme::BORDER) })
        .shadow_lg()
        .translate_y(move || px(pulled.get()))
        .opacity(move || 1.0 - (-pulled.get() / (PUT_AWAY * 3.0)).clamp(0.0, 0.6))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(move || n.with(|n| [n.1.title.clone(), n.1.body.clone().unwrap_or_default()].join(", ")))
        .claim_touches(TouchClaim::on_pan(PanActivation::vertical(px(8.0), px(16.0))))
        .on_responder_move(move |e: &ResponderEvent| {
            let dy = e.gesture.delta.y.0;
            // Up as far as it goes; down only a little, to say it won't.
            pulled.set(if dy < 0.0 { dy } else { dy.min(24.0) * 0.3 });
        })
        .on_responder_terminate(move |_: &ResponderEvent| pulled.set(0.0))
        .on_responder_release(move |_: &ResponderEvent| {
            if pulled.get_untracked() <= -PUT_AWAY {
                put_away(&released, id);
            } else {
                pulled.set(0.0);
            }
        })
        .on_press(move || {
            let note = n.with_untracked(|n| n.1.clone());
            put_away(&tapped, id);
            go(&tapped, prefs, note);
        })
        // What it is about: the agent's program, a dot in its status on it.
        .child(
            view()
                .w(px(28.0))
                .h(px(28.0))
                .items_center()
                .justify_center()
                .child(dynamic(move || match program.get() {
                    Some(p) if p["icon"].is_string() => ui::program_icon(move || p.clone(), 24.0).into_element(),
                    _ => ui::glyph(if alert.is_some() { "terminal" } else { "info" }, 22.0, if error { theme::BLOCKED } else { theme::MUTED }).into_element(),
                }))
                .children(alert.map(|word| {
                    view()
                        .absolute()
                        .right(px(-3.0))
                        .bottom(px(-3.0))
                        .w(px(11.0))
                        .h(px(11.0))
                        .rounded(px(5.5))
                        .border_2()
                        .border_color(theme::POPOVER)
                        .bg(theme::status(word))
                })),
        )
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(text(move || n.with(|n| n.1.title.clone())).flex_shrink(1.0).text_size(theme::CALLOUT).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND).number_of_lines(1))
                        .children((alert == Some("blocked")).then(|| ui::status_mark(|| Some("blocked"), || false))),
                )
                .child(when(move || n.with(|n| n.1.body.as_deref().is_some_and(|b| !b.is_empty())), move || {
                    text(move || n.with(|n| n.1.body.clone().unwrap_or_default()))
                        .text_size(theme::FOOTNOTE)
                        .text_color(theme::MUTED)
                        .number_of_lines(if alert.is_some() { 1 } else { 3 })
                })),
        )
        .child(ui::icon_button("x", t!("Not now"), move || put_away(&closed, id)).w(px(40.0)).h(px(40.0)))
}

#[cfg(test)]
mod tests {
    use super::Notify;
    use herdc_engine::{Action, Note};

    fn of(alert: Option<&'static str>, terminal: &str) -> Note {
        let action = Action { name: "agent.focus".into(), args: serde_json::json!({ "terminal": terminal }) };
        Note { title: "x".into(), body: None, level: "info", action: Some(action), alert, toast: true }
    }

    #[test]
    fn only_what_matters_comes_over_the_page() {
        let lead = Some("t1");
        // Waiting, anyone; done, only the main agent.
        assert!(Notify::Important.shows(&of(Some("blocked"), "t9"), lead));
        assert!(Notify::Important.shows(&of(Some("done"), "t1"), lead));
        assert!(!Notify::Important.shows(&of(Some("done"), "t9"), lead));
        assert!(!Notify::Important.shows(&of(Some("done"), "t9"), None));
        assert!(Notify::All.shows(&of(Some("done"), "t9"), lead));
        // Off: no agent's; what went wrong still.
        assert!(!Notify::Off.shows(&of(Some("blocked"), "t1"), lead));
        assert!(Notify::Off.shows(&Note::error("Split failed", "no room"), lead));
    }
}
