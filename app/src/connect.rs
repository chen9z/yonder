//! The way in: the computers this phone knows, and adding one. Adding asks
//! the computer for its key first and shows it to compare, as `ssh` does
//! the first time; only then does the password travel. It is kept on the
//! phone, sealed: nothing is left on the computer.
//! Each computer lists the spaces it showed last, with how they were then:
//! a tap on one opens the computer there.

use crate::link::{self, Computer, Lost};
use crate::{live, theme, ui};
use herdc_engine::tools::worktree::ago;
use herdc_engine::SpaceView;
use herdc_model::model::priority;
use herdc_ssh::Credential;
use nsh_base::input;
use nsh_core::prelude::*;

pub fn computers_page() -> impl IntoElement {
    let known = signal(link::computers());
    let adding = signal(known.with(Vec::is_empty));
    let original = stored(None::<Computer>);
    let edit = move |computer: Computer| {
        original.replace(Some(computer));
        adding.set(true);
    };
    view()
        .flex_1()
        .bg(theme::CANVAS)
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
        .child(
            ui::top_bar()
                .child(view().w(px(12.0)))
                .child(text("Yonder").text_size(px(20.0)).font_weight(FontWeight::BOLD).text_color(theme::FOREGROUND).flex_1())
                .child(when(move || !adding.get(), move || ui::icon_button("+", t!("Add a computer"), move || { original.replace(None); adding.set(true); }))),
        )
        .child(when(move || adding.get(), move || {
            let old = original.replace(None);
            let previous = old.as_ref().map(Computer::id);
            connection_form(old, !known.with_untracked(Vec::is_empty), move || adding.set(false), move |computer, session| {
                if let Some(id) = &previous {
                    link::replace(id, &computer)?;
                    known.set(link::computers());
                    adding.set(false);
                } else {
                    live::open(computer, Some(session));
                }
                Ok(())
            })
        }).otherwise(move || computer_list(known, edit)))
}

