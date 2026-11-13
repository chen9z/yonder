//! Changes: what isn't committed in the repo the focused folder is in, each
//! file's diff a tap away, and a commit of all of it, as the desktop's
//! changes mod.

use crate::files::{code_line, mark_color};
use crate::live::{off, Live};
use crate::nav::{self, Route};
use crate::{theme, ui};
use herdc_engine::tools::changes::{self, Changes, Kind, Line, FILES};
use herdc_engine::tools::Place;
use nsh_base::input;
use nsh_core::element::Screen;
use nsh_core::prelude::*;
use std::sync::Arc;
use std::time::Duration;

const EVERY: Duration = Duration::from_secs(10);

pub fn changes_page(live: Live, place: Place) -> Screen {
    let found = signal(None::<Arc<Changes>>);
    let error = signal(None::<String>);
    let message = signal(String::new());
    let busy = signal(false);
    let note = signal(None::<String>);
    let rescan = {
        let (live, place) = (live.clone(), place.clone());
        move || {
            let (live, place) = (live.clone(), place.clone());
            async move {
                let read = match live.there(place.on.clone()) {
                    Ok(there) => off(async move { changes::scan(&there, &place.folder).await }).await,
                    Err(e) => Some(Err(e)),
                };
                match read {
                    Some(Ok(c)) => {
                        found.set(Some(Arc::new(c)));
                        error.set(None);
                    }
                    Some(Err(e)) => {
                        found.set(None);
                        error.set(Some(e));
                    }
                    None => {}
                }
            }
        }
    };
    {
        let rescan = rescan.clone();
        spawn(async move {
            loop {
                if !busy.get_untracked() {
                    rescan().await;
                }
                off(tokio::time::sleep(EVERY)).await;
            }
        });
    }
    let pulled_scan = rescan.clone();
    let commit = {
        let (live, place) = (live.clone(), place.clone());
        move || {
            let Some(root) = found.with_untracked(|c| c.as_ref().map(|c| c.root.clone())) else { return };
            let words = message.get_untracked();
            if words.trim().is_empty() || busy.get_untracked() {
                return;
            }
            let Ok(there) = live.there(place.on.clone()) else { return };
            busy.set(true);
            note.set(None);
            let rescan = rescan.clone();
            spawn(async move {
                match off(async move { changes::commit(&there, &root, &words).await }).await {
                    Some(Ok(())) => message.set(String::new()),
                    Some(Err(e)) => note.set(Some(e)),
                    None => {}
                }
                busy.set(false);
                rescan().await;
            });
        }
    };
    let files = computed(move || found.with(|c| c.as_ref().map_or(0, |c| c.files.len())));
    // Pulled down, read again now rather than in up to 10 s.
    let refreshing = signal(false);
    let pulled = {
        let rescan = pulled_scan;
        move || {
            refreshing.set(true);
            let rescan = rescan.clone();
            spawn(async move {
                rescan().await;
                refreshing.set(false);
            });
        }
    };
    ui::page(t!("Changes"))
        .header_right(
            text(move || found.with(|c| c.as_ref().map(|c| format!("+{}  −{}", c.added(), c.deleted())).unwrap_or_default()))
                .font_family(theme::MONO)
                .text_size(px(13.0))
                .text_color(theme::MUTED),
        )
        .child(
            view()
                .flex_1()
                .bg(theme::CANVAS)
                .child(
                    virtual_list(move || files.get(), move |at| {
                        let place = place.clone();
                        let file = move || found.with(|c| c.as_ref().and_then(|c| c.files.get(at.index()).cloned()));
                        let (dir, name) = (
                            move || file().map(|f| f.path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default()).unwrap_or_default(),
                            move || file().map(|f| f.path.rsplit('/').next().unwrap_or_default().to_owned()).unwrap_or_default(),
                        );
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(16.0))
                            .h(px(48.0))
                            .border_b_1()
                            .border_color(theme::BORDER)
                            .active(|s| s.bg(theme::HOVER))
                            .on_press(move || {
                                if let Some(changes) = found.get_untracked() {
                                    nav::push(Route::Diff { place: place.clone(), changes, file: at.index() });
                                }
                            })
                            .child(
                                text(move || file().map(|f| f.status.to_string()).unwrap_or_default())
                                    .w(px(14.0))
                                    .font_family(theme::MONO)
                                    .text_size(px(13.0))
                                    .text_color(move || theme::now(mark_color(file().map(|f| f.status)))),
                            )
                            .child(
                                view()
                                    .flex_1()
                                    .flex_row()
                                    .child(text(dir).text_size(px(14.0)).text_color(theme::DIM).number_of_lines(1).flex_shrink(1.0))
                                    .child(text(name).text_size(px(14.0)).text_color(theme::FOREGROUND).number_of_lines(1).flex_shrink_0()),
                            )
                            .child(count(move || file().map_or(0, |f| f.add), "+", theme::ADDED))
                            .child(count(move || file().map_or(0, |f| f.del), "−", theme::REMOVED))
                    })
                    .estimated_item_size(px(48.0))
                    .refreshing(move || refreshing.get())
                    .on_refresh(pulled)
                    .empty(
                        view().p(px(32.0)).items_center().child(
                            text(move || match (error.get(), found.with(Option::is_some)) {
                                (Some(e), _) => e,
                                (None, false) => t!("Loading changes…").into(),
                                (None, true) => t!("No uncommitted changes").into(),
                            })
                            .text_size(px(15.0))
                            .text_color(theme::MUTED),
                        ),
                    )
                    .keyboard_taps(KeyboardTaps::Handled)
                    .flex_1(),
                )
                .child(when(move || found.with(|c| c.as_ref().is_some_and(|c| c.limited)), || {
                    view().px(px(16.0)).py(px(6.0)).bg(theme::RAISED).child(text(t!("Showing the first {} changed files", FILES)).text_size(px(12.0)).text_color(theme::MUTED))
                }))
                .child(when(move || note.with(Option::is_some), move || {
                    view().px(px(16.0)).py(px(6.0)).child(text(move || note.get().unwrap_or_default()).text_size(px(13.0)).text_color(theme::REMOVED).number_of_lines(4))
                }))
                .child(when(move || files.get() > 0, move || commit_bar(message, busy, commit.clone()))),
        )
}

