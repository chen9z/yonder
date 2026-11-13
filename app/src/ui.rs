//! The app's small parts: bars, buttons, rows, dots and the dialogs that
//! ask for a word or a yes.

use crate::theme;
use nsh_base::{alert_dialog, dialog};
use nsh_base::input;
use nsh_core::prelude::*;

/// A row of the app's own top bar, under the status bar.
pub fn top_bar() -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .pt(move || px(safe_area().top.0))
        .px(px(4.0))
        .min_h(move || px(48.0 + safe_area().top.0))
        .bg(theme::CHROME)
        .border_b_1()
        .border_color(theme::BORDER)
}

/// A square press target with a glyph.
pub fn icon_button(glyph: &'static str, label: &'static str, press: impl Fn() + 'static) -> View {
    view()
        .w(px(44.0))
        .h(px(44.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(press)
        .child(text(glyph).text_size(px(20.0)).text_color(theme::FOREGROUND))
}

/// The main action of a page.
pub fn button(label: impl IntoReactive<String> + 'static, press: impl Fn() + 'static) -> View {
    let label = label.into_reactive();
    view()
        .h(px(48.0))
        .px(px(20.0))
        .rounded(px(12.0))
        .items_center()
        .justify_center()
        .bg(theme::SOLID)
        .active(|s| s.opacity(0.7))
        .role(AccessibilityRole::Button)
        .on_press(press)
        .child(text(move || label.get()).text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::SOLID_FOREGROUND))
}

/// A quieter action.
pub fn ghost_button(label: &'static str, press: impl Fn() + 'static) -> View {
    view()
        .h(px(44.0))
        .px(px(16.0))
        .rounded(px(12.0))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(theme::BORDER)
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(press)
        .child(text(label).text_size(px(15.0)).text_color(theme::FOREGROUND))
}

/// A file the app ships, by its path under `assets/`.
pub fn asset(path: &str) -> String {
    format!("asset:/@{}/{path}", env!("CARGO_PKG_NAME"))
}

/// One of the lists' glyphs (herdr-app's), in `color`.
pub fn glyph(name: &str, size: f32, color: AdaptiveColor) -> nsh_core::element::Image {
    image(asset(&format!("chrome/{name}.svg"))).w(px(size)).h(px(size)).flex_shrink(0.0).tint_color(color)
}

/// What runs, as its icon from herdr-app's program table: a one-color
/// glyph takes the text's color, a picture keeps its own. Nothing for a
/// program without one.
pub fn program_icon(program: impl Fn() -> serde_json::Value + 'static, size: f32) -> impl IntoElement {
    let icon = computed(move || {
        let p = program();
        p["icon"].as_str().map(|i| (i.to_owned(), p["tint"].as_bool().unwrap_or(true)))
    });
    dynamic(move || match icon.get() {
        Some((icon, tint)) => {
            let picture = image(asset(&icon)).w(px(size)).h(px(size)).flex_shrink(0.0);
            if tint { picture.tint_color(theme::FOREGROUND).into_element() } else { picture.into_element() }
        }
        None => view().into_element(),
    })
}

/// A status's color, faint: what a badge sits on.
fn faint(color: AdaptiveColor) -> impl Fn() -> Hsla {
    move || Hsla { a: 0.15, ..theme::now(color) }
}

/// An agent's status as the desktop's lists draw it: working turns, done
/// is a dot, idle a ring, and blocked says "Needs you", or `!` where a
/// heading already says it (`compact`).
// Adapted from herdr-app defaults/mods/agents/main.js, Apache-2.0.
pub fn status_mark(status: impl Fn() -> Option<&'static str> + 'static, compact: impl Fn() -> bool + 'static) -> impl IntoElement {
    let shown = computed(move || (status(), compact()));
    dynamic(move || match shown.get() {
        (Some("working"), _) => glyph("spinner", 12.0, theme::WORKING).mx(px(1.0)).animate_spin().into_element(),
        (Some("blocked"), true) => view()
            .w(px(14.0))
            .h(px(14.0))
            .rounded(px(4.0))
            .bg(faint(theme::BLOCKED))
            .items_center()
            .justify_center()
            .child(text("!").text_size(px(10.5)).font_weight(FontWeight::BOLD).text_color(theme::BLOCKED))
            .into_element(),
        (Some("blocked"), false) => view()
            .px(px(6.0))
            .h(px(18.0))
            .rounded(px(5.0))
            .bg(faint(theme::BLOCKED))
            .justify_center()
            .child(text(t!("Needs you")).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::BLOCKED))
            .into_element(),
        (Some("idle"), _) => view().w(px(7.0)).h(px(7.0)).mx(px(3.0)).rounded(px(3.5)).border(px(1.5)).border_color(theme::IDLE).into_element(),
        (Some(word), _) => view().w(px(7.0)).h(px(7.0)).mx(px(3.0)).rounded(px(3.5)).bg(theme::status(word)).into_element(),
        (None, _) => view().w(px(7.0)).h(px(7.0)).mx(px(3.0)).into_element(),
    })
}

