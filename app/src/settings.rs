//! Settings: the computer the app is on (its connection, its machines,
//! leaving it), the terminal's size, fit and keys, the
//! app's language, light or dark, how agents are sorted, this phone's
//! key.

use crate::i18n::Language;
use crate::notes::Notify;
use crate::theme::Look;
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
    let lead_live = live.clone();
    let core = computed(move || {
        world.with(|v| match &v.link {
            Link::Connected { version, .. } => format!("herdr-core {version}"),
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
                .child(ui::card_heading(t!("COMPUTER")))
                .child(ui::card(vec![
                    setting("monitor", move || live.computer.name.clone(), move || core.get())
                        .child(view().w(px(8.0)).h(px(8.0)).mr(px(4.0)).rounded(px(4.0)).bg(move || theme::now(state.get()))),
                    action("plug", t!("Edit connection"), Act::Go, || nav::push(Route::EditConnection)),
                    action("server", t!("Add a machine…"), Act::Go, || nav::push(Route::AddMachine)),
                    // The bots Yonder runs on this computer (its own yonder).
                    action("bot", t!("Bots"), Act::Go, || nav::push(Route::Bots)),
                ]))
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
                    setting("smartphone", || t!("Fit terminals to the phone").into(), String::new).child(ui::toggle(prefs.fit, t!("Fit terminals to the phone"))),
                    leads(setting("keyboard", || t!("Key bar").into(), move || t!("{} keys", prefs.keys.with(Vec::len))), || nav::push(Route::KeyBar)),
                ]))
                .child(ui::card_footer(t!("Fitted, the terminal you look at takes the phone's rows and columns; else it keeps the desktop's size, drawn smaller.")))
                .child(ui::card_heading(t!("GENERAL")))
                .child(ui::card(vec![
                    // The agent the person talks to, on a tab of its own.
                    leads(
                        setting("lead", || t!("Main agent").into(), {
                            let computer = lead_live.computer.id();
                            move || prefs.lead(&computer).map(|l| l.name()).unwrap_or_else(|| t!("None").into())
                        }),
                        move || crate::lead::choose(prefs, lead_live.clone()),
                    ),
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
                    // Light or dark, whatever the phone's is.
                    leads(
                        setting("contrast", || t!("Appearance").into(), move || match prefs.look.get() {
                            Look::System => t!("System").into(),
                            Look::Light => t!("Light").into(),
                            Look::Dark => t!("Dark").into(),
                        }),
                        move || {
                            spawn(async move {
                                use crate::pick::{self, Icon, Item};
                                let now = prefs.look.get_untracked();
                                let items = vec![
                                    Item::new("system", t!("System"), Icon::None).checked(now == Look::System),
                                    Item::new("light", t!("Light"), Icon::None).checked(now == Look::Light),
                                    Item::new("dark", t!("Dark"), Icon::None).checked(now == Look::Dark),
                                ];
                                match pick::one(t!("Appearance"), items).await.as_deref() {
                                    Some("system") => prefs.look.set(Look::System),
                                    Some("light") => prefs.look.set(Look::Light),
                                    Some("dark") => prefs.look.set(Look::Dark),
                                    _ => {}
                                }
                            });
                        },
                    ),
                    // Which agents' news comes over the page.
                    leads(
                        setting("bell", || t!("Notes").into(), move || match prefs.notify.get() {
                            Notify::Important => t!("Important only").into(),
                            Notify::All => t!("All").into(),
                            Notify::Off => t!("Off").into(),
                        }),
                        move || {
                            spawn(async move {
                                use crate::pick::{self, Icon, Item};
                                let now = prefs.notify.get_untracked();
                                let items = vec![
                                    Item::new("important", t!("Important only"), Icon::None).checked(now == Notify::Important),
                                    Item::new("all", t!("All"), Icon::None).checked(now == Notify::All),
                                    Item::new("off", t!("Off"), Icon::None).checked(now == Notify::Off),
                                ];
                                match pick::one(t!("Notes"), items).await.as_deref() {
                                    Some("important") => prefs.notify.set(Notify::Important),
                                    Some("all") => prefs.notify.set(Notify::All),
                                    Some("off") => prefs.notify.set(Notify::Off),
                                    _ => {}
                                }
                            });
                        },
                    ),
                    // How the Agents list goes: by what needs you, or by space.
                    leads(
                        setting("layers", || t!("Sort agents").into(), move || if prefs.grouped.get() { t!("Grouped by space").into() } else { t!("Priority").into() }),
                        move || {
                            spawn(async move {
                                use crate::pick::{self, Icon, Item};
                                let on = prefs.grouped.get_untracked();
                                let items = vec![
                                    Item::new("priority", t!("Priority"), Icon::Glyph("clock")).checked(!on),
                                    Item::new("grouped", t!("Grouped by space"), Icon::Glyph("layers")).checked(on),
                                ];
                                match pick::one(t!("Sort agents"), items).await.as_deref() {
                                    Some("priority") => prefs.grouped.set(false),
                                    Some("grouped") => prefs.grouped.set(true),
                                    _ => {}
                                }
                            });
                        },
                    ),
                ]))
                .child(ui::card_heading(t!("THIS PHONE'S KEY")))
                .child(ui::card(vec![action_saying("copy", t!("Copy the public key"), short_key(&public), Act::Do, move || {
                    set_clipboard(public.clone());
                    copied.say(herdc_engine::Note::info(t!("Copied"), t!("Add it to ~/.ssh/authorized_keys on the computer.")));
                })]))
                .child(ui::card_footer(t!("On a computer, in ~/.ssh/authorized_keys, it logs the phone in without the password.")))
                .child(ui::card_heading(t!("ABOUT")))
                .child(ui::card(vec![
                    setting("info", || "Yonder".into(), version),
                    action("stethoscope", t!("Copy diagnostics"), Act::Do, move || {
                        set_clipboard(diagnostics(&about, prefs));
                        about.say(herdc_engine::Note::info(t!("Copied"), t!("Paste it where you tell what went wrong.")));
                    }),
                ]))
                // Leaving, last and alone, as losing something goes.
                .child(view().h(px(28.0)))
                .child(ui::card(vec![action("log-out", t!("Leave this computer"), Act::Lose, live::close)])),
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
    action_saying(icon, title, String::new(), act, press)
}

/// An action's row with what it works on under its words, when `said`.
fn action_saying(icon: &'static str, title: &'static str, said: String, act: Act, press: impl Fn() + 'static) -> View {
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
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(text(title).text_size(theme::ROW).text_color(ink).number_of_lines(1))
                .children((!said.is_empty()).then(|| text(said).text_size(theme::CAPTION).text_color(theme::MUTED).number_of_lines(1))),
        )
        .children((act == Act::Go).then(|| ui::glyph("chevron-right", 16.0, theme::DIM)))
}

/// The phone's key, short: its kind and its end (`ssh-ed25519 …/v/Nlz`);
/// words that aren't a key (why it couldn't be loaded) as they are.
fn short_key(public: &str) -> String {
    let mut parts = public.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(kind), Some(key)) if kind.starts_with("ssh-") && key.len() > 8 => format!("{kind} …{}", &key[key.len() - 8..]),
        _ => public.to_owned(),
    }
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

/// A setting's row: its glyph, what it is, what it is now (when it says
/// something), and its controls after.
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
                .child(text(title).text_size(theme::ROW).text_color(theme::FOREGROUND))
                .child(dynamic(move || {
                    let now = now();
                    if now.is_empty() { view().into_element() } else { text(now).text_size(theme::CAPTION).text_color(theme::MUTED).into_element() }
                })),
        )
}