fn count(n: impl Fn() -> u64 + 'static, sign: &'static str, color: AdaptiveColor) -> impl IntoElement {
    text(move || match n() {
        0 => String::new(),
        n => format!("{sign}{n}"),
    })
    .font_family(theme::MONO)
    .text_size(px(12.0))
    .text_color(color)
}

/// The message and its button, above the keyboard.
fn commit_bar(message: Signal<String>, busy: Signal<bool>, commit: impl Fn() + Clone + 'static) -> impl IntoElement {
    let submit = commit.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .pt(px(8.0))
        .pb(move || px(8.0 + keyboard_height().0.max(safe_area().bottom.0)))
        .bg(theme::CHROME)
        .border_t_1()
        .border_color(theme::BORDER)
        .child(
            input()
                .value(move || message.get())
                .on_change(move |t: &str| message.set(t.to_owned()))
                .on_submit(move |_: &str| submit())
                .placeholder(t!("Commit message"))
                .placeholder_color(move || theme::now(theme::DIM))
                .return_key_type(ReturnKeyType::Done)
                .flex_1()
                .h(px(42.0))
                .px(px(12.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(theme::INPUT)
                .bg(theme::BACKGROUND)
                .text_size(px(15.0))
                .text_color(theme::FOREGROUND),
        )
        .child(
            view()
                .h(px(42.0))
                .px(px(14.0))
                .rounded(px(10.0))
                .justify_center()
                .bg(move || if message.with(|m| m.trim().is_empty()) || busy.get() { theme::now(theme::RAISED) } else { theme::now(theme::ACCENT) })
                .on_press(commit)
                .child(
                    text(move || if busy.get() { t!("Committing…") } else { t!("Commit") })
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || if message.with(|m| m.trim().is_empty()) { theme::now(theme::MUTED) } else { theme::now(theme::ACCENT_FOREGROUND) }),
                ),
        )
}

/// A changed file's diff; the file before or after it a tap away.
pub fn diff_page(live: Live, place: Place, changes: Arc<Changes>, at: usize) -> Screen {
    let lines = signal(None::<Vec<Line>>);
    let file = changes.files.get(at).cloned();
    let title = file.as_ref().map(|f| f.path.rsplit('/').next().unwrap_or_default().to_owned()).unwrap_or_default();
    let font = use_context::<crate::prefs::Prefs>().expect("prefs").font.get_untracked();
    let total = changes.files.len();
    let step = {
        let (place, changes) = (place.clone(), changes.clone());
        move |to: usize| {
            let (place, changes) = (place.clone(), changes.clone());
            move || nav::replace(Route::Diff { place: place.clone(), changes: changes.clone(), file: to })
        }
    };
    let turn = |glyph: &'static str, label: &'static str, to: Option<usize>| match to {
        Some(to) => ui::icon_button(glyph, label, step(to)),
        None => ui::icon_button(glyph, label, || {}).disabled(true).opacity(0.3),
    };
    let header = view()
        .flex_row()
        .items_center()
        .child(text(t!("{} of {}", at + 1, total)).text_size(px(13.0)).text_color(theme::MUTED).mr(px(4.0)))
        .child(turn("chevron-left", t!("Previous file"), at.checked_sub(1)))
        .child(turn("chevron-right", t!("Next file"), (at + 1 < total).then_some(at + 1)));
    if let Some(file) = file {
        spawn(async move {
            let Ok(there) = live.there(place.on.clone()) else { return lines.set(Some(Vec::new())) };
            if let Some(read) = off(async move { changes::diff(&there, &changes, &file).await }).await {
                lines.set(Some(read));
            }
        });
    }
    let count = computed(move || lines.with(|l| l.as_ref().map_or(0, Vec::len)));
    ui::page(title).header_right(header).child(
        view().flex_1().bg(theme::BACKGROUND).child(
            virtual_list(move || count.get(), move |at| {
                let line = move || lines.with(|l| l.as_ref().and_then(|l| l.get(at.index()).cloned()));
                let kind = move || line().map_or(Kind::Other, |l| l.kind);
                code_line(
                    font,
                    move || line().and_then(|l| l.number),
                    move || line().map(|l| l.text).unwrap_or_default(),
                    move || match kind() {
                        Kind::Add => theme::ADDED_ROW,
                        Kind::Del => theme::REMOVED_ROW,
                        Kind::Hunk => theme::RAISED,
                        _ => theme::BACKGROUND,
                    },
                    move || match kind() {
                        Kind::Hunk | Kind::Other => theme::MUTED,
                        _ => theme::FOREGROUND,
                    },
                )
            })
            .estimated_item_size(px(18.0))
            .empty(view().p(px(32.0)).items_center().child(text(t!("Loading diff…")).text_size(px(15.0)).text_color(theme::MUTED)))
            // The last line clears the system's bar.
            .footer(view().h(move || px(safe_area().bottom.0)))
            .flex_1(),
        ),
    )
}