/// A tab's status as the desktop's tabs say it: turning while it works,
/// a dot when it needs you or is done, else nothing.
pub fn tab_status(status: impl Fn() -> Option<&'static str> + 'static) -> impl IntoElement {
    let shown = computed(status);
    dynamic(move || match shown.get() {
        Some("working") => glyph("spinner", 12.0, theme::WORKING).animate_spin().into_element(),
        Some(word @ ("blocked" | "done")) => view().w(px(6.0)).h(px(6.0)).rounded(px(3.0)).bg(theme::status(word)).into_element(),
        _ => view().into_element(),
    })
}

/// The spaces' colors, as the desktop gives them out.
// Adapted from herdr-app defaults/mods/spaces/main.js, Apache-2.0.
pub const MARKS: [u32; 6] = [0x22a99a, 0xe3814f, 0x5b8def, 0x8b7cf6, 0x3fae6a, 0xd9649a];

/// A space's mark: its first letter on its color.
pub fn space_mark(name: impl Fn() -> String + 'static, color: impl Fn() -> usize + 'static) -> View {
    view()
        .w(px(22.0))
        .h(px(22.0))
        .rounded(px(6.0))
        .flex_shrink(0.0)
        .bg(move || rgb(MARKS[color() % MARKS.len()]))
        .items_center()
        .justify_center()
        .child(
            text(move || name().chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_else(|| "?".into()))
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xffffff)),
        )
}

/// An agent's status as a dot (nothing when it has none).
pub fn dot(status: impl Fn() -> Option<&'static str> + 'static) -> View {
    let status = std::rc::Rc::new(status);
    let shown = status.clone();
    view()
        .w(px(8.0))
        .h(px(8.0))
        .rounded(px(4.0))
        .bg(move || match shown() {
            Some(word) => theme::now(theme::status(word)),
            None => Hsla::transparent_black(),
        })
}

/// A labeled field of a form; `keyboard` says what it takes (a host or a
/// user name: `Url`, which keeps a pinyin or kana keyboard from composing).
pub fn field(label: &'static str, value: Signal<String>, keyboard: KeyboardType) -> View {
    let focused = signal(false);
    view()
        .gap(px(6.0))
        .child(text(label).text_size(px(13.0)).text_color(theme::MUTED))
        .child(
            input()
                .test_id(&label.to_lowercase())
                .keyboard_type(keyboard)
                .value(move || value.get())
                .on_change(move |t: &str| value.set(t.to_owned()))
                .on_focus(move || focused.set(true))
                .on_blur(move || focused.set(false))
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .rounded(px(10.0))
                .border_1()
                .border_color(move || if focused.get() { theme::now(theme::ACCENT) } else { theme::now(theme::INPUT) })
                .bg(theme::BACKGROUND)
                .px(px(12.0))
                .h(px(46.0))
                .text_size(px(16.0))
                .text_color(theme::FOREGROUND)
                .placeholder_color(move || theme::now(theme::DIM)),
        )
}

/// A list row: a title, what it is under it, and what it leads to.
pub fn row(title: impl Fn() -> String + 'static, detail: impl Fn() -> String + 'static) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .px(px(16.0))
        .min_h(px(56.0))
        .py(px(8.0))
        .active(|s| s.bg(theme::HOVER))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(text(title).text_size(px(16.0)).text_color(theme::FOREGROUND).number_of_lines(1))
                .child(text(detail).text_size(px(13.0)).text_color(theme::MUTED).number_of_lines(1)),
        )
}

/// A section's heading in a list.
pub fn heading(title: impl Fn() -> String + 'static) -> View {
    view()
        .px(px(16.0))
        .pt(px(18.0))
        .pb(px(6.0))
        .child(text(title).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::DIM))
}

/// A page pushed over the workspace, its header in the app's colors (the
/// system's own is light whatever the app is).
pub fn page(title: impl IntoReactive<SharedString>) -> nsh_core::element::Screen {
    nsh_base::nav_page()
        .title(title)
        .header_style_with(|s| s.bg(theme::CHROME).text_color(theme::FOREGROUND).tint_color(theme::FOREGROUND))
        .bg(theme::CANVAS)
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
}

/// What covers the bottom of the screen: the keyboard (which counts the
/// system's bar under it) or, without one, that bar.
pub fn bottom_inset() -> Points {
    keyboard_height().max(safe_area().bottom)
}

/// A terminal's title as a narrow screen shows it: no spinner, no
/// invisible characters, and no leading marker like Codex's
/// `[ ! ] Action Required | ` (its status says as much).
pub fn title(raw: &str) -> String {
    let shown: String = herdc_engine::shown_title(raw)
        .chars()
        .filter(|c| !matches!(c, '\u{200b}'..='\u{200f}' | '\u{2060}'..='\u{2064}' | '\u{feff}'))
        .collect();
    let shown = shown.trim();
    match shown.split_once(" | ") {
        Some((marker, rest)) if marker.starts_with('[') && !rest.trim().is_empty() => rest.trim().to_owned(),
        _ => shown.to_owned(),
    }
}

