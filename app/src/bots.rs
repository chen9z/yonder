//! Bots: the bots Yonder runs on the computer, their state, and starting,
//! stopping the turn, pausing and resuming them. Everything comes from
//! the computer's own `yonder` (`bot ls --json`, `bot status --json`, the
//! actions), run through the core's `machine.exec` on the core's machine
//! in a login shell, so it is found where the person's shell finds it.
//! No reset or rm here: they cannot be undone (the command line has
//! them). What the actions do can be undone, so nothing asks first.

use crate::live::{off, Live};
use crate::nav::{self, Route};
use crate::{theme, ui};
use herdc_engine::tools::There;
use herdc_engine::Link;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;
use std::time::Duration;
use yonder_model::bots::{bot_rows, json_array, BotListed, BotRow, BotStatus};

/// How often the page reads again while it is looked at.
const EVERY: Duration = Duration::from_secs(10);

/// `yonder <args>` in the login shell of the core's machine: `$SHELL`
/// there (else `/bin/sh`) with `-l -i`, as bots' harnesses are run
/// (spec 2.7). The arguments stay arguments of their own.
pub const WRAP: &str = r#"exec "${SHELL:-/bin/sh}" -lic 'exec "$@"' yonder-app "$@""#;

/// Where to keep the last list a computer gave (`bots.<computer id>`).
const KEPT: &str = "bots.";

/// The argv `machine.exec` runs for `yonder <args>`.
pub fn argv(args: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = ["/bin/sh", "-c", WRAP, "yonder-app", "yonder"].map(String::from).into();
    all.extend(args.iter().map(|a| (*a).to_owned()));
    all
}

/// Why `yonder` did not do what was asked.
#[derive(Clone, Debug, PartialEq)]
enum Fail {
    /// No `yonder` there (the login shell's exit 127).
    Missing,
    /// What it said: the first line of its stderr (`code: why; next`).
    Said(String),
    /// The computer could not be asked.
    Away(String),
}

/// What the page shows of one computer's bots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    computer: String,
    rows: Vec<BotRow>,
    /// Read just now from the computer (else kept from before).
    fresh: bool,
    /// Read at least once, or kept from before.
    loaded: bool,
    /// No `yonder` on the computer.
    missing: bool,
    /// Why the list could not be read.
    error: Option<String>,
    /// The bot an action runs on, and which.
    busy: Option<(String, &'static str)>,
    /// The bot whose last action failed, and what yonder said.
    failed: Option<(String, String)>,
}

fn shown() -> Signal<Shown> {
    global::<Shown>()
}

/// The shown list is computer `id`'s: if it was another's, the one kept
/// for `id` from before (not fresh).
fn on(id: &str) {
    if shown().with_untracked(|s| s.computer == id) {
        return;
    }
    let kept: Option<Vec<BotRow>> = Storage::get_item(&format!("{KEPT}{id}")).and_then(|t| serde_json::from_str(&t).ok());
    shown().set(Shown { computer: id.to_owned(), loaded: kept.is_some(), rows: kept.unwrap_or_default(), ..Shown::default() });
}

async fn yonder(there: There, args: Vec<String>, timeout: Duration) -> Result<herdc_engine::Ran, Fail> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let all = argv(&refs);
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let ran = there.run(&all, None, timeout).await.map_err(Fail::Away)?;
    if ran.code == Some(127) {
        return Err(Fail::Missing);
    }
    Ok(ran)
}

