//! Add a machine: the hosts in the computer's `~/.ssh/config` to pick
//! from (or one typed), each added with a tap (`machine.add` on the core,
//! dialing from the computer). A machine that needs answers first (a
//! password, a new host key, herdr-core installed there) says what to run
//! in a terminal, and shows as added once that is done. As the desktop's
//! machine-add mod.

use crate::live::{off, Live};
use crate::{theme, ui};
use herdc_engine::tools::machines::{self, Added, Host};
use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// Where one target is.
#[derive(Clone, Debug, PartialEq)]
enum Doing {
    Adding,
    /// Waiting for the person to answer what it asks, in a terminal.
    Waiting(String),
    Failed(String),
}

#[derive(Clone, Copy)]
struct Page {
    hosts: Signal<Option<Vec<Host>>>,
    added: Signal<Vec<Value>>,
    doing: Signal<BTreeMap<String, Doing>>,
    /// Added Macs: their name and where their herdr-core is.
    macs: Signal<BTreeMap<String, (String, String)>>,
}

impl Page {
    fn is_added(&self, target: &str) -> bool {
        self.added.with(|a| machines::is_added(a, target))
    }
}

pub fn add_machine_page(live: Live) -> nsh_core::element::Screen {
    let page = Page { hosts: signal(None), added: signal(Vec::new()), doing: signal(BTreeMap::new()), macs: signal(BTreeMap::new()) };
    let typed = signal(String::new());
    {
        let live = live.clone();
        spawn(async move {
            let Ok(there) = live.there(None) else { return page.hosts.set(Some(Vec::new())) };
            let home = live.view.with_untracked(|v| v.home.clone());
            let read = off(async move {
                let hosts = machines::hosts(&there, &home).await;
                (hosts, machines::added(&there).await)
            });
            let (hosts, added) = read.await.unwrap_or_default();
            page.added.set(added);
            page.hosts.set(Some(hosts));
            // While some wait for answers in a terminal, look again every 3 s.
            loop {
                off(tokio::time::sleep(Duration::from_secs(3))).await;
                let waiting: Vec<String> = page.doing.with(|d| d.iter().filter(|(_, s)| matches!(s, Doing::Waiting(_))).map(|(t, _)| t.clone()).collect());
                if waiting.is_empty() {
                    continue;
                }
                refresh(&live, page).await;
                for target in waiting.into_iter().filter(|t| page.is_added(t)) {
                    page.doing.update(|d| { d.remove(&target); });
                    if typed.get_untracked().trim() == target {
                        typed.set(String::new());
                    }
                    check_mac(&live, page, target).await;
                }
            }
        });
    }
    let add_typed = {
        let live = live.clone();
        move || {
            let target = typed.get_untracked().trim().to_owned();
            add(live.clone(), page, target, Some(typed));
        }
    };
    let listed = computed(move || page.hosts.get().unwrap_or_default().into_iter().filter(|h| !page.is_added(&h.alias)).collect::<Vec<_>>());
    let prefs = use_context::<crate::prefs::Prefs>().unwrap();
    ui::page(t!("Add a machine")).child(
        scroll().flex_1().bg(theme::CANVAS).keyboard_dismiss_mode(KeyboardDismissMode::Interactive).child(
            view()
                .pb(move || ui::bottom_inset() + px(24.0))
                .child(
                    text(t!("Its terminals and spaces show here beside this computer's. The computer dials it over ssh, so it is reached as the computer reaches it."))
                        .px(px(16.0))
                        .pt(px(16.0))
                        .pb(px(8.0))
                        .text_size(px(14.0))
                        .text_color(theme::MUTED),
                )
                .child(each(move || page.added.with(|all| all.iter().filter_map(|m| {
                    let name = m["name"].as_str()?;
                    Some((name.to_owned(), m["target"].as_str().unwrap_or(name).to_owned()))
                }).collect::<Vec<_>>()), |row: &(String, String)| row.0.clone(), {
                    let live = live.clone();
                    move |row, _| {
                        let (name, target): (String, String) = row.get_untracked();
                        let id = live.computer.id();
                        let shown = name.clone();
                        let live = live.clone();
                        let details = live.clone();
                        // A tap renames it, as the config's hosts say Add.
                        let called = {
                            let (id, shown) = (id.clone(), shown.clone());
                            move || prefs.machine_name(&id, &shown)
                        };
                        view().child(ui::row(called.clone(), || t!("Added").into())
                            .role(AccessibilityRole::Button)
                            .label(move || t!("Rename {}", called()))
                            .on_press(move || {
                                let (live, name) = (live.clone(), name.clone());
                                spawn(async move { prefs.rename_machine(&live, &name).await; });
                            })
                            .child(text(t!("Rename")).text_size(px(14.0)).text_color(theme::ACCENT))).child(said(details, page, move || target.clone()))
                    }
                }))
                .child(ui::heading(move || match page.hosts.with(|h| h.as_ref().map(Vec::len)) {
                    None => t!("READING ~/.SSH/CONFIG…").into(),
                    Some(0) => t!("YOUR SSH CONFIG LISTS NO HOSTS").into(),
                    Some(_) => t!("FROM YOUR SSH CONFIG").into(),
                }))
                .child(each(move || listed.get(), |h: &Host| h.alias.clone(), {
                    let live = live.clone();
                    move |h, _| host_row(live.clone(), page, h.get_untracked())
                }))
                .child(ui::heading(|| t!("OR TYPE HOW SSH REACHES IT").into()))
                .child(
                    view()
                        .flex_row()
                        .gap(px(10.0))
                        .px(px(16.0))
                        .child(
                            input()
                                .test_id("machine-target")
                                .flex_1()
                                .h(px(46.0))
                                .px(px(12.0))
                                .rounded(px(10.0))
                                .border_1()
                                .border_color(theme::INPUT)
                                .bg(theme::BACKGROUND)
                                .text_size(px(16.0))
                                .text_color(theme::FOREGROUND)
                                .placeholder("user@host")
                                .placeholder_color(move || theme::now(theme::DIM))
                                .keyboard_type(KeyboardType::Url)
                                .auto_capitalize(AutoCapitalize::None)
                                .auto_correct(false)
                                .return_key_type(ReturnKeyType::Go)
                                .value(move || typed.get())
                                .on_change(move |t: &str| typed.set(t.to_owned()))
                                .on_submit({
                                    let add_typed = add_typed.clone();
                                    move |_: &str| add_typed()
                                }),
                        )
                        .child(ui::ghost_button(t!("Add"), add_typed).label(t!("Add the machine typed"))),
                )
                .child(said(live.clone(), page, move || typed.get().trim().to_owned())),
        ),
    )
}

