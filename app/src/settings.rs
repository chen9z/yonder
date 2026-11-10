//! Settings: the terminal's size and fit, the app's language, this
//! phone's key, and the computer it is on.

use crate::i18n::Language;
use crate::live::{self, Live};
use crate::prefs::Prefs;
use crate::nav::{self, Route};
use crate::{link, theme, ui};
use herdc_engine::Link;
use nsh_base::switch;
use nsh_core::prelude::*;

pub fn settings_page(live: Live) -> nsh_core::element::Screen {
    let prefs = use_context::<Prefs>().expect("prefs");
    let world = live.view;
    let (_, public) = link::phone_key();
    let about = live.clone();
    let core = computed(move || {
        world.with(|v| match &v.link {
            Link::Connected { core, version } => format!("{core} · herdr-core {version}"),
            Link::Connecting => t!("Connecting…").into(),
            Link::Lost { error } => link::Lost::of(error).said(&live.computer.host),
        })
    });
    ui::page(t!("Settings")).child(
        scroll().flex_1().bg(theme::CANVAS).child(
            view()
                .pb(move || px(safe_area().bottom.0 + 24.0))
                .child(ui::heading(|| t!("TERMINAL").into()))
                .child(
                    setting(|| t!("Font size").into(), move || t!("{} pt", prefs.font.get()))
                        .child(ui::ghost_button("−", move || prefs.font.update(|f| *f = (*f - 1.0).max(crate::terminal::FONT.0))).label(t!("Smaller text")))
                        .child(ui::ghost_button("+", move || prefs.font.update(|f| *f = (*f + 1.0).min(crate::terminal::FONT.1))).label(t!("Larger text"))),
                )
                .child(
                    setting(|| t!("Fit terminals to the phone").into(), || t!("Else they keep the desktop's size, drawn smaller").into())
                        .child(switch().checked(prefs.fit).on_change(move |on: &bool| prefs.fit.set(*on))),
                )
                .child(
                    setting(|| t!("Key bar").into(), move || t!("{} keys", prefs.keys.with(Vec::len)))
                        .role(AccessibilityRole::Button)
                        .on_press(|| nav::push(Route::KeyBar)),
                )
                .child(
                    setting(|| t!("Language").into(), move || match prefs.language.get() {
                        Language::System => t!("System").into(),
                        Language::English => "English".into(),
                        Language::Chinese => "中文".into(),
                    })
                    .role(AccessibilityRole::Button)
                    .on_press(move || {
                        spawn(async move {
                            let picked = ui::choose(t!("Language"), &[("system", t!("System")), ("en", "English"), ("zh", "中文")]).await;
                            match picked.as_deref() {
                                Some("system") => prefs.language.set(Language::System),
                                Some("en") => prefs.language.set(Language::English),
                                Some("zh") => prefs.language.set(Language::Chinese),
                                _ => {}
                            }
                        });
                    }),
                )
                .child(ui::heading(|| t!("COMPUTER").into()))
                .child(setting(move || live.computer.name.clone(), move || core.get()))
                .child(view().px(px(16.0)).pt(px(8.0)).child(ui::ghost_button(t!("Leave this computer"), live::close)))
                .child(ui::heading(|| t!("THIS PHONE'S KEY").into()))
                .child(
                    view().px(px(16.0)).gap(px(8.0)).child(
                        text(public.clone())
                            .font_family(theme::MONO)
                            .text_size(px(12.0))
                            .text_color(theme::MUTED)
                            .selectable(true),
                    ),
                )
                .child(view().px(px(16.0)).pt(px(8.0)).child(ui::ghost_button(t!("Copy"), move || set_clipboard(public.clone()))))
                .child(ui::heading(|| t!("ABOUT").into()))
                .child(setting(|| "Yonder".into(), version))
                .child(view().px(px(16.0)).pt(px(8.0)).child(ui::ghost_button(t!("Copy diagnostics"), move || {
                    set_clipboard(diagnostics(&about, prefs));
                    about.say(herdc_engine::Note::info(t!("Copied"), t!("Paste it where you tell what went wrong.")));
                }))),
        ),
    )
}

/// The app's version and build (the commits it was built at, `apk.sh`).
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

/// A setting's row: what it is, what it is now, and its controls after.
fn setting(title: impl Fn() -> String + 'static, now: impl Fn() -> String + 'static) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(10.0))
        .min_h(px(56.0))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(text(title).text_size(px(15.0)).text_color(theme::FOREGROUND))
                .child(text(now).text_size(px(12.0)).text_color(theme::MUTED)),
        )
}
