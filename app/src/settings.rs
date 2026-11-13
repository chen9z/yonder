//! Settings: the terminal's size and fit, the app's language, this
//! phone's key, and the computer it is on.

use crate::i18n::Language;
use crate::live::{self, Live};
use crate::prefs::Prefs;
use crate::nav::{self, Route};
use crate::{link, theme, ui};
use crate::ui::Act;
use herdc_engine::Link;
use nsh_core::prelude::*;

pub fn settings_page(live: Live) -> nsh_core::element::Screen {
    let prefs = use_context::<Prefs>().expect("prefs");
    let world = live.view;
    let public = link::phone_key().map(|(_, public)| public).unwrap_or_else(|error| t!("Cannot load the phone's key: {}", error));
    let about = live.clone();
    let core = computed(move || {
        world.with(|v| match &v.link {
            Link::Connected { core, version } => format!("{core} · herdr-core {version}"),
            Link::Connecting => t!("Connecting…").into(),
            Link::Lost { error } => link::Lost::of(error).said(&live.computer.host),
        })
    });
    // The line's state, as a dot by the computer's name.
    let state = computed(move || world.with(|v| match &v.link {
        Link::Connected { .. } => theme::ADDED,
        Link::Connecting => theme::WORKING,
        Link::Lost { .. } => theme::BLOCKED,
    }));
    let copied = about.clone();
    ui::page(t!("Settings")).child(
        scroll().flex_1().bg(theme::CANVAS).child(
            view()
                .pt(px(4.0))
                .pb(move || px(safe_area().bottom.0 + 24.0))
                .child(ui::card_heading(t!("TERMINAL")))
                .child(ui::card(vec![
                    setting("type", || t!("Font size").into(), move || t!("{} pt", prefs.font.get())).child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(34.0))
                            .rounded(px(10.0))
                            .bg(theme::HOVER)
                            .child(step("minus", t!("Smaller text"), move || prefs.font.update(|f| *f = (*f - 1.0).max(crate::terminal::FONT.0))))
                            .child(view().w(px(1.0)).h(px(16.0)).bg(theme::BORDER))
                            .child(step("plus", t!("Larger text"), move || prefs.font.update(|f| *f = (*f + 1.0).min(crate::terminal::FONT.1)))),
                    ),
                    setting("smartphone", || t!("Fit terminals to the phone").into(), || t!("Else they keep the desktop's size, drawn smaller").into())
                        .child(ui::toggle(prefs.fit, t!("Fit terminals to the phone"))),
                    leads(setting("keyboard", || t!("Key bar").into(), move || t!("{} keys", prefs.keys.with(Vec::len))), || nav::push(Route::KeyBar)),
                    leads(
                        setting("languages", || t!("Language").into(), move || match prefs.language.get() {
                            Language::System => t!("System").into(),
                            Language::English => "English".into(),
                            Language::Chinese => "中文".into(),
                        }),
                        move || {
                            spawn(async move {
                                use crate::pick::{self, Icon, Item};
                                let now = prefs.language.get_untracked();
                                let items = vec![
                                    Item::new("system", t!("System"), Icon::None).checked(now == Language::System),
                                    Item::new("en", "English", Icon::None).checked(now == Language::English),
                                    Item::new("zh", "中文", Icon::None).checked(now == Language::Chinese),
                                ];
                                match pick::one(t!("Language"), items).await.as_deref() {
                                    Some("system") => prefs.language.set(Language::System),
                                    Some("en") => prefs.language.set(Language::English),
                                    Some("zh") => prefs.language.set(Language::Chinese),
                                    _ => {}
                                }
                            });
                        },
                    ),
                ]))
                .child(ui::card_heading(t!("COMPUTER")))
                .child(ui::card(vec![
                    setting("monitor", move || live.computer.name.clone(), move || core.get())
                        .child(view().w(px(8.0)).h(px(8.0)).mr(px(4.0)).rounded(px(4.0)).bg(move || theme::now(state.get()))),
                    action("plug", t!("Edit connection"), Act::Go, || nav::push(Route::EditConnection)),
                    action("log-out", t!("Leave this computer"), Act::Lose, live::close),
                ]))
                .child(ui::card_heading(t!("THIS PHONE'S KEY")))
                .child(ui::card(vec![
                    view().px(px(16.0)).py(px(14.0)).child(
                        text(public.clone())
                            .font_family(theme::MONO)
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(theme::MUTED)
                            .selectable(true),
                    ),
                    action("copy", t!("Copy"), Act::Do, move || {
                        set_clipboard(public.clone());
                        copied.say(herdc_engine::Note::info(t!("Copied"), t!("Add it to ~/.ssh/authorized_keys on the computer.")));
                    }),
                ]))
                .child(ui::card_heading(t!("ABOUT")))
                .child(ui::card(vec![
                    setting("info", || "Yonder".into(), version),
                    action("stethoscope", t!("Copy diagnostics"), Act::Do, move || {
                        set_clipboard(diagnostics(&about, prefs));
                        about.say(herdc_engine::Note::info(t!("Copied"), t!("Paste it where you tell what went wrong.")));
                    }),
                ])),
        ),
    )
}

