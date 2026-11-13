//! The key bar's keys: which, in what order (Settings, Key bar). The
//! terminal shows them as chosen, between Enter and Hide keyboard.

use crate::prefs::Prefs;
use crate::{theme, ui};
use herdc_term::keys::Key;
use nsh_core::prelude::*;

/// What a key of the bar does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Does {
    Key(Key),
    /// Holds Ctrl or Alt for the next key.
    Ctrl,
    Alt,
    /// Types what it shows.
    Text,
    Paste,
}

/// Every key the bar can have: its id (as kept), what it shows, what it
/// does. The first [`AT_FIRST`] are on it until chosen otherwise.
pub const KEYS: &[(&str, &str, Does)] = &[
    ("esc", "Esc", Does::Key(Key::Escape)),
    ("tab", "Tab", Does::Key(Key::Tab)),
    ("ctrl", "Ctrl", Does::Ctrl),
    ("alt", "Alt", Does::Alt),
    ("paste", "Paste", Does::Paste),
    ("left", "←", Does::Key(Key::Left)),
    ("down", "↓", Does::Key(Key::Down)),
    ("up", "↑", Does::Key(Key::Up)),
    ("right", "→", Does::Key(Key::Right)),
    ("|", "|", Does::Text),
    ("~", "~", Does::Text),
    ("/", "/", Does::Text),
    ("-", "-", Does::Text),
    ("home", "Home", Does::Key(Key::Home)),
    ("end", "End", Does::Key(Key::End)),
    ("pgup", "PgUp", Does::Key(Key::PageUp)),
    ("pgdn", "PgDn", Does::Key(Key::PageDown)),
    ("\\", "\\", Does::Text),
    ("`", "`", Does::Text),
    (":", ":", Does::Text),
    (";", ";", Does::Text),
    ("_", "_", Does::Text),
    ("$", "$", Does::Text),
    ("*", "*", Does::Text),
    ("&", "&", Does::Text),
    ("'", "'", Does::Text),
    ("\"", "\"", Does::Text),
    ("<", "<", Does::Text),
    (">", ">", Does::Text),
    ("[", "[", Does::Text),
    ("]", "]", Does::Text),
    ("{", "{", Does::Text),
    ("}", "}", Does::Text),
    ("del", "Del", Does::Key(Key::Delete)),
    ("ins", "Ins", Does::Key(Key::Insert)),
    ("f1", "F1", Does::Key(Key::F(1))),
    ("f2", "F2", Does::Key(Key::F(2))),
    ("f3", "F3", Does::Key(Key::F(3))),
    ("f4", "F4", Does::Key(Key::F(4))),
    ("f5", "F5", Does::Key(Key::F(5))),
    ("f6", "F6", Does::Key(Key::F(6))),
    ("f7", "F7", Does::Key(Key::F(7))),
    ("f8", "F8", Does::Key(Key::F(8))),
    ("f9", "F9", Does::Key(Key::F(9))),
    ("f10", "F10", Does::Key(Key::F(10))),
    ("f11", "F11", Does::Key(Key::F(11))),
    ("f12", "F12", Does::Key(Key::F(12))),
];

pub const AT_FIRST: usize = 17;

/// The bar as it is at first.
pub fn at_first() -> Vec<String> {
    KEYS[..AT_FIRST].iter().map(|(id, ..)| (*id).to_owned()).collect()
}

/// What key `id` shows and does.
pub fn find(id: &str) -> Option<(&'static str, Does)> {
    KEYS.iter().find(|(k, ..)| *k == id).map(|(_, shows, does)| (*shows, *does))
}

