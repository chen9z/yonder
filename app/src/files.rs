//! Files: the focused folder as a tree, folders opened as they are tapped,
//! git's marks on the files; a file's text a tap away, highlighted as its
//! name says. Read only, as the desktop's files mod is.

use crate::live::{off, Live};
use crate::nav::{self, Route};
use crate::{theme, ui};
use herdc_engine::tools::files::{self, Listing, Preview, PER_FOLDER, TOTAL};
use herdc_engine::tools::Place;
use nsh_base::{editor, text_view, EditorScrollBeyond};
use nsh_core::element::Screen;
use nsh_core::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

/// How often the tree is read again (the desktop also watches local
/// folders; through a core there is nothing to watch).
const EVERY: Duration = Duration::from_secs(10);

/// One row of the tree.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    /// Under the folder, `/` between parts.
    path: String,
    name: String,
    depth: usize,
    dir: bool,
    open: bool,
    mark: Option<char>,
}

#[derive(Clone, Copy)]
struct Tree {
    /// Read folders by path ("" the top).
    folders: Signal<HashMap<String, Listing>>,
    /// Folders opened, the top always.
    open: Signal<BTreeSet<String>>,
    marks: Signal<HashMap<String, char>>,
    error: Signal<Option<String>>,
    read: Signal<bool>,
}

impl Tree {
    fn rows(&self, root: &str) -> Vec<Row> {
        let mut rows = Vec::new();
        self.folders.with(|folders| {
            self.open.with(|open| {
                self.marks.with(|marks| walk(folders, open, marks, root, "", 0, &mut rows));
            })
        });
        rows
    }

    /// Whether some folder was cut short.
    fn limited(&self) -> bool {
        self.folders.with(|f| f.values().any(|l| l.limited)) || self.folders.with(|f| f.values().map(|l| l.entries.len()).sum::<usize>() >= TOTAL)
    }
}

fn walk(folders: &HashMap<String, Listing>, open: &BTreeSet<String>, marks: &HashMap<String, char>, root: &str, at: &str, depth: usize, rows: &mut Vec<Row>) {
    let Some(listing) = folders.get(at) else { return };
    for e in &listing.entries {
        if rows.len() >= TOTAL || depth > 40 {
            return;
        }
        let path = if at.is_empty() { e.name.clone() } else { format!("{at}/{}", e.name) };
        let is_open = e.dir && open.contains(&path);
        let mark = (!e.dir).then(|| marks.get(&format!("{root}/{path}")).copied()).flatten();
        rows.push(Row { path: path.clone(), name: e.name.clone(), depth, dir: e.dir, open: is_open, mark });
        if is_open {
            walk(folders, open, marks, root, &path, depth + 1, rows);
        }
    }
}

