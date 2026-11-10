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
use herdc_ssh::{Credential, Greeted};
use nsh_base::input;
use nsh_core::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub fn computers_page() -> impl IntoElement {
    let known = signal(link::computers());
    let adding = signal(known.with(Vec::is_empty));
    view()
        .flex_1()
        .bg(theme::CANVAS)
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
        .child(
            ui::top_bar()
                .child(view().w(px(12.0)))
                .child(text("Yonder").text_size(px(20.0)).font_weight(FontWeight::BOLD).text_color(theme::FOREGROUND).flex_1())
                .child(when(move || !adding.get(), move || ui::icon_button("+", t!("Add a computer"), move || adding.set(true)))),
        )
        .child(when(move || adding.get(), move || add_form(adding, known)).otherwise(move || computer_list(known)))
}

fn computer_list(known: Signal<Vec<Computer>>) -> impl IntoElement {
    scroll().flex_1().child(
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
                        .child(computer_row(c, status, then, known))
                        .children(spaces.into_iter().map(move |s| {
                            let id = s.id.clone();
                            seen_space(&s).on_press(move || live::open_at(c.get_untracked(), id.clone()))
                        }))
                },
            ))
            .child(
                text(t!("Long-press a computer to forget it."))
                    .px(px(16.0))
                    .pt(px(12.0))
                    .text_size(px(13.0))
                    .text_color(theme::DIM),
            ),
    )
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A computer: its name and address, how its agents were when last seen.
fn computer_row(c: ReadSignal<Computer>, status: Option<&'static str>, then: Option<String>, known: Signal<Vec<Computer>>) -> View {
    ui::row(move || c.with(|c| c.name.clone()), move || c.with(|c| then.as_ref().map_or_else(|| c.id(), |then| format!("{} · {then}", c.id()))))
        .child(ui::dot(move || status))
        .on_press(move || live::open(c.get_untracked(), None))
        .on_long_press(move || {
            let computer = c.get_untracked();
            spawn(async move {
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

fn add_form(adding: Signal<bool>, known: Signal<Vec<Computer>>) -> impl IntoElement {
    let (host, port, user, password) = (signal(String::new()), signal("22".to_owned()), signal(String::new()), signal(String::new()));
    let session = signal(String::new());
    let step = signal(Step::Form);
    let greeted: Rc<RefCell<Option<Greeted>>> = Rc::default();
    let connect = move || {
        // One try at a time: pressed again meanwhile, nothing more.
        if matches!(step.get_untracked(), Step::Busy(_)) {
            return;
        }
        let session = Some(session.get_untracked().trim().to_owned()).filter(|s| !s.is_empty());
        let host_name = host.get_untracked().trim().to_owned();
        let computer = Computer {
            name: session.as_ref().map_or_else(|| host_name.clone(), |s| format!("{host_name} · {s}")),
            host: host.get_untracked().trim().to_owned(),
            port: port.get_untracked().trim().parse().unwrap_or(22),
            user: user.get_untracked().trim().to_owned(),
            fingerprint: None,
            password: None,
            windows: false,
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
        let greeted = greeted.clone();
        // Busy at once: a press before the task runs is the same try.
        step.set(Step::Busy(t!("Reaching the computer…")));
        spawn(async move {
            let target = computer.target();
            let hello = background(async move { herdc_ssh::greet(&target, None).await }).await;
            let hello = match hello {
                Ok(Ok(g)) => g,
                Ok(Err(e)) => return step.set(Step::Failed(Lost::of(&e.to_string()).why(&computer.host))),
                Err(e) => return step.set(Step::Failed(e.to_string())),
            };
            let key = hello.host_key.clone();
            *greeted.borrow_mut() = Some(hello);
            let message = t!(
                "{}\n{}\n\nCompare it with what the computer shows for\nssh-keygen -lf /etc/ssh/ssh_host_*_key.pub",
                key.algorithm, key.fingerprint
            );
            if !ui::confirm(&t!("Trust {}?", computer.host), &message, t!("Trust")).await {
                greeted.borrow_mut().take();
                return step.set(Step::Form);
            }
            let Some(hello) = greeted.borrow_mut().take() else { return };
            step.set(Step::Busy(t!("Logging in…")));
            let mut computer = Computer { fingerprint: Some(key.fingerprint.clone()), ..computer };
            let (user, pipe) = (computer.user.clone(), computer.pipe());
            let made = background(async move {
                let session = hello.login(&user, &Credential::Password(secret.clone())).await.map_err(|e| e.to_string())?;
                // Herdr answers before the computer is remembered.
                drop(session.connect_core(&pipe).await.map_err(|e| e.to_string())?);
                Ok::<_, String>((session, Some(secret)))
            })
            .await;
            match made {
                Ok(Ok((session, kept))) => {
                    computer.password = kept;
                    known.set(link::computers());
                    adding.set(false);
                    live::open(computer, Some(session));
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
            .child(ui::field(t!("Computer"), host, KeyboardType::Url).label(t!("Computer")))
            .child(
                view()
                    .flex_row()
                    .gap(px(12.0))
                    .child(ui::field(t!("User"), user, KeyboardType::Url).flex_1())
                    .child(ui::field(t!("Port"), port, KeyboardType::Number).w(px(88.0))),
            )
            .child(password_field(password))
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
            .child(ui::button(t!("Connect"), connect.clone()).disabled(move || matches!(step.get(), Step::Busy(_))).disabled_style(|s| s.opacity(0.4)))
            .child(when(move || !known.with(Vec::is_empty), move || ui::ghost_button(t!("Cancel"), move || adding.set(false)))),
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