/// What a failed run said: the first line of its stderr that is
/// yonder's (`code: why; next`), not the login shell's (one without a
/// terminal says "no job control in this shell" first, as the shell or
/// as `yonder-app`); else stderr's last line, stdout's, how it ended.
fn said(ran: &herdc_engine::Ran) -> String {
    let lines = |b: &[u8]| String::from_utf8_lossy(b).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect::<Vec<_>>();
    let shells = ["yonder-app: ", "sh: ", "bash: ", "zsh: ", "-sh: ", "-bash: ", "-zsh: "];
    let shell = |l: &&String| shells.iter().any(|p| l.starts_with(p)) || l.ends_with("no job control in this shell");
    let err = lines(&ran.stderr);
    err.iter()
        .find(|l| !shell(l))
        .or(err.last())
        .cloned()
        .or_else(|| lines(&ran.stdout).pop())
        .unwrap_or_else(|| match (ran.timed_out, ran.code) {
            (true, _) => "timed out".to_owned(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "ended by a signal".to_owned(),
        })
}

/// The bots there, with their states.
async fn load(there: There) -> Result<Vec<BotRow>, Fail> {
    let words = |args: &[&str]| args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    let ls = yonder(there.clone(), words(&["bot", "ls", "--json"]), Duration::from_secs(30)).await?;
    if !ls.ok() {
        return Err(Fail::Said(said(&ls)));
    }
    let listed: Vec<BotListed> = json_array(&ls.out()).map_err(Fail::Said)?;
    let status = yonder(there, words(&["bot", "status", "--json"]), Duration::from_secs(60)).await?;
    if !status.ok() {
        return Err(Fail::Said(said(&status)));
    }
    let statuses: Vec<BotStatus> = json_array(&status.out()).map_err(Fail::Said)?;
    Ok(bot_rows(&listed, &statuses))
}

/// Reads the list again; the computer away, what is shown is not fresh.
async fn refresh(live: Live) {
    let id = live.computer.id();
    on(&id);
    let read = match live.there(None) {
        Ok(there) => off(load(there)).await,
        Err(e) => Some(Err(Fail::Away(e))),
    };
    let Some(read) = read else { return };
    if shown().with_untracked(|s| s.computer != id) {
        return;
    }
    shown().update(|s| match read {
        Ok(rows) => {
            if let Ok(text) = serde_json::to_string(&rows) {
                let _ = Storage::set_item(format!("{KEPT}{id}"), text);
            }
            s.rows = rows;
            (s.fresh, s.loaded, s.missing, s.error) = (true, true, false, None);
        }
        Err(Fail::Missing) => (s.fresh, s.missing, s.error) = (false, true, None),
        Err(Fail::Said(line)) => (s.fresh, s.missing, s.error) = (false, false, Some(line)),
        Err(Fail::Away(_)) => s.fresh = false,
    });
}

/// Runs `yonder bot <verb> <name>`, once at a time a bot; then reads the
/// list again. A failure is said where the buttons are.
fn act(live: &Live, verb: &'static str, name: &str) {
    if shown().with_untracked(|s| s.busy.is_some()) {
        return;
    }
    let there = match live.there(None) {
        Ok(there) => there,
        Err(e) => return shown().update(|s| s.failed = Some((name.to_owned(), crate::i18n::tr_string(&e)))),
    };
    shown().update(|s| {
        s.busy = Some((name.to_owned(), verb));
        s.failed = None;
    });
    let (live, name) = (live.clone(), name.to_owned());
    // A start waits for the bot to be ready (90 s by default).
    let timeout = Duration::from_secs(if verb == "start" || verb == "resume" { 240 } else { 120 });
    live.clone().spawn(async move {
        let args = vec!["bot".to_owned(), verb.to_owned(), name.clone()];
        let ran = off(yonder(there, args, timeout)).await;
        shown().update(|s| {
            s.busy = None;
            match ran {
                Some(Ok(ran)) if ran.ok() => {}
                Some(Ok(ran)) => s.failed = Some((name.clone(), said(&ran))),
                Some(Err(Fail::Missing)) => s.missing = true,
                Some(Err(Fail::Said(line) | Fail::Away(line))) => s.failed = Some((name.clone(), line)),
                None => {}
            }
        });
        refresh(live).await;
    });
}

/// A state in words, and how it is marked: a shape and a color each, so
/// they differ without color too. Paused is the owner's choice: neutral.
fn state_word(state: &str) -> &'static str {
    match state {
        "stopped" => t!("Not running"),
        "starting" => t!("Starting"),
        "ready" => t!("Ready"),
        "failed" => t!("Failed"),
        "gone" => t!("Gone"),
        "paused" => t!("Paused"),
        _ => t!("Unknown"),
    }
}