pub fn files_page(live: Live, place: Place) -> Screen {
    let tree = Tree { folders: signal(HashMap::new()), open: signal(BTreeSet::from([String::new()])), marks: signal(HashMap::new()), error: signal(None), read: signal(false) };
    let root = place.folder.clone();
    let rows = {
        let root = root.clone();
        computed(move || tree.rows(&root))
    };
    {
        let (live, place) = (live.clone(), place.clone());
        spawn(async move {
            loop {
                refresh(&live, &place, tree).await;
                off(tokio::time::sleep(EVERY)).await;
            }
        });
    }
    let title = root.rsplit('/').find(|p| !p.is_empty()).unwrap_or("/").to_owned();
    // Pulled down, read again now rather than in up to 10 s.
    let refreshing = signal(false);
    let pulled = {
        let (live, place) = (live.clone(), place.clone());
        move || {
            refreshing.set(true);
            let (live, place) = (live.clone(), place.clone());
            spawn(async move {
                refresh(&live, &place, tree).await;
                refreshing.set(false);
            });
        }
    };
    ui::page(title).child(
        view()
            .flex_1()
            .bg(theme::CANVAS)
            .child(
                virtual_list(move || rows.with(Vec::len), move |at| {
                    let (live, place) = (live.clone(), place.clone());
                    let row = move || rows.with(|r| r.get(at.index()).cloned());
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .pl(move || px(12.0 + row().map_or(0, |r| r.depth) as f32 * 16.0))
                        .pr(px(16.0))
                        .h(px(44.0))
                        .active(|s| s.bg(theme::HOVER))
                        .role(AccessibilityRole::Button)
                        .label(move || row().map(|r| r.name).unwrap_or_default())
                        .accessibility_state(move || AccessibilityState {
                            expanded: row().filter(|r| r.dir).map(|r| r.open),
                            ..Default::default()
                        })
                        .on_press(move || {
                            let Some(r) = row() else { return };
                            if r.dir {
                                toggle(&live, &place, tree, r.path);
                            } else {
                                nav::push(Route::File { place: place.clone(), path: r.path });
                            }
                        })
                        // A folder's chevron (a file's room for one), then what it is.
                        .child(dynamic(move || match row() {
                            Some(Row { dir: true, open, .. }) => ui::glyph(if open { "chevron-down" } else { "chevron-right" }, 12.0, theme::DIM).mx(px(1.0)).into_element(),
                            _ => view().w(px(14.0)).into_element(),
                        }))
                        .child(dynamic(move || match row() {
                            Some(Row { dir: true, open, .. }) => ui::glyph(if open { "folder-open" } else { "folder" }, 17.0, theme::MUTED).mr(px(4.0)).into_element(),
                            _ => ui::glyph("file", 17.0, theme::DIM).mr(px(4.0)).into_element(),
                        }))
                        .child(
                            text(move || row().map(|r| r.name).unwrap_or_default())
                                .flex_1()
                                .text_size(px(15.0))
                                .text_color(theme::FOREGROUND)
                                .number_of_lines(1),
                        )
                        .child(
                            text(move || row().and_then(|r| r.mark).map(String::from).unwrap_or_default())
                                .font_family(theme::MONO)
                                .text_size(px(13.0))
                                .text_color(move || theme::now(mark_color(row().and_then(|r| r.mark)))),
                        )
                })
                .estimated_item_size(px(44.0))
                .refreshing(move || refreshing.get())
                .on_refresh(pulled)
                // The last row clears the system's bar (the note on the limit does, if shown).
                .footer(view().h(move || px(if tree.limited() { 0.0 } else { safe_area().bottom.0 })))
                .empty(
                    view().p(px(32.0)).items_center().child(
                        text(move || match tree.error.get() {
                            Some(e) => e,
                            None if !tree.read.get() => t!("Loading files…").into(),
                            None => t!("No files").into(),
                        })
                        .text_size(px(15.0))
                        .text_color(theme::MUTED),
                    ),
                )
                .flex_1(),
            )
            .child(when(move || tree.limited(), || {
                view()
                    .px(px(16.0))
                    .py(px(8.0))
                    .pb(move || px(safe_area().bottom.0 + 8.0))
                    .bg(theme::RAISED)
                    .child(text(t!("File list limited to {} entries per folder and {} total", PER_FOLDER, TOTAL)).text_size(px(12.0)).text_color(theme::MUTED))
            })),
    )
}

pub fn mark_color(mark: Option<char>) -> AdaptiveColor {
    match mark {
        Some('A') => theme::ADDED,
        Some('D' | 'U') => theme::REMOVED,
        Some(_) => theme::WORKING,
        None => theme::MUTED,
    }
}

/// Reads the open folders again, the top first, and the marks.
async fn refresh(live: &Live, place: &Place, tree: Tree) {
    let there = match live.there(place.on.clone()) {
        Ok(there) => there,
        Err(e) => return tree.error.set(Some(e)),
    };
    let open: Vec<String> = tree.open.get_untracked().into_iter().collect();
    for path in open {
        if !tree.open.with_untracked(|o| o.contains(&path)) {
            continue;
        }
        load(&there, place, tree, path).await;
    }
    let (there2, root) = (there.clone(), place.folder.clone());
    if let Some(marks) = off(async move { files::marks(&there2, &root).await }).await {
        tree.marks.set(marks);
    }
    tree.read.set(true);
}

/// Reads one folder, keeping within [`TOTAL`] with the others.
async fn load(there: &herdc_engine::tools::There, place: &Place, tree: Tree, path: String) {
    let full = if path.is_empty() { place.folder.clone() } else { format!("{}/{path}", place.folder) };
    let there = there.clone();
    let Some(read) = off(async move { files::list(&there, &full).await }).await else { return };
    match read {
        Ok(mut listing) => {
            // Opened, then closed while it was read: not wanted any more.
            if !tree.open.with_untracked(|o| o.contains(&path)) {
                return;
            }
            let used: usize = tree.folders.with_untracked(|f| f.iter().filter(|(p, _)| **p != path).map(|(_, l)| l.entries.len()).sum());
            let room = PER_FOLDER.min(TOTAL.saturating_sub(used));
            if listing.entries.len() > room {
                listing.entries.truncate(room);
                listing.limited = true;
            }
            tree.folders.update(|f| drop(f.insert(path.clone(), listing)));
            if path.is_empty() {
                tree.error.set(None);
            }
        }
        // A folder that went away closes; the top says why.
        Err(e) if path.is_empty() => tree.error.set(Some(e)),
        Err(_) => close(tree, &path),
    }
}