/// A host of the config: added, being added, or a tap away.
fn host_row(live: Live, page: Page, host: Host) -> impl IntoElement {
    let target = host.alias.clone();
    let detail = match (&host.user, &host.hostname) {
        _ if host.hostname.is_none() && !host.aliases.is_empty() => t!("also {}", host.aliases.join(" ")),
        (Some(user), Some(name)) => format!("{user}@{name}"),
        (None, Some(name)) => name.clone(),
        _ => String::new(),
    };
    let state = {
        let target = target.clone();
        move || {
            if page.is_added(&target) {
                t!("Added")
            } else if page.doing.with(|d| d.get(&target) == Some(&Doing::Adding)) {
                t!("Connecting…")
            } else {
                t!("Add")
            }
        }
    };
    let press = {
        let (live, target) = (live.clone(), target.clone());
        move || {
            if !page.is_added(&target) {
                add(live.clone(), page, target.clone(), None);
            }
        }
    };
    let shown = target.clone();
    view()
        .child(
            ui::row(move || shown.clone(), move || detail.clone())
                .role(AccessibilityRole::Button)
                .label(t!("Add {}", target))
                .on_press(press)
                .child({
                    let state = state.clone();
                    text(state.clone()).text_size(px(14.0)).text_color(move || {
                        if state() == t!("Add") { theme::now(theme::ACCENT) } else { theme::now(theme::MUTED) }
                    })
                }),
        )
        .child(said(live, page, move || target.clone()))
}