fn state_mark(state: &str) -> View {
    let mark = match state {
        "ready" => view().w(px(8.0)).h(px(8.0)).rounded(px(4.0)).bg(theme::ADDED).into_element(),
        "starting" => ui::glyph("spinner", 12.0, theme::WORKING).animate_spin().into_element(),
        "failed" => text("!").text_size(theme::SMALL).font_weight(FontWeight::BOLD).text_color(theme::BLOCKED).into_element(),
        "paused" => ui::glyph("pause", 11.0, theme::MUTED).into_element(),
        "stopped" | "gone" => view().w(px(8.0)).h(px(8.0)).rounded(px(4.0)).border_1().border_color(theme::DIM).into_element(),
        _ => text("?").text_size(theme::SMALL).font_weight(FontWeight::BOLD).text_color(theme::DIM).into_element(),
    };
    let ink = match state {
        "ready" => theme::ADDED,
        "starting" => theme::WORKING,
        "failed" => theme::BLOCKED,
        "paused" => theme::FOREGROUND,
        _ => theme::MUTED,
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .px(px(7.0))
        .h(px(20.0))
        .rounded(px(6.0))
        .bg(theme::HOVER)
        .test_id(&format!("state-{state}"))
        .child(view().w(px(12.0)).items_center().child(mark))
        .child(text(state_word(state)).text_size(theme::CAPTION).font_weight(FontWeight::MEDIUM).text_color(ink))
}

/// The Bots page: one card of the computer's bots.
pub fn bots_page(live: Live) -> nsh_core::element::Screen {
    on(&live.computer.id());
    {
        let live = live.clone();
        spawn(nav::while_looked_at(Route::Bots, EVERY, move || refresh(live.clone())));
    }
    let refreshing = signal(false);
    let pulled = {
        let live = live.clone();
        move || {
            refreshing.set(true);
            let live = live.clone();
            spawn(async move {
                refresh(live).await;
                refreshing.set(false);
            });
        }
    };
    let world = live.view;
    let state = computed(move || {
        let connected = world.with(|v| matches!(v.link, Link::Connected { .. }));
        shown().with(|s| (s.rows.clone(), s.fresh && connected, s.loaded, s.missing, s.error.clone()))
    });
    ui::page(t!("Bots")).child(
        scroll()
            .flex_1()
            .bg(theme::CANVAS)
            .refreshing(move || refreshing.get())
            .on_refresh(pulled)
            .child(dynamic(move || {
                let (rows, fresh, loaded, missing, error) = state.get();
                let mut page = view().pt(px(4.0)).pb(move || px(safe_area().bottom.0 + 24.0));
                if missing {
                    return page.child(not_there()).into_element();
                }
                if let Some(error) = error {
                    page = page.child(note(error, theme::REMOVED, "bots-error"));
                } else if loaded && !fresh {
                    page = page.child(note(t!("Not up to date: the computer is not connected.").into(), theme::MUTED, "bots-stale"));
                }
                if !loaded {
                    return page.child(note(t!("Loading bots…").into(), theme::MUTED, "bots-loading")).into_element();
                }
                if rows.is_empty() {
                    return page
                        .child(note(t!("No bots yet.").into(), theme::MUTED, "bots-none"))
                        .child(ui::card_footer(t!("Make one on the computer: yonder bot new <name>")))
                        .into_element();
                }
                page.child(ui::card_heading(t!("BOTS"))).child(ui::card(rows.into_iter().map(bot_row).collect())).into_element()
            })),
    )
}

/// A line of words above the card (or instead of it).
fn note(words: String, ink: AdaptiveColor, id: &'static str) -> View {
    view().px(px(32.0)).pt(px(14.0)).test_id(id).child(text(words).text_size(theme::FOOTNOTE).text_color(ink).number_of_lines(4))
}

/// No `yonder` on the computer: said so, and how to get it there.
fn not_there() -> View {
    view()
        .test_id("bots-missing")
        .child(ui::card_heading(t!("BOTS")))
        .child(ui::card(vec![view()
            .flex_row()
            .items_center()
            .gap(px(14.0))
            .px(px(16.0))
            .py(px(14.0))
            .child(ui::glyph("bot", 20.0, theme::MUTED))
            .child(text(t!("There is no yonder on this computer.")).flex_1().text_size(theme::ROW).text_color(theme::FOREGROUND))]))
        .child(ui::card_footer(t!("Install it there (in the Yonder repository: cargo install --path crates/yonder-cli), so that a login shell finds yonder; then pull down to look again.")))
}

/// One bot's row: its name and description, machine and harness, state.
fn bot_row(row: BotRow) -> View {
    let name = row.name.clone();
    let sub = if row.valid { format!("{} · {}", row.machine, row.harness) } else { row.problem.clone().unwrap_or_default() };
    let label = format!("{}, {}", row.name, if row.valid { state_word(&row.state) } else { t!("Definition has errors") });
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .py(px(10.0))
        .min_h(px(58.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(label)
        .test_id(&format!("bot:{}", row.name))
        .on_press(move || nav::push(Route::Bot(name.clone())))
        .child(ui::glyph("bot", 20.0, theme::MUTED))
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(text(row.name.clone()).text_size(theme::ROW).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND).number_of_lines(1).flex_shrink_0())
                        .child(text(row.description.clone()).text_size(theme::CAPTION).text_color(theme::MUTED).number_of_lines(1).flex_shrink(1.0)),
                )
                .child(text(sub).text_size(theme::CAPTION).text_color(if row.valid { theme::MUTED } else { theme::BLOCKED }).number_of_lines(2)),
        )
        .child(if row.valid {
            state_mark(&row.state)
        } else {
            view().test_id("state-invalid").child(text(t!("Definition has errors")).text_size(theme::CAPTION).text_color(theme::BLOCKED))
        })
        .child(ui::glyph("chevron-right", 16.0, theme::DIM))
}