fn toggle(live: &Live, place: &Place, tree: Tree, path: String) {
    if tree.open.with_untracked(|o| o.contains(&path)) {
        return close(tree, &path);
    }
    tree.open.update(|o| {
        o.insert(path.clone());
    });
    let Ok(there) = live.there(place.on.clone()) else { return };
    let place = place.clone();
    spawn(async move { load(&there, &place, tree, path).await });
}

/// Closes a folder and every one under it.
fn close(tree: Tree, path: &str) {
    let under = |p: &str| p == path || p.strip_prefix(path).is_some_and(|r| r.starts_with('/'));
    tree.open.update(|o| o.retain(|p| !under(p)));
    tree.folders.update(|f| f.retain(|p, _| !under(p)));
}

/// One file's text; Markdown is shown as it reads (its source a press away).
pub fn file_page(live: Live, place: Place, path: String) -> Screen {
    let shown = signal(None::<Preview>);
    {
        let full = format!("{}/{path}", place.folder);
        spawn(async move {
            let preview = match live.there(place.on.clone()) {
                Ok(there) => off(async move { files::preview(&there, &full).await }).await,
                Err(_) => Some(Preview::Unreadable),
            };
            shown.set(preview);
        });
    }
    let code = move || shown.with(|p| match p {
        Some(Preview::Text { text, .. }) if !text.is_empty() => Some(SharedString::from(text.clone())),
        _ => None,
    });
    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
    // As large as the terminal's text (Settings, or a pinch there).
    let font = use_context::<crate::prefs::Prefs>().expect("prefs").font.get_untracked();
    // The editor reads the language from the file's name (`main.rs`, `Dockerfile`).
    let language = SharedString::from(name.clone());
    let markdown = [".md", ".markdown"].iter().any(|e| name.to_ascii_lowercase().ends_with(e));
    let reading = signal(markdown);
    ui::page(name)
        .header_right(when(move || markdown && code().is_some(), move || {
            view()
                .px(px(12.0))
                .h(px(40.0))
                .justify_center()
                .role(AccessibilityRole::Button)
                .label(move || if reading.get() { t!("Show the source") } else { t!("Show as it reads") })
                .on_press(move || reading.update(|r| *r = !*r))
                .child(text(move || if reading.get() { t!("Source") } else { t!("Preview") }).text_size(px(15.0)).text_color(theme::FOREGROUND))
        }))
        .child(view().flex_1().bg(theme::BACKGROUND).pb(move || px(safe_area().bottom.0)).child(dynamic(move || {
            if code().is_none() {
                return view().p(px(32.0)).items_center().child(
                    text(move || shown.with(|p| p.as_ref().map_or(t!("Loading file…"), |p| p.said().unwrap_or(t!("Empty file"))).to_owned()))
                        .text_size(px(15.0))
                        .text_color(theme::MUTED),
                ).into_element();
            }
            if reading.get() {
                return scroll().flex_1().child(
                    text_view()
                        .markdown(code)
                        .selectable(true)
                        .gap_3()
                        .p(px(16.0))
                        .text_size(px(15.0))
                        .line_height(px(22.0))
                        .text_color(theme::FOREGROUND),
                ).into_element();
            }
            editor()
                .default_value(code())
                .language(Some(language.clone()))
                .read_only(true)
                .soft_wrap(true)
                .active_line(false)
                .scroll_beyond_last_line(EditorScrollBeyond::from(false))
                .font_family(Some(SharedString::from(theme::MONO)))
                .font_size(px(font))
                .line_height(px((font * 1.5).round()))
                .flex_1()
                .into_element()
        })))
}

/// A line of code at `font` points: its number in the gutter, the text
/// wrapped after.
pub fn code_line(
    font: f32,
    number: impl Fn() -> Option<u64> + 'static,
    line: impl Fn() -> String + 'static,
    row: impl Fn() -> AdaptiveColor + 'static,
    color: impl Fn() -> AdaptiveColor + 'static,
) -> View {
    view()
        .flex_row()
        .bg(move || theme::now(row()))
        .px(px(4.0))
        .child(
            text(move || number().map(|n| n.to_string()).unwrap_or_default())
                .w(px((font * 3.4).round()))
                .pr(px(8.0))
                .text_right()
                .font_family(theme::MONO)
                .text_size(px(font))
                .line_height(px((font * 1.5).round()))
                .text_color(theme::DIM),
        )
        .child(text(line).flex_1().font_family(theme::MONO).text_size(px(font)).line_height(px((font * 1.5).round())).text_color(move || theme::now(color())).selectable(true))
}