/// What a reader says for a key that shows a sign (and the list calls it).
pub fn spoken(shows: &'static str) -> &'static str {
    match shows {
        "↵" => t!("Enter"),
        "←" => t!("Left"),
        "↓" => t!("Down"),
        "↑" => t!("Up"),
        "→" => t!("Right"),
        "|" => t!("Pipe"),
        "~" => t!("Tilde"),
        "/" => t!("Slash"),
        "-" => t!("Dash"),
        "Paste" => t!("Paste"),
        "⌄" => t!("Hide keyboard"),
        _ => shows,
    }
}

/// Settings, Key bar: the keys on it, to move or take off, and the others
/// to put on.
pub fn keybar_page() -> nsh_core::element::Screen {
    let prefs = use_context::<Prefs>().expect("prefs");
    let keys = prefs.keys;
    let others = computed(move || keys.with(|on| KEYS.iter().map(|(id, ..)| *id).filter(|id| !on.iter().any(|k| k == id)).collect::<Vec<_>>()));
    ui::page(t!("Key bar")).child(
        scroll().flex_1().bg(theme::CANVAS).child(
            view()
                .pb(move || px(safe_area().bottom.0 + 24.0))
                .child(ui::heading(|| t!("ON THE BAR").into()))
                .child(each(move || keys.get(), |id| id.clone(), move |id, _| on_bar(keys, id)))
                .child(ui::heading(|| t!("MORE KEYS").into()))
                .child(
                    view().flex_row().flex_wrap().px(px(12.0)).gap(px(6.0)).child(each(
                        move || others.get(),
                        |id| *id,
                        move |id, _| {
                            let id = id.get_untracked();
                            let shows = find(id).map_or(id, |(shows, _)| shows);
                            view()
                                .min_w(px(48.0))
                                .h(px(40.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(theme::BORDER)
                                .items_center()
                                .justify_center()
                                .active(|s| s.bg(theme::HOVER))
                                .role(AccessibilityRole::Button)
                                .label(t!("Add {}", spoken(shows)))
                                .on_press(move || keys.update(|k| k.push(id.to_owned())))
                                .child(text(shows).font_family(theme::MONO).text_size(px(14.0)).text_color(theme::FOREGROUND))
                        },
                    )),
                )
                .child(ui::action_row(t!("As at first"), ui::Act::Do, move || keys.set(at_first())).mt(px(12.0))),
        ),
    )
}

/// One of a key's quiet controls; nothing to do (the first one up), faint.
fn control(icon: &'static str, label: String, ink: AdaptiveColor, idle: impl Fn() -> bool + 'static, press: impl Fn() + 'static) -> View {
    view()
        .w(px(40.0))
        .h(px(40.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(label)
        .disabled(idle)
        .disabled_style(|s| s.opacity(0.25))
        .on_press(press)
        .child(ui::glyph(icon, 18.0, ink))
}

/// A key on the bar: earlier, later, off.
fn on_bar(keys: Signal<Vec<String>>, id: ReadSignal<String>) -> View {
    let id = id.get_untracked();
    let shows = find(&id).map_or("?", |(shows, _)| shows);
    let at = {
        let id = id.clone();
        move |k: &Vec<String>| k.iter().position(|o| *o == id)
    };
    let (first, last) = {
        let (a, b) = (at.clone(), at.clone());
        (move || keys.with(|k| a(k) == Some(0)), move || keys.with(|k| b(k).is_some_and(|i| i + 1 == k.len())))
    };
    let step = move |by: isize| {
        let at = at.clone();
        move || {
            keys.update(|k| {
                if let Some(i) = at(k) {
                    let to = (i as isize + by).clamp(0, k.len() as isize - 1) as usize;
                    k.swap(i, to);
                }
            })
        }
    };
    let name = spoken(shows);
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .px(px(16.0))
        .min_h(px(48.0))
        .child(text(shows).font_family(theme::MONO).text_size(px(15.0)).text_color(theme::FOREGROUND).w(px(56.0)))
        .child(text(if name == shows { String::new() } else { name.to_owned() }).flex_1().text_size(px(13.0)).text_color(theme::MUTED))
        .child(control("arrow-up", t!("{} earlier", name), theme::MUTED, first, step(-1)))
        .child(control("arrow-down", t!("{} later", name), theme::MUTED, last, step(1)))
        .child(control("x", t!("Take {} off", name), theme::DIM, || false, move || keys.update(|k| k.retain(|o| *o != id))))
}