/// Asks yes or no; true for yes.
pub async fn confirm(title: &str, message: &str, yes: &str) -> bool {
    let answer = alert_dialog()
        .title(title.to_owned())
        .description(message.to_owned())
        .cancel("no", t!("Cancel"))
        .destructive("yes", yes.to_owned())
        .show()
        .await;
    answer.ok().and_then(|a| a.action).is_some_and(|a| a == "yes")
}

/// Asks yes or no where yes loses nothing (trusting, going on); true for yes.
pub async fn agree(title: &str, message: &str, yes: &str) -> bool {
    let answer = alert_dialog()
        .title(title.to_owned())
        .description(message.to_owned())
        .cancel("no", t!("Cancel"))
        .action("yes", yes.to_owned())
        .show()
        .await;
    answer.ok().and_then(|a| a.action).is_some_and(|a| a == "yes")
}

/// Picks one of `actions` (id, label) from a sheet.
pub async fn choose(title: &str, actions: &[(&str, &str)]) -> Option<String> {
    let mut sheet = alert_dialog().title(title.to_owned()).action_sheet().cancel("cancel", t!("Cancel"));
    for (id, label) in actions {
        sheet = sheet.action(id.to_string(), label.to_string());
    }
    let action = sheet.show().await.ok()?.action?;
    (action != "cancel").then(|| action.to_string())
}

/// A question that wants words: shown while `asking` holds one.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub title: String,
    pub placeholder: String,
    pub value: String,
    pub done: &'static str,
    /// Words not to show (a password).
    pub masked: bool,
}

/// The dialog of [`Prompt`]s: `answer` gets the words (None: cancelled).
pub fn name_prompt(value: String) -> Prompt {
    Prompt { title: t!("Rename").into(), placeholder: t!("Display name").into(), value, done: t!("OK"), masked: false }
}

pub fn prompt_dialog(asking: Signal<Option<Prompt>>, answer: impl Fn(Option<String>) + 'static) -> impl IntoElement {
    let answer = std::rc::Rc::new(answer);
    when(move || asking.with(Option::is_some), move || {
        let p = asking.get_untracked().expect("a prompt");
        let words = signal(p.value.clone());
        let finish = {
            let answer = answer.clone();
            move |given: Option<String>| {
                asking.set(None);
                answer(given);
            }
        };
        let finish = std::rc::Rc::new(finish);
        let (cancel, submit, done) = (finish.clone(), finish.clone(), finish.clone());
        dialog().animation(ModalAnimation::Fade).on_cancel(move || cancel(None)).child(
            view()
                .flex_1()
                .bg(rgba(0x00000066))
                .items_center()
                .justify_center()
                .pb(move || px(keyboard_height().0))
                .child(
                    view()
                        .w(px(320.0))
                        .gap(px(14.0))
                        .p(px(20.0))
                        .rounded(px(16.0))
                        .bg(theme::POPOVER)
                        .border_1()
                        .border_color(theme::BORDER)
                        .on_press(|| {})
                        .child(text(p.title.clone()).text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND))
                        .child(
                            input()
                                .test_id("prompt")
                                .masked(p.masked)
                                .value(move || words.get())
                                .on_change(move |t: &str| words.set(t.to_owned()))
                                .on_submit(move |t: &str| submit(Some(t.to_owned())))
                                .placeholder(p.placeholder.clone())
                                .placeholder_color(move || theme::now(theme::DIM))
                                .auto_focus(true)
                                .auto_correct(false)
                                .auto_capitalize(AutoCapitalize::None)
                                .return_key_type(ReturnKeyType::Done)
                                .rounded(px(10.0))
                                .border_1()
                                .border_color(theme::INPUT)
                                .px(px(12.0))
                                .h(px(44.0))
                                .text_size(px(16.0))
                                .text_color(theme::FOREGROUND),
                        )
                        .child(
                            view()
                                .flex_row()
                                .justify_end()
                                .gap(px(8.0))
                                .child(ghost_button(t!("Cancel"), {
                                    let finish = finish.clone();
                                    move || finish(None)
                                }))
                                .child(button(p.done.to_owned(), move || done(Some(words.get_untracked())))),
                        ),
                ),
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn titles_lose_markers_and_what_cannot_be_seen() {
        assert_eq!(super::title("[ ! ] Action Required | Create notes\u{feff}\u{feff}"), "Create notes");
        assert_eq!(super::title("\u{2733} Fix the build"), "Fix the build");
        assert_eq!(super::title("a | b"), "a | b");
        assert_eq!(super::title("[x] | "), "[x] |");
    }
}