fn computer_list(known: Signal<Vec<Computer>>, edit: impl Fn(Computer) + Copy + 'static) -> impl IntoElement {
    let editing = stored(None::<Computer>);
    let asking = signal(None::<ui::Prompt>);
    view().flex_1().child(scroll().flex_1().child(
        view()
            .pb(move || px(safe_area().bottom.0 + 16.0))
            .child(ui::heading(|| t!("COMPUTERS").to_owned()))
            .child(each(
                move || known.get(),
                |c: &Computer| c.id(),
                move |c, _| {
                    let seen = link::seen(&c.with_untracked(Computer::id));
                    let spaces = seen.as_ref().map(|s| s.view.spaces.clone()).unwrap_or_default();
                    // The most urgent of its spaces, as they were.
                    let status = spaces.iter().map(|s| s.status).max_by_key(|s| priority(*s)).flatten();
                    let then = seen.map(|s| t!("{} ago", ago(now() - s.at as i64)));
                    view()
                        .child(computer_row(c, status, then, known, editing, asking, edit))
                        .children(spaces.into_iter().map(move |s| {
                            let id = s.id.clone();
                            seen_space(&s).on_press(move || live::open_at(c.get_untracked(), id.clone()))
                        }))
                },
            ))
            .child(
                text(t!("Long-press a computer to edit, rename or forget it."))
                    .px(px(16.0))
                    .pt(px(12.0))
                    .text_size(px(13.0))
                    .text_color(theme::DIM),
            ),
    )).child(ui::prompt_dialog(asking, move |answer| {
        if let (Some(mut computer), Some(name)) = (editing.replace(None), answer)
            && !name.trim().is_empty()
        {
            computer.name = name.trim().to_owned();
            match link::update(&computer) {
                Ok(()) => known.set(link::computers()),
                Err(e) => link::storage_error(e),
            }
        }
    }))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A computer: its name and address, how its agents were when last seen.
fn computer_row(c: ReadSignal<Computer>, status: Option<&'static str>, then: Option<String>, known: Signal<Vec<Computer>>, editing: Stored<Option<Computer>>, asking: Signal<Option<ui::Prompt>>, edit: impl Fn(Computer) + Copy + 'static) -> View {
    ui::row(move || c.with(|c| c.name.clone()), move || then.clone().unwrap_or_default())
        .child(ui::dot(move || status))
        .on_press(move || live::open(c.get_untracked(), None))
        .label(move || c.with(|c| c.name.clone()))
        .on_long_press(move || {
            let computer = c.get_untracked();
            spawn(async move {
                match ui::choose(&computer.name, &[("edit", t!("Edit connection")), ("rename", t!("Rename")), ("forget", t!("Forget"))]).await.as_deref() {
                    Some("edit") => return edit(computer),
                    Some("rename") => {
                        let prompt = ui::name_prompt(computer.name.clone());
                        editing.replace(Some(computer));
                        asking.set(Some(prompt));
                        return;
                    }
                    Some("forget") => {}
                    _ => return,
                }
                let title = t!("Forget {}?", computer.name);
                if ui::confirm(&title, t!("Its key stays trusted nowhere else; adding it again asks for the password."), t!("Forget")).await {
                    link::forget(&computer.id());
                    known.set(link::computers());
                }
            });
        })
}

/// A space a computer showed last, under it.
fn seen_space(s: &SpaceView) -> View {
    let status = s.status;
    let tabs = s.tabs.iter().filter(|t| !t.settled).count();
    let detail = format!("{}{}", if tabs == 1 { t!("1 tab").to_owned() } else { t!("{} tabs", tabs) }, s.branch.as_ref().map(|b| format!(" · {b}")).unwrap_or_default());
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .pl(px(32.0))
        .pr(px(16.0))
        .py(px(6.0))
        .active(|st| st.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(s.name.clone())
        .child(ui::dot(move || status))
        .child(text(s.name.clone()).text_size(px(14.0)).text_color(theme::FOREGROUND).number_of_lines(1).flex_shrink(1.0))
        .child(text(detail).text_size(px(12.0)).text_color(theme::MUTED).number_of_lines(1))
}

/// What adding is doing now.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Form,
    Busy(&'static str),
    Failed(String),
}

pub fn edit_current_page(live: live::Live) -> nsh_core::element::Screen {
    let id = live.computer.id();
    let original = link::computers().into_iter().find(|c| c.id() == id).unwrap_or(live.computer);
    ui::page(t!("Edit connection")).child(connection_form(Some(original), true, crate::nav::back, move |computer, session| {
        live::switch_connection(computer, Some(session), Some(&id))
    }))
}

fn connection_form(
    original: Option<Computer>,
    can_cancel: bool,
    cancel: impl Fn() + Copy + 'static,
    save: impl Fn(Computer, herdc_ssh::Session) -> Result<(), String> + Clone + 'static,
) -> impl IntoElement {
    let host = signal(original.as_ref().map(|c| c.host.clone()).unwrap_or_default());
    let user = signal(original.as_ref().map(|c| c.user.clone()).unwrap_or_default());
    let port = signal(original.as_ref().map_or(22, |c| c.port).to_string());
    let name = signal(original.as_ref().map(|c| c.name.clone()).unwrap_or_default());
    let session = signal(original.as_ref().and_then(|c| c.session.clone()).unwrap_or_default());
    let password = signal(String::new());
    let is_edit = original.is_some();
    let step = signal(Step::Form);
    let connect = move || {
        // One try at a time: pressed again meanwhile, nothing more.
        if matches!(step.get_untracked(), Step::Busy(_)) {
            return;
        }
        let session = Some(session.get_untracked().trim().to_owned()).filter(|s| !s.is_empty());
        let host_name = host.get_untracked().trim().to_owned();
        let Some(port) = port.get_untracked().trim().parse::<u16>().ok().filter(|&p| p != 0) else {
            step.set(Step::Failed(t!("Enter a port from 1 to 65535.").into()));
            return;
        };
        let computer = Computer {
            name: name.get_untracked().trim().to_owned(),
            host: host_name,
            port,
            user: user.get_untracked().trim().to_owned(),
            fingerprint: original.as_ref().and_then(|c| c.fingerprint.clone()),
            password: None,
            windows: original.as_ref().is_some_and(|c| c.windows),
            session,
        };
        let secret = password.get_untracked();
        if computer.host.is_empty() || computer.user.is_empty() {
            step.set(Step::Failed(t!("Enter the computer and your user name on it.").into()));
            return;
        }
        // It goes into the computer's shell as it is.
        if !computer.session.as_deref().unwrap_or_default().chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c)) {
            step.set(Step::Failed(t!("A session is letters, digits and . _ - / only.").into()));
            return;
        }
        let original = original.clone();
        let save = save.clone();
        // Busy at once: a press before the task runs is the same try.
        step.set(Step::Busy(t!("Reaching the computer…")));
        spawn(async move {
            let target = computer.target();
            let trusted = computer.fingerprint.clone();
            let hello = background(async move { herdc_ssh::greet(&target, trusted.as_deref()).await }).await;
            let hello = match hello {
                Ok(Ok(g)) => g,
                Ok(Err(e)) => return step.set(Step::Failed(Lost::of(&e.to_string()).why(&computer.host))),
                Err(e) => return step.set(Step::Failed(e.to_string())),
            };
            let key = hello.host_key.clone();
            let message = t!(
                "{}\n{}\n\nCompare it with what the computer shows for\nssh-keygen -lf /etc/ssh/ssh_host_*_key.pub",
                key.algorithm, key.fingerprint
            );
            if computer.fingerprint.is_none() && !ui::confirm(&t!("Trust {}?", computer.host), &message, t!("Trust")).await {
                return step.set(Step::Form);
            }
            step.set(Step::Busy(t!("Logging in…")));
            let mut computer = Computer { fingerprint: Some(key.fingerprint.clone()), ..computer };
            let (user, pipe) = (computer.user.clone(), computer.pipe());
            computer.password = if is_edit && secret.is_empty() {
                original.as_ref().and_then(|c| c.password.clone())
            } else { Some(secret) };
            let credential = match &computer.password {
                Some(p) => Credential::Password(p.clone()),
                None => match link::phone_key() {
                    Ok((openssh, _)) => Credential::Key { openssh, passphrase: None },
                    Err(e) => return step.set(Step::Failed(e)),
                },
            };
            let made = background(async move {
                let session = hello.login(&user, &credential).await.map_err(|e| e.to_string())?;
                // Herdr answers before the computer is remembered.
                let core = session.connect_core(&pipe).await.map_err(|e| e.to_string())?;
                let reported = core.hello().pointer("/core/name").and_then(|v| v.as_str()).map(str::to_owned);
                Ok::<_, String>((session, reported))
            })
            .await;
            match made {
                Ok(Ok((session, reported))) => {
                    if computer.name.is_empty() {
                        let name = reported.as_deref().and_then(link::host_label).or_else(|| link::host_label(&computer.host)).unwrap_or(t!("Computer"));
                        computer.name = computer.session.as_ref().map_or_else(|| name.to_owned(), |s| format!("{name} · {s}"));
                    }
                    if let Err(e) = save(computer, session) {
                        step.set(Step::Failed(e));
                    }
                }
                Ok(Err(e)) => step.set(Step::Failed(crate::i18n::tr_string(&e))),
                Err(e) => step.set(Step::Failed(e.to_string())),
            }
        });
    };
    scroll().flex_1().keyboard_taps(KeyboardTaps::Handled).child(
        view()
            .p(px(16.0))
            .gap(px(16.0))
            .pb(move || px(safe_area().bottom.0 + 24.0))
            .child(
                text(t!("Reach the computer running Herdr over SSH: its name or address on this network, or its Tailscale name."))
                    .text_size(px(14.0))
                    .text_color(theme::MUTED),
            )
            .child(ui::field(t!("Display name"), name, KeyboardType::Default))
            .child(ui::field(t!("Computer"), host, KeyboardType::Url).label(t!("Computer")))
            .child(
                view()
                    .flex_row()
                    .gap(px(12.0))
                    .child(ui::field(t!("User"), user, KeyboardType::Url).flex_1())
                    .child(ui::field(t!("Port"), port, KeyboardType::Number).w(px(88.0))),
            )
            .child(password_field(password))
            .child(when(move || is_edit, || text(t!("Leave blank to keep the current login.")).text_size(px(12.0)).text_color(theme::DIM)))
            .child(
                view()
                    .gap(px(4.0))
                    .child(ui::field(t!("Session"), session, KeyboardType::Url))
                    .child(text(t!("Optional: another herdr's HC_SESSION on that computer.")).text_size(px(12.0)).text_color(theme::DIM)),
            )
            .child(dynamic(move || match step.get() {
                Step::Failed(why) => text(why).text_size(px(14.0)).text_color(theme::BLOCKED).into_element(),
                Step::Busy(what) => text(what).text_size(px(14.0)).text_color(theme::MUTED).into_element(),
                Step::Form => view().into_element(),
            }))
            // Pressed, it waits (dimmed) for the try to end.
            .child(ui::button(if is_edit { t!("Test and save") } else { t!("Connect") }, connect.clone()).disabled(move || matches!(step.get(), Step::Busy(_))).disabled_style(|s| s.opacity(0.4)))
            .child(when(move || can_cancel, move || ui::ghost_button(t!("Cancel"), cancel).disabled(move || matches!(step.get(), Step::Busy(_))))),
    )
}

