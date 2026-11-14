//! The app's small parts: bars, buttons, rows, dots and the dialogs that
//! ask for a word or a yes.

use crate::theme;
use nsh_base::{alert_dialog, dialog};
use nsh_base::input;
use nsh_core::prelude::*;
use nsh_core::Checked;

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

/// A square press target with one of the lists' glyphs (`assets/chrome`).
pub fn icon_button(icon: &'static str, label: &'static str, press: impl Fn() + 'static) -> View {
    view()
        .w(px(44.0))
        .h(px(44.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .active(|s| s.bg(theme::HOVER).scale(PRESSED))
        .transition()
        .duration(QUICK)
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(press)
        .child(glyph(icon, 22.0, theme::FOREGROUND))
}

/// How long a press's or a switch's change takes, in ms.
pub const QUICK: f32 = 140.0;
/// How long the layout takes to settle (a group folds, a row moves), in ms.
pub const SETTLE: f32 = 240.0;
/// How small a button goes while pressed.
pub const PRESSED: f32 = 0.92;

/// The next change of the layout moves into place rather than jumps: what
/// goes fades, what stays slides. Call it just before the change.
pub fn smoothly() {
    use nsh_core::{LayoutAnimation, LayoutCurve, LayoutTransition};
    animate_layout(LayoutAnimation::all(LayoutTransition::new(LayoutCurve::EaseInEaseOut, SETTLE)));
}

/// A fold's chevron: down while open, turned right while folded.
pub fn chevron(open: impl Fn() -> bool + 'static, size: f32) -> impl IntoElement {
    glyph("chevron-down", size, theme::DIM).rotate(move || deg(if open() { 0.0 } else { -90.0 })).transition_transform().duration(SETTLE)
}

/// On or off, taking effect at once: a track the thumb slides along, in
/// the accent while on. (NSH's `switch` leaves its look to the app.)
pub fn toggle(on: Signal<bool>, label: &'static str) -> View {
    view()
        .w(px(44.0))
        .h(px(26.0))
        .p(px(3.0))
        .rounded(px(13.0))
        .flex_shrink(0.0)
        .bg(move || if on.get() { theme::now(theme::ACCENT) } else { theme::now(theme::INPUT) })
        .transition_colors()
        .duration(QUICK)
        .role(AccessibilityRole::Switch)
        .label(label)
        .accessibility_state(move || AccessibilityState { checked: Some(if on.get() { Checked::True } else { Checked::False }), ..Default::default() })
        .hit_slop(EdgeInsets::all(px(10.0)))
        .on_press(move || on.update(|o| *o = !*o))
        .child(
            view()
                .w(px(20.0))
                .h(px(20.0))
                .rounded(px(10.0))
                .bg(rgb(0xffffff))
                .translate_x(move || px(if on.get() { 18.0 } else { 0.0 }))
                .transition_transform()
                .duration(QUICK),
        )
}

/// What an [`action_row`] does: leads to a page, does something here, or
/// does something that loses something.
#[derive(Clone, Copy, PartialEq)]
pub enum Act {
    Go,
    Do,
    Lose,
}

/// A list row that is pressed: its words in the color of what it does, a
/// chevron where it leads to a page.
pub fn action_row(title: &'static str, act: Act, press: impl Fn() + 'static) -> View {
    let color = match act {
        Act::Go => theme::FOREGROUND,
        Act::Do => theme::ACCENT,
        Act::Lose => theme::BLOCKED,
    };
    view()
        .flex_row()
        .items_center()
        .px(px(16.0))
        .h(px(52.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(title)
        .on_press(press)
        .child(text(title).flex_1().text_size(theme::ROW).text_color(color).number_of_lines(1))
        .child(when(move || act == Act::Go, || glyph("chevron-right", 16.0, theme::DIM)))
}

/// The main action of a page.
pub fn button(label: impl IntoReactive<String> + 'static, press: impl Fn() + 'static) -> View {
    busy_button(label, || false, press)
}

/// The main action, turning while what it started goes on (`busy`, its
/// label saying what).
pub fn busy_button(label: impl IntoReactive<String> + 'static, busy: impl Fn() -> bool + 'static, press: impl Fn() + 'static) -> View {
    let label = label.into_reactive();
    view()
        .flex_row()
        .gap(px(10.0)) 
        .h(px(48.0))
        .px(px(20.0))
        .rounded(px(12.0))
        .items_center()
        .justify_center()
        .bg(theme::SOLID)
        .active(|s| s.opacity(0.8).scale(0.97))
        .transition()
        .duration(QUICK)
        .role(AccessibilityRole::Button)
        .on_press(press)
        .child(when(busy, || glyph("spinner", 16.0, theme::SOLID_FOREGROUND).animate_spin()))
        .child(text(move || label.get()).text_size(theme::BODY).font_weight(FontWeight::SEMIBOLD).text_color(theme::SOLID_FOREGROUND))
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
        .active(|s| s.bg(theme::HOVER).scale(0.97))
        .transition()
        .duration(QUICK)
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(press)
        .child(text(label).text_size(theme::ROW).text_color(theme::FOREGROUND))
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
pub fn faint(color: AdaptiveColor) -> impl Fn() -> Hsla {
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
            .child(text("!").text_size(theme::SMALL).font_weight(FontWeight::BOLD).text_color(theme::BLOCKED))
            .into_element(),
        (Some("blocked"), false) => view()
            .px(px(6.0))
            .h(px(18.0))
            .rounded(px(5.0))
            .bg(faint(theme::BLOCKED))
            .justify_center()
            .child(text(t!("Needs you")).text_size(theme::SMALL).font_weight(FontWeight::SEMIBOLD).text_color(theme::BLOCKED))
            .into_element(),
        // Idle is the usual: nothing to say, as on the tabs and spaces.
        (Some("idle") | None, _) => view().w(px(7.0)).h(px(7.0)).mx(px(3.0)).into_element(),
        (Some(word), _) => view().w(px(7.0)).h(px(7.0)).mx(px(3.0)).rounded(px(3.5)).bg(theme::status(word)).into_element(),
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
                .text_size(theme::CAPTION)
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xffffff)),
        )
}

/// A space's mark with what runs there on its corner: how an agent or a
/// terminal is listed (Agents, Jump to a terminal).
pub fn space_badge(name: impl Fn() -> String + 'static, color: impl Fn() -> usize + 'static, program: impl Fn() -> serde_json::Value + 'static) -> View {
    view().child(space_mark(name, color)).child(
        view()
            .absolute()
            .right(px(-5.0))
            .bottom(px(-5.0))
            .p(px(1.5))
            .rounded(px(5.0))
            .bg(theme::POPOVER)
            .child(program_icon(program, 11.0)),
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
        .child(text(label).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
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
                .text_size(theme::BODY)
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
                .child(text(title).text_size(theme::ROW).text_color(theme::FOREGROUND).number_of_lines(1))
                .child(text(detail).text_size(theme::CAPTION).text_color(theme::MUTED).number_of_lines(1)),
        )
}

/// A group's heading over its card, the same on Agents and Spaces: the
/// fold's chevron, its title in `ink`, how many, then what the caller adds
/// at its end. A tap folds the group (the caller's `on_press`).
pub fn group_heading(open: impl Fn() -> bool + 'static, title: impl Fn() -> String + 'static, ink: impl Fn() -> Hsla + 'static, count: impl Fn() -> usize + 'static) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .mx(theme::INSET)
        .mt(px(10.0))
        .pl(px(10.0))
        .pr(px(4.0))
        .h(px(36.0))
        .rounded(px(8.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .child(chevron(open, 12.0))
        .child(text(title).flex_shrink(1.0).text_size(theme::CAPTION).font_weight(FontWeight::SEMIBOLD).text_color(ink).number_of_lines(1))
        .child(text(move || count().to_string()).text_size(theme::CAPTION).text_color(theme::DIM))
        .child(view().flex_1())
}

/// A row's line on its card, above it, from where its words start (after
/// a 22 pt mark): for rows that move on their own (a space dragged).
pub fn row_line() -> View {
    view().absolute().top(px(0.0)).left(px(46.0)).right(px(0.0)).h(px(1.0)).bg(theme::BORDER)
}

/// What a card's rows need said, under it in small words.
pub fn card_footer(said: &'static str) -> View {
    view().px(px(32.0)).pt(px(8.0)).child(text(said).text_size(theme::CAPTION).line_height(px(17.0)).text_color(theme::DIM))
}

/// A card's heading, in line with its rows' words.
pub fn card_heading(title: &'static str) -> View {
    view()
        .px(px(32.0))
        .pt(px(22.0))
        .pb(px(8.0))
        .child(text(title).text_size(theme::CAPTION).font_weight(FontWeight::SEMIBOLD).text_color(theme::DIM))
}

/// Rows that go together, on a card, a line between each from where their
/// words start (after a row's glyph).
pub fn card(rows: Vec<View>) -> View {
    let last = rows.len().saturating_sub(1);
    view().mx(theme::INSET).rounded(theme::CARD_RADIUS).overflow_hidden().bg(theme::POPOVER).children(rows.into_iter().enumerate().map(move |(at, row)| {
        view().child(row).children((at < last).then(|| view().h(px(1.0)).ml(px(50.0)).bg(theme::BORDER)))
    }))
}

/// A section's heading in a list.
pub fn heading(title: impl Fn() -> String + 'static) -> View {
    view()
        .px(px(16.0))
        .pt(px(18.0))
        .pb(px(6.0))
        .child(text(title).text_size(theme::CAPTION).font_weight(FontWeight::SEMIBOLD).text_color(theme::DIM))
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
                        .child(text(p.title.clone()).text_size(theme::HEADLINE).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND))
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
                                .text_size(theme::BODY)
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

/// `title` without one of `names` at its start or end, and what joins it
/// there (` | `, ` - `, ` · `): what an agent adds of where it works
/// (`π - nova`), where the space already says it.
pub fn without(title: &str, names: &[String]) -> String {
    for name in names.iter().filter(|n| !n.is_empty()) {
        if title == name {
            return String::new();
        }
        for join in [" | ", " - ", " – ", " — ", " · ", ": "] {
            let rest = title.strip_suffix(&format!("{join}{name}")).or_else(|| title.strip_prefix(&format!("{name}{join}")));
            if let Some(rest) = rest {
                return rest.trim().to_owned();
            }
        }
    }
    title.to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn titles_lose_the_space_s_name() {
        let names = ["nova".to_owned(), "nova".to_owned()];
        let without = |t: &str| super::without(t, &names);
        assert_eq!(without("π - nova"), "π");
        assert_eq!(without("nova"), "");
        assert_eq!(without("检查输入框回车键缺失 | nova"), "检查输入框回车键缺失");
        assert_eq!(without("nova · notes"), "notes");
        assert_eq!(without("nova2 | fix"), "nova2 | fix");
        assert_eq!(super::without("Fix it", &[]), "Fix it");
    }

    #[test]
    fn titles_lose_markers_and_what_cannot_be_seen() {
        assert_eq!(super::title("[ ! ] Action Required | Create notes\u{feff}\u{feff}"), "Create notes");
        assert_eq!(super::title("\u{2733} Fix the build"), "Fix the build");
        assert_eq!(super::title("a | b"), "a | b");
        assert_eq!(super::title("[x] | "), "[x] |");
    }
}