/// One bot: what the list says and its terminal, session and last reason,
/// and the buttons its state allows.
pub fn bot_page(live: Live, name: String) -> nsh_core::element::Screen {
    on(&live.computer.id());
    {
        let live = live.clone();
        spawn(nav::while_looked_at(Route::Bot(name.clone()), EVERY, move || refresh(live.clone())));
    }
    let world = live.view;
    let found = {
        let name = name.clone();
        computed(move || {
            let connected = world.with(|v| matches!(v.link, Link::Connected { .. }));
            shown().with(|s| (s.rows.iter().find(|r| r.name == name).cloned(), s.fresh && connected, s.busy.clone(), s.failed.clone().filter(|(n, _)| *n == name)))
        })
    };
    let title = name.clone();
    ui::page(title).child(scroll().flex_1().bg(theme::CANVAS).child(dynamic(move || {
        let (row, fresh, busy, failed) = found.get();
        let page = view().pt(px(4.0)).pb(move || px(safe_area().bottom.0 + 24.0));
        let Some(row) = row else {
            return page.child(note(t!("This bot is not on the computer's list.").into(), theme::MUTED, "bot-gone")).into_element();
        };
        let none = || "-".to_owned();
        let field = |key: &'static str, value: String| detail(key, value);
        let mut facts = vec![
            view()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .px(px(16.0))
                .min_h(px(52.0))
                .child(text(t!("State")).w(px(96.0)).text_size(theme::CALLOUT).text_color(theme::MUTED))
                .child(if row.valid { state_mark(&row.state) } else { view().child(text(t!("Definition has errors")).text_size(theme::CALLOUT).text_color(theme::BLOCKED)) }),
        ];
        if !row.valid {
            facts.push(field(t!("Problem"), row.problem.clone().unwrap_or_else(none)));
        } else {
            facts.extend([
                field(t!("Description"), row.description.clone()),
                field(t!("Machine"), row.machine.clone()),
                field(t!("Harness"), row.harness.clone()),
                field(t!("Terminal"), row.address.clone().unwrap_or_else(none)),
                field(t!("Session"), row.session.clone().unwrap_or_else(none)),
                field(t!("Last reason"), row.reason.clone().unwrap_or_else(none)),
            ]);
        }
        let mut page = page;
        if !fresh {
            page = page.child(note(t!("Not up to date: the computer is not connected.").into(), theme::MUTED, "bots-stale"));
        }
        page = page.child(ui::card_heading(t!("BOT"))).child(ui::card(facts));
        if !row.valid {
            return page.into_element();
        }
        let mut buttons = Vec::new();
        let doing = busy.as_ref().map(|(_, verb)| *verb);
        let held = busy.is_some();
        let mut button = |verb: &'static str, icon: &'static str, words: &'static str, going: &'static str| {
            let (live, name) = (live.clone(), row.name.clone());
            let now = if doing == Some(verb) { going } else { words };
            buttons.push(action(icon, now, held, verb, move || act(&live, verb, &name)));
        };
        match row.state.as_str() {
            "stopped" | "failed" | "gone" => button("start", "play", t!("Start"), t!("Starting…")),
            "ready" => {
                button("stop", "square", t!("Stop this turn"), t!("Stopping…"));
                button("pause", "pause", t!("Pause"), t!("Pausing…"));
            }
            "paused" => button("resume", "play", t!("Resume"), t!("Resuming…")),
            _ => {}
        }
        let alive = row.address.as_ref().is_some_and(|a| world.with_untracked(|v| v.terminals.iter().any(|t| t["address"].as_str() == Some(a) && t["state"].as_str() != Some("finished"))));
        if let (true, Some(address)) = (alive, row.address.clone()) {
            buttons.push(ui_go("terminal", t!("Show the terminal"), "bot-terminal", move || nav::push(Route::Peek(address.clone()))));
        }
        if !buttons.is_empty() {
            page = page.child(ui::card_heading(t!("ACTIONS"))).child(ui::card(buttons));
        }
        if let Some((_, line)) = failed {
            page = page.child(view().px(px(32.0)).pt(px(8.0)).test_id("bot-error").child(text(line).text_size(theme::FOOTNOTE).text_color(theme::REMOVED).number_of_lines(4)));
        }
        page.into_element()
    })))
}