/// The password, hidden unless asked: a keyboard that changed a letter
/// shows here.
fn password_field(password: Signal<String>) -> impl IntoElement {
    let shown = signal(false);
    view()
        .gap(px(6.0))
        .child(text(t!("Password")).text_size(px(13.0)).text_color(theme::MUTED))
        .child(
            view()
                .flex_row()
                .items_center()
                .rounded(px(10.0))
                .border_1()
                .border_color(theme::INPUT)
                .bg(theme::BACKGROUND)
                .child(
                    input()
                        .masked(move || !shown.get())
                        .test_id("password")
                        .value(move || password.get())
                        .on_change(move |t: &str| password.set(t.to_owned()))
                        .auto_fill(nsh_core::element::AutoFill::Password)
                        .auto_correct(false)
                        .auto_capitalize(nsh_core::element::AutoCapitalize::None)
                        .label(t!("Password"))
                        .flex_1()
                        .px(px(12.0))
                        .h(px(46.0))
                        .text_size(px(16.0))
                        .text_color(theme::FOREGROUND),
                )
                .child(
                    view()
                        .h(px(46.0))
                        .px(px(14.0))
                        .justify_center()
                        .role(AccessibilityRole::Button)
                        .label(t!("Show the password"))
                        .active(|s| s.bg(theme::HOVER))
                        .on_press(move || shown.update(|s| *s = !*s))
                        .child(text(move || if shown.get() { t!("Hide") } else { t!("Show") }).text_size(px(14.0)).text_color(theme::MUTED)),
                ),
        )
        .child(
            text(t!("Kept on this phone, sealed: nothing is left on the computer."))
                .text_size(px(12.0))
                .text_color(theme::DIM),
        )
}
