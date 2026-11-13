//! A sheet of choices from the bottom, as the "more" sheet is: rows with
//! what they are drawn before them, in sections, the one shown checked.
//! Asked from anywhere, one at a time; [`host`] draws it over every page.

use crate::{theme, ui};
use nsh_base::sheet;
use nsh_core::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;

/// What is drawn before a choice.
#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    None,
    /// One of the lists' glyphs (`assets/chrome`).
    Glyph(&'static str),
    /// A program's own (its programs.json entry).
    Program(Value),
    /// A space's mark: its name and color.
    Space(String, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub icon: Icon,
    pub status: Option<&'static str>,
    /// The one shown now.
    pub checked: bool,
    /// It loses something (closes, forgets, stops): in red.
    pub danger: bool,
}

impl Item {
    pub fn new(id: impl Into<String>, label: impl Into<String>, icon: Icon) -> Item {
        Item { id: id.into(), label: label.into(), icon, status: None, checked: false, danger: false }
    }

    /// With one of the lists' glyphs.
    pub fn glyph(id: impl Into<String>, label: impl Into<String>, glyph: &'static str) -> Item {
        Item::new(id, label, Icon::Glyph(glyph))
    }

    pub fn danger(self) -> Item {
        Item { danger: true, ..self }
    }

    pub fn checked(self, checked: bool) -> Item {
        Item { checked, ..self }
    }
}

/// Choices under a heading (none: no heading).
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: Option<String>,
    pub items: Vec<Item>,
}

/// A sheet asked for: its title and its sections.
type Shown = Option<Rc<(String, Vec<Section>)>>;
/// Who gets the id picked.
type Answer = Rc<RefCell<Option<tokio::sync::oneshot::Sender<Option<String>>>>>;

/// The sheet's slot and who gets the answer.
struct Host {
    shown: Signal<Shown>,
    answer: Answer,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Asks for one of `sections`' choices: its id, or None when the sheet
/// is let go (or there is nowhere to show it). The keyboard goes first:
/// the sheet rises from the bottom, where it would be under it.
pub async fn menu(title: &str, sections: Vec<Section>) -> Option<String> {
    dismiss_keyboard();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let shown = HOST.with(|h| {
        let h = h.borrow();
        let h = h.as_ref()?;
        if let Some(earlier) = h.answer.borrow_mut().replace(tx) {
            let _ = earlier.send(None);
        }
        Some(h.shown)
    })?;
    shown.set(Some(Rc::new((title.to_owned(), sections))));
    rx.await.ok().flatten()
}

/// One of `items`, in a section of their own.
pub async fn one(title: &str, items: Vec<Item>) -> Option<String> {
    menu(title, vec![Section { title: None, items }]).await
}

/// The sheet, over every page.
pub fn host() -> impl IntoElement {
    let shown = signal(Shown::None);
    let answer = Answer::default();
    HOST.with(|h| *h.borrow_mut() = Some(Host { shown, answer: answer.clone() }));
    let finish = Rc::new(move |picked: Option<String>| {
        shown.set(None);
        if let Some(tx) = answer.borrow_mut().take() {
            let _ = tx.send(picked);
        }
    });
    let (close, cancel) = (finish.clone(), finish.clone());
    sheet()
        .open(move || Some(shown.with(Option::is_some)))
        .close_on_swipe(true)
        .backdrop_class("bg-black/40")
        .on_close(move || close(None))
        .on_cancel(move || cancel(None))
        .child(dynamic(move || {
            let Some(menu) = shown.get() else { return view().into_element() };
            let finish = finish.clone();
            view()
                .w_full()
                .items_center()
                .child(
                    view()
                        .w_full()
                        .max_w(px(560.0))
                        .rounded_t(px(18.0))
                        .bg(theme::CHROME)
                        .pt(px(8.0))
                        .pb(move || px(safe_area().bottom.0 + 8.0))
                        .child(view().w(px(36.0)).h(px(4.0)).rounded(px(2.0)).bg(theme::BORDER).self_center().mb(px(4.0)))
                        .child(text(menu.0.clone()).px(px(20.0)).py(px(8.0)).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::MUTED))
                        .child(
                            scroll().max_h(move || px(window().height.0 * 0.7)).child(
                                view().children(menu.1.iter().enumerate().map(|(at, section)| {
                                    let finish = finish.clone();
                                    view()
                                        .children((at > 0).then(|| view().h(px(1.0)).mx(px(20.0)).my(px(6.0)).bg(theme::BORDER)))
                                        .children(section.title.clone().map(|t| text(t).px(px(20.0)).pt(px(6.0)).pb(px(2.0)).text_size(px(12.0)).text_color(theme::DIM)))
                                        .children(section.items.iter().cloned().map(move |item| {
                                            let finish = finish.clone();
                                            let id = item.id.clone();
                                            row(item, move || finish(Some(id.clone())))
                                        }))
                                })),
                            ),
                        ),
                )
                .into_element()
        }))
}

/// One choice: its icon, its words, then its status and a check.
fn row(item: Item, press: impl Fn() + 'static) -> impl IntoElement {
    let ink = if item.danger { theme::BLOCKED } else { theme::FOREGROUND };
    let has_icon = item.icon != Icon::None;
    let icon = match item.icon {
        Icon::None => view().into_element(),
        Icon::Glyph(name) => ui::glyph(name, 20.0, ink).into_element(),
        Icon::Program(program) => ui::program_icon(move || program.clone(), 20.0).into_element(),
        Icon::Space(name, color) => ui::space_mark(move || name.clone(), move || color).into_element(),
    };
    let status = item.status;
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(20.0))
        .h(px(52.0))
        .active(|s| s.bg(theme::HOVER))
        .test_id(&format!("pick-{}", item.id))
        .role(AccessibilityRole::Button)
        .label(item.label.clone())
        .accessibility_state(AccessibilityState { selected: item.checked, ..Default::default() })
        .on_press(press)
        .children(has_icon.then(|| view().w(px(22.0)).items_center().child(icon)))
        .child(
            text(item.label)
                .flex_shrink(1.0)
                .text_size(px(16.0))
                .font_weight(if item.checked { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(ink)
                .number_of_lines(1),
        )
        .child(view().flex_1())
        .child(ui::tab_status(move || status))
        .children(item.checked.then(|| ui::glyph("check", 18.0, theme::ACCENT)))
}