/// A fact about the bot: what it is, and its value (selectable).
fn detail(key: &'static str, value: String) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .py(px(10.0))
        .min_h(px(44.0))
        .child(text(key).w(px(96.0)).text_size(theme::CALLOUT).text_color(theme::MUTED))
        .child(text(value).flex_1().text_size(theme::CALLOUT).text_color(theme::FOREGROUND).selectable(true))
}

/// A button of the bot's: accent, as rows that do something here are;
/// faded and still while an action runs.
fn action(icon: &'static str, words: &'static str, held: bool, verb: &'static str, press: impl Fn() + 'static) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .h(px(52.0))
        .opacity(if held { 0.5 } else { 1.0 })
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(words)
        .disabled(held)
        .test_id(&format!("bot-{verb}"))
        .on_press(move || {
            if !held {
                press();
            }
        })
        .child(ui::glyph(icon, 20.0, theme::ACCENT))
        .child(text(words).flex_1().text_size(theme::ROW).text_color(theme::ACCENT))
}

/// A row that goes to another page.
fn ui_go(icon: &'static str, words: &'static str, id: &'static str, press: impl Fn() + 'static) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .h(px(52.0))
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(words)
        .test_id(id)
        .on_press(press)
        .child(ui::glyph(icon, 20.0, theme::MUTED))
        .child(text(words).flex_1().text_size(theme::ROW).text_color(theme::FOREGROUND))
        .child(ui::glyph("chevron-right", 16.0, theme::DIM))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_says_yonder_s_first_line_not_the_shell_s() {
        let ran = |err: &str| herdc_engine::Ran { code: Some(1), stdout: Vec::new(), stderr: err.as_bytes().to_vec(), timed_out: false };
        let err = "sh: no job control in this shell\nyonder-app: no job control in this shell\nconfig: bad home; fix it\nmore\n";
        assert_eq!(said(&ran(err)), "config: bad home; fix it");
        assert_eq!(said(&ran("sh: no job control in this shell\n")), "sh: no job control in this shell");
        assert_eq!(said(&ran("")), "exit 1");
    }

    #[test]
    fn yonder_runs_in_the_login_shell_with_its_arguments_whole() {
        let argv = argv(&["bot", "start", "ops"]);
        assert_eq!(argv[..5], ["/bin/sh", "-c", WRAP, "yonder-app", "yonder"]);
        assert_eq!(argv[5..], ["bot", "start", "ops"]);
        // Run here as the core runs it: each argument arrives whole.
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..4])
            .args(["printf", "[%s]", "a b", "$HOME", "'q'"])
            .env("SHELL", "/bin/sh")
            .env("HOME", std::env::temp_dir())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "[a b][$HOME]['q']");
    }
}