/// A setting that leads to a page or a choice: pressed, with a chevron.
fn leads(row: View, press: impl Fn() + 'static) -> View {
    row.active(|s| s.bg(theme::HOVER)).role(AccessibilityRole::Button).on_press(press).child(ui::glyph("chevron-right", 16.0, theme::DIM))
}

/// A row that does something: its glyph and words in the color of what
/// it does, a chevron where it leads to a page.
fn action(icon: &'static str, title: &'static str, act: Act, press: impl Fn() + 'static) -> View {
    let ink = match act {
        Act::Go => theme::FOREGROUND,
        Act::Do => theme::ACCENT,
        Act::Lose => theme::BLOCKED,
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .h(px(52.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(title)
        .on_press(press)
        .child(ui::glyph(icon, 20.0, if act == Act::Go { theme::MUTED } else { ink }))
        .child(text(title).flex_1().text_size(px(15.0)).text_color(ink).number_of_lines(1))
        .children((act == Act::Go).then(|| ui::glyph("chevron-right", 16.0, theme::DIM)))
}

/// The app's release build number and source revision (`apk.sh`).
fn version() -> String {
    let build = option_env!("HERDC_BUILD").map(|b| format!(" ({b}, {})", option_env!("HERDC_COMMIT").unwrap_or("?"))).unwrap_or_default();
    format!("{}{build}", env!("CARGO_PKG_VERSION"))
}

/// What to tell when something goes wrong: the app, the phone, the
/// computer and how the line to it is; no secrets (the computer's
/// address and user are said, its password and the phone's key aren't).
fn diagnostics(live: &Live, prefs: Prefs) -> String {
    let (major, minor) = os_version();
    let size = window();
    let c = &live.computer;
    let mut out = vec![
        format!("Yonder {}", version()),
        format!("Android {major}.{minor} · {:.0}×{:.0} pt · {}", size.width.0, size.height.0, locale().as_str()),
        format!("font {} pt · fit {} · language {:?}", prefs.font.get_untracked(), prefs.fit.get_untracked(), prefs.language.get_untracked()),
        format!("key: {}", link::key_kept()),
        format!("computer: {}@{}:{}{}", c.user, c.host, c.port, c.session.as_ref().map(|s| format!(" · session {s}")).unwrap_or_default()),
    ];
    live.view.with_untracked(|v| {
        out.push(format!("link: {}", serde_json::to_string(&v.link).unwrap_or_default()));
        for m in &v.machines {
            let error = m.error.as_ref().map(|e| format!(" ({e})")).unwrap_or_default();
            out.push(format!("machine {}: {} · layout {}{error}", m.name.as_deref().unwrap_or("(core)"), m.state, m.layout));
        }
        let tabs: usize = v.spaces.iter().map(|s| s.tabs.len()).sum();
        out.push(format!("{} spaces · {tabs} tabs · {} terminals · {} agents", v.spaces.len(), v.terminals.len(), v.agents.len()));
    });
    out.join("\n")
}

/// One half of the font size's − / +.
fn step(icon: &'static str, label: &'static str, press: impl Fn() + 'static) -> View {
    view()
        .w(px(44.0))
        .h_full()
        .items_center()
        .justify_center()
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(press)
        .child(ui::glyph(icon, 16.0, theme::FOREGROUND))
}

/// A setting's row: its glyph, what it is, what it is now, and its
/// controls after.
fn setting(icon: &'static str, title: impl Fn() -> String + 'static, now: impl Fn() -> String + 'static) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .py(px(10.0))
        .min_h(px(58.0))
        .child(ui::glyph(icon, 20.0, theme::MUTED))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(text(title).text_size(px(15.0)).text_color(theme::FOREGROUND))
                .child(text(now).text_size(px(12.0)).text_color(theme::MUTED)),
        )
}