/// What a target came to: why it failed, what it waits for, or, for an
/// added Mac, the setting it needs once.
fn said(live: Live, page: Page, target: impl Fn() -> String + Clone + 'static) -> impl IntoElement {
    let words = {
        let target = target.clone();
        move || match page.doing.with(|d| d.get(&target()).cloned()) {
            Some(Doing::Waiting(w) | Doing::Failed(w)) => Some(w),
            _ => None,
        }
    };
    let mac = {
        let target = target.clone();
        move || page.macs.with(|m| m.get(&target()).cloned())
    };
    view()
        .child(when({ let words = words.clone(); move || words().is_some() }, move || {
            text({ let words = words.clone(); move || words().unwrap_or_default() }).mx(px(16.0)).mb(px(8.0)).text_size(px(13.0)).text_color(theme::MUTED)
        }))
        .child(when({ let mac = mac.clone(); move || mac().is_some() }, move || {
            let (name, core) = mac().unwrap_or_default();
            let live = live.clone();
            let open = name.clone();
            view()
                .mx(px(16.0))
                .mb(px(8.0))
                .p(px(12.0))
                .gap(px(10.0))
                .rounded(px(12.0))
                .bg(theme::CHROME)
                .child(text(t!("{} is a Mac: allow its folders once", name)).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND))
                .child(
                    text(t!("macOS asks on its own screen before anything reads Desktop, Documents or Downloads, and waits. Give {} Full Disk Access there.", core))
                        .text_size(px(13.0))
                        .text_color(theme::MUTED),
                )
                .child(ui::ghost_button(t!("Open the setting there"), move || {
                    let (live, name) = (live.clone(), open.clone());
                    spawn(async move {
                        let Ok(there) = live.there(None) else { return };
                        if let Some(Err(e)) = off(async move { machines::open_mac_setting(&there, &name).await }).await {
                            live.say(herdc_engine::Note::error("Open the setting", e));
                        }
                    });
                }))
        }))
}

fn add(live: Live, page: Page, target: String, typed: Option<Signal<String>>) {
    if target.is_empty() || page.doing.with(|d| d.get(&target) == Some(&Doing::Adding)) {
        return;
    }
    page.doing.update(|d| { d.insert(target.clone(), Doing::Adding); });
    spawn(async move {
        let Ok(there) = live.there(None) else {
            return page.doing.update(|d| { d.insert(target, Doing::Failed(t!("Not connected").into())); });
        };
        let added = {
            let target = target.clone();
            off(async move { machines::add(&there, &target).await }).await
        };
        match added {
            Some(Added::Yes) => {
                page.doing.update(|d| { d.remove(&target); });
                if let Some(typed) = typed {
                    typed.set(String::new());
                }
                refresh(&live, page).await;
                check_mac(&live, page, target).await;
            }
            Some(Added::NeedsYou(what)) => page.doing.update(|d| { d.insert(target, Doing::Waiting(what)); }),
            Some(Added::No(why)) => page.doing.update(|d| { d.insert(target, Doing::Failed(why)); }),
            None => {}
        }
    });
}

async fn refresh(live: &Live, page: Page) {
    let Ok(there) = live.there(None) else { return };
    if let Some(added) = off(async move { machines::added(&there).await }).await {
        page.added.set(added);
    }
}

async fn check_mac(live: &Live, page: Page, target: String) {
    let Some(name) = page.added.with(|a| a.iter().find(|m| m["target"] == target.as_str() || m["name"] == target.as_str()).and_then(|m| m["name"].as_str().map(str::to_owned))) else {
        return;
    };
    let Ok(there) = live.there(None) else { return };
    let found = {
        let name = name.clone();
        off(async move { machines::mac_core(&there, &name).await }).await.flatten()
    };
    if let Some(core) = found {
        page.macs.update(|m| { m.insert(target, (name, core)); });
    }
}
