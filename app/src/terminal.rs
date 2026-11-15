//! One terminal on the phone. Its screen is drawn on a canvas, scaled down
//! when the grid is the desktop's; a swipe goes back through history (or
//! turns the program's wheel); a tap brings the keyboard, and typing takes
//! the terminal at the phone's own grid; a long press opens its text to
//! select and copy; a pinch sizes its font; a swipe across goes to the
//! next or previous tab. Back in history, a tap away
//! from the latest output. Keys a phone keyboard lacks are on a bar above it.
//! What a program copies (OSC 52) goes to the phone's clipboard.

use crate::prefs::Prefs;
use crate::nav::{self, Route};
use crate::keybar::{self, Does};
use crate::{link, theme, ui};
use herdc_engine::{Engine, Info, Mode, Viewer};
use herdc_term::{CursorKind, Key, Mods, Snapshot};
use nsh_core::canvas::CanvasTextBaseline;
use nsh_base::input;
use nsh_core::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) const INSET_X: f32 = 8.0;
pub(crate) const INSET_Y: f32 = 6.0;

/// The font sizes a pinch (and the settings) go between, in points.
pub const FONT: (f32, f32) = (7.0, 24.0);

/// How much history a selection offers.
const SELECT_HISTORY: usize = 2000;

/// The least time between two paints of a terminal (30 a second).
const PAINT: std::time::Duration = std::time::Duration::from_millis(33);

/// A fling slows by this much a second (of its speed), and stops below
/// [`FLING_LEAST`] points a second.
const FLING_DECAY: f32 = 0.06;
const FLING_LEAST: f32 = 60.0;

/// So long after a swipe or a key, what changes is the answer: drawn at
/// once, not held to [`PAINT`].
const ANSWERING: std::time::Duration = std::time::Duration::from_millis(300);

/// How far a swipe across goes to change tabs, in points.
const SWIPE: f32 = 80.0;

/// What a swipe across does: change tabs (in the workspace), or nothing
/// (an agent peeked at).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swipes {
    Tabs,
    Nothing,
}

/// Which way a drag went, once it went far enough to tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Open,
    /// Up or down: through history.
    Along,
    /// Left or right.
    Across,
}

/// A character cell at a font size, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub font: f32,
    pub width: f32,
    pub height: f32,
}

impl Cell {
    pub fn at(font: f32) -> Cell {
        let width = measure_text(&format!("{font}px {}", theme::MONO), "MMMMMMMMMM").0 / 10.0;
        Cell { font, width: if width > 0.0 { width } else { font * 0.6 }, height: (font * 1.3).ceil() }
    }

    /// The grid that fills `area`.
    pub fn fit(&self, (w, h): (f32, f32)) -> (u16, u16) {
        (((w / self.width) as u16).max(10), ((h / self.height) as u16).max(4))
    }
}

/// How Ctrl or Alt of the bar holds: not, for the next key (a tap), or
/// until tapped again (a second tap at once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    Off,
    Once,
    Locked,
}

/// Ctrl and Alt from the bar.
#[derive(Clone, Copy)]
struct Latch {
    ctrl: Signal<Hold>,
    alt: Signal<Hold>,
}

impl Latch {
    fn take(&self) -> Mods {
        let held = |hold: Signal<Hold>| {
            let now = hold.get_untracked();
            if now == Hold::Once {
                hold.set(Hold::Off);
            }
            now != Hold::Off
        };
        Mods { shift: false, ctrl: held(self.ctrl), alt: held(self.alt) }
    }
}

/// A key of the bar held down types again: after this long, this often.
const REPEAT_AFTER: std::time::Duration = std::time::Duration::from_millis(450);
const REPEAT_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

/// What the person typed: text as it is (or keys, while Ctrl or Alt hold).
fn send_text(viewer: &Viewer, latch: Latch, text: &str) {
    let mods = latch.take();
    if mods == Mods::NONE {
        viewer.input(text.as_bytes());
    } else {
        for c in text.chars() {
            viewer.key(Key::Char(c), mods);
        }
    }
}

/// What turns `old` into `new` when typed: backspaces, then the rest.
fn typed(old: &str, new: &str) -> (usize, String) {
    let same = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
    (old.chars().count() - same, new.chars().skip(same).collect())
}

/// The terminal at `address`, filling its parent: taken at the phone's
/// grid as it is shown, while the terminal fits the phone (the settings);
/// else watched at the desktop's until a tap brings the keyboard, unless
/// this phone started it. Another screen typing takes it back.
pub fn terminal(engine: Engine, address: String, swipes: Swipes) -> impl IntoElement {
    shown(engine, address, swipes, None)
}

/// The terminal as a conversation (the main agent's): a field to write
/// to it always under it, saying to whom (`name`); a tap on the terminal
/// still types into it, key by key.
pub fn talk(engine: Engine, address: String, name: String) -> impl IntoElement {
    draft_for(&address);
    shown(engine, address, Swipes::Nothing, Some(name))
}

fn shown(engine: Engine, address: String, swipes: Swipes, talk: Option<String>) -> impl IntoElement {
    let talked_to = address.clone();
    let prefs = use_context::<Prefs>().expect("prefs");
    let viewer = {
        let _entered = link::runtime().enter();
        let mode = engine.first_mode(&address, prefs.fit.get_untracked());
        let viewer = engine.viewer(&address, mode, untracked(theme::palette));
        if prefs.fit.get_untracked() {
            viewer.eager();
        }
        Rc::new(viewer)
    };
    let snapshot = signal(Arc::new(viewer.snapshot()));
    let typing = signal(viewer.mode() != Mode::Watch);
    let info = signal(viewer.info());
    {
        let (viewer, mut changes) = (viewer.clone(), viewer.changes());
        spawn(async move {
            let mut painted = std::time::Instant::now();
            while changes.changed().await.is_ok() {
                // One paint a frame, whatever arrived meanwhile, and at most
                // every PAINT: a flood is drawn at half the cost. A scroll
                // or a key just made is drawn every frame.
                // Recheck interaction on each vsync: a timer here cannot be
                // interrupted by a touch, and adds another frame after waking.
                loop {
                    next_frame().await;
                    if painted.elapsed() >= PAINT || viewer.idle() <= ANSWERING {
                        break;
                    }
                }
                painted = std::time::Instant::now();
                changes.borrow_and_update();
                snapshot.set(Arc::new(viewer.snapshot()));
                let now = viewer.info();
                if now.copied.is_some() && now.copied != info.with_untracked(|i| i.copied.clone()) {
                    set_clipboard(now.copied.clone().unwrap_or_default());
                    if let Some(live) = crate::live::live() {
                        live.say(herdc_engine::Note::info(t!("Copied"), t!("What the program copied is on the clipboard.")));
                    }
                }
                info.set(now);
                // It takes a terminal nobody holds, and lets go when another screen types.
                typing.set(viewer.mode() != Mode::Watch);
            }
        });
    }
    {
        let viewer = viewer.clone();
        effect(move || {
            let palette = theme::palette();
            viewer.with_screen(|s| s.set_palette(palette));
            snapshot.set(Arc::new(viewer.snapshot()));
        });
    }

    let area = signal((0.0f32, 0.0f32));
    let cell = computed(move || Cell::at(prefs.font.get()));
    // Typing takes the terminal at the phone's grid (or as it is), and new
    // terminals start at that grid too.
    {
        let (viewer, engine) = (viewer.clone(), engine.clone());
        effect(move || {
            let (w, h) = area.get();
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let (cols, rows) = cell.get().fit((w, h));
            engine.set_size(cols, rows);
            viewer.fit(cols, rows);
            if typing.get() {
                if prefs.fit.get() {
                    viewer.set_mode(Mode::Type { cols, rows });
                } else {
                    viewer.take();
                }
            }
        });
    }
    // The drawing's scale: the whole grid in the pane, never larger.
    let scale = computed(move || {
        let (w, h) = area.get();
        let c = cell.get();
        let (cols, rows) = snapshot.with(|s| (s.cols as f32, s.rows as f32));
        (w / (cols * c.width)).min(h / (rows * c.height)).clamp(0.05, 1.0)
    });

    let latch = Latch { ctrl: signal(Hold::Off), alt: signal(Hold::Off) };
    let field = element_ref();
    let focused = signal(false);
    let tap = move || field.focus();
    let swiped = stored(0i32);
    // Which fling is going: a new touch (or fling) ends the one before.
    let flung = stored(0u32);
    let flinger = viewer.clone();
    let axis = stored(Axis::Open);
    // Two fingers pinch the font: the drawing follows them, the font is
    // set when they lift.
    let zoom = animated_value(1.0);
    let pinching = stored(false);
    // A long press opened the text: its release is no tap.
    let held = stored(false);
    // Held: its text to select, or the clipboard pasted in.
    // The row a finger went down on: the links nearest it are offered first.
    let pressed = stored(0u16);
    // A message written before it goes in, kept while not sent.
    let writing = signal(false);
    let draft = signal(String::new());
    // Talked to, its field under it is where a message is written.
    let talks = talk.is_some();
    let select = {
        let viewer = viewer.clone();
        move || {
            let viewer = viewer.clone();
            let links = nearest_links(viewer.with_screen(|s| s.links()), pressed.get());
            let mut paths = nearest_links(viewer.with_screen(|s| s.file_paths()), pressed.get());
            paths.truncate(PATHS);
            let address = address.clone();
            spawn(async move {
                use crate::pick::{self, Item};
                let mut items: Vec<Item> = links.iter().enumerate().map(|(i, l)| Item::glyph(format!("link:{i}"), t!("Open {}", short_url(&l.url)), "external-link")).collect();
                items.extend(paths.iter().enumerate().map(|(i, p)| Item::glyph(format!("file:{i}"), t!("Open {}", short_path(&p.url)), "file")));
                items.extend([Item::glyph("select", t!("Select text"), "copy"), Item::glyph("paste", t!("Paste"), "clipboard")]);
                if !talks {
                    items.push(Item::glyph("write", t!("Write a message"), "pencil"));
                }
                let picked = pick::one(t!("Terminal"), items).await;
                match picked.as_deref() {
                    Some(link) if link.starts_with("link:") => {
                        if let Some(l) = link[5..].parse::<usize>().ok().and_then(|i| links.get(i)) {
                            let _ = open_url(l.url.clone()).await;
                        }
                    }
                    Some(file) if file.starts_with("file:") => {
                        let said = file[5..].parse::<usize>().ok().and_then(|i| paths.get(i));
                        if let Some(route) = said.and_then(|p| file_route(&address, &p.url)) {
                            nav::push(route);
                        }
                    }
                    Some("select") => {
                        let text = viewer.with_screen(|s| s.text_with_history(SELECT_HISTORY));
                        nav::push(Route::Text(Arc::new(text)));
                    }
                    Some("paste") => {
                        let text = clipboard_string().await;
                        if !text.is_empty() {
                            viewer.paste(&text);
                        }
                    }
                    Some("write") => writing.set(true),
                    _ => {}
                }
            });
        }
    };
    let drag = {
        let viewer = viewer.clone();
        move |e: &ResponderEvent| {
            if pinching.get() || e.gesture.active_touches > 1 {
                return;
            }
            let d = e.gesture.delta;
            if axis.get() == Axis::Open && d.x.0.hypot(d.y.0) > 12.0 {
                let across = swipes == Swipes::Tabs && d.x.0.abs() > d.y.0.abs() * 1.5;
                axis.replace(if across { Axis::Across } else { Axis::Along });
            }
            if axis.get() != Axis::Along {
                return;
            }
            let c = cell.get_untracked();
            let s = scale.get_untracked();
            let lines = (e.gesture.delta.y.0 / (c.height * s)) as i32;
            let turn = lines - swiped.replace(lines);
            if turn != 0 {
                let col = (e.location.x.0 / (c.width * s)) as u16;
                let row = (e.location.y.0 / (c.height * s)) as u16;
                viewer.wheel(turn, col, row);
                // The gesture already runs on the UI frame. Waiting for the
                // change watcher adds another frame of latency to every move.
                snapshot.set(Arc::new(viewer.snapshot()));
            }
        }
    };

    view()
        .flex_1()
        .bg(theme::BACKGROUND)
        .overflow_hidden()
        .child(
            view()
                .flex_1()
                .overflow_hidden()
                .mx(px(INSET_X))
                .my(px(INSET_Y))
                .on_layout(move |b: &Bounds<Points>| area.set((b.size.width.0, b.size.height.0)))
                .claim_touches(TouchClaim::on_start())
                .test_id("terminal")
                .on_responder_grant(move |e: &ResponderEvent| {
                    let (c, s) = (cell.get_untracked(), scale.get_untracked());
                    pressed.replace((e.location.y.0 / (c.height * s)).max(0.0) as u16);
                    // A finger down stops a fling.
                    flung.replace(flung.get().wrapping_add(1));
                    swiped.replace(0);
                    axis.replace(Axis::Open);
                    held.replace(false);
                })
                .on_responder_move(drag)
                .pinch_value(&zoom)
                .on_pinch_start(move |_: &GestureEvent| {
                    pinching.replace(true);
                })
                .on_pinch_end(move |e: &GestureEvent| {
                    let font = (prefs.font.get_untracked() * e.value).round().clamp(FONT.0, FONT.1);
                    prefs.font.set(font);
                    zoom.set(1.0);
                })
                .on_long_press(move || {
                    if swiped.get() == 0 && axis.get() == Axis::Open && !pinching.get() {
                        held.replace(true);
                        select();
                    }
                })
                .on_responder_release(move |e: &ResponderEvent| {
                    let d = e.gesture.delta;
                    let pinched = pinching.replace(false) || e.gesture.active_touches > 1;
                    if axis.get() == Axis::Along && !pinched && e.gesture.vy.abs() > FLING_LEAST * 4.0 {
                        fling(flinger.clone(), snapshot, flung, e.gesture.vy, e.location, cell.get_untracked(), scale.get_untracked());
                    }
                    if !held.replace(false) && !pinched && d.x.0.abs() < 8.0 && d.y.0.abs() < 8.0 {
                        // A program that asked for the mouse is clicked where tapped.
                        let (c, s) = (cell.get_untracked(), scale.get_untracked());
                        flinger.click((e.location.x.0 / (c.width * s)) as u16, (e.location.y.0 / (c.height * s)) as u16);
                        tap();
                    }
                    // Across, far enough: the next tab (to the left) or the one before.
                    if axis.replace(Axis::Open) == Axis::Across && !pinched && d.x.0.abs() > SWIPE
                        && let Some(live) = crate::live::live()
                    {
                        live.run(if d.x.0 < 0.0 { "tab.next" } else { "tab.prev" }, serde_json::json!({}));
                    }
                })
                .child(
                    // Touches are the terminal's: where a finger is is then
                    // counted from its top, not from the row it is on.
                    terminal_rows(snapshot, cell, scale).bind_scale(&zoom).pointer_events(PointerEvents::None),
                )
                .child(status(info))
                .child(latest(viewer.clone(), snapshot))
                .child(keyboard_field(viewer.clone(), field, focused, typing, latch)),
        )
        .child(when(move || focused.get(), {
            let viewer = viewer.clone();
            move || {
                hardware_keys(viewer.clone());
                key_bar(viewer.clone(), latch, writing)
            }
        }))
        .child(match talk {
            None => compose(viewer.clone(), writing, draft).into_element(),
            // Typed into key by key, the key bar is under it instead.
            Some(name) => {
                let viewer = viewer.clone();
                let owner = Owner::current();
                let attach = crate::chat::attach(talked_to.clone());
                when(move || !focused.get(), move || {
                    let viewer = viewer.clone();
                    let placeholder = t!("Message {}", name.clone());
                    talk_bar(move || placeholder.clone(), move |words: String| {
                        viewer.paste(&words);
                        let viewer = viewer.clone();
                        owner.run(|| set_timeout(ENTER_AFTER, move || viewer.key(Key::Enter, Mods::NONE)));
                    }, Offers { stop: None, attach: Some(attach.clone()) })
                })
                .into_element()
            }
        })
}

/// What is written to the main agent and not sent yet: kept while its
/// tab is left.
#[derive(Clone, Default, PartialEq)]
struct Unsent(String);

/// Whose the field is: a computer's agent.
#[derive(Clone, Default, PartialEq)]
struct DraftOf(String);

/// The field is for the agent at `address` on the computer open: what
/// was written for another is let go. Whether it was.
pub fn draft_for(address: &str) -> bool {
    let who = format!("{}\0{address}", crate::live::live().map(|l| l.computer.id()).unwrap_or_default());
    let of = global::<DraftOf>();
    if of.with_untracked(|o| o.0 == who) {
        return false;
    }
    of.set(DraftOf(who));
    global::<Unsent>().set(Unsent::default());
    true
}

/// What could not be sent, back in the field (before what was written
/// since).
pub fn unsent(words: &str) {
    global::<Unsent>().update(|d| d.0 = if d.0.trim().is_empty() { words.to_owned() } else { format!("{words}\n{}", d.0) });
}

/// What is written in the field (read as it changes).
pub fn draft() -> String {
    global::<Unsent>().with(|d| d.0.clone())
}

/// What is written in the field now.
pub fn written() -> String {
    global::<Unsent>().with_untracked(|d| d.0.clone())
}

/// What is written, replaced (a message to edit, a command chosen): the
/// caret after it.
pub fn rewrite(words: &str) {
    global::<Unsent>().set(Unsent(words.to_owned()));
    global::<ToEnd>().update(|n| n.0 += 1);
}

/// Words added at the end of what is written (a picture's path), the
/// caret after them.
pub fn add_to_draft(words: &str) {
    global::<Unsent>().update(|d| d.0 = if d.0.trim().is_empty() { words.to_owned() } else { format!("{}\n{words}", d.0.trim_end()) });
    global::<ToEnd>().update(|n| n.0 += 1);
}

/// Asked, the caret goes to the end of what is written (put there, not
/// typed: the field keeps it where it was).
#[derive(Clone, Copy, Default, PartialEq)]
struct ToEnd(u64);

/// Something a button does.
pub type Act = Rc<dyn Fn()>;

/// What the field under the main agent offers besides sending.
#[derive(Clone, Default)]
pub struct Offers {
    /// While the first says it works and nothing is written, its button
    /// stops it instead.
    pub stop: Option<(Memo<bool>, Act)>,
    /// Pictures to send: a button before the field, turning while they go.
    pub attach: Option<(Signal<bool>, Act)>,
}

/// The field under the main agent (its terminal or its conversation):
/// what is written goes to `send` as a message does, and the keyboard
/// stays for the next.
pub fn talk_bar(placeholder: impl Fn() -> String + 'static, send: impl Fn(String) + 'static, offers: Offers) -> impl IntoElement {
    let draft = global::<Unsent>();
    let send = Rc::new(move || {
        let words = draft.with_untracked(|d| d.0.trim_end().to_owned());
        if words.trim().is_empty() {
            return;
        }
        draft.set(Unsent(String::new()));
        send(words);
    });
    let none = move || draft.with(|d| d.0.trim().is_empty());
    let field = element_ref();
    let to_end = global::<ToEnd>();
    let moved = stored(to_end.get_untracked().0);
    effect(move || {
        let asked = to_end.get().0;
        if moved.replace(asked) != asked {
            // Once the field has the words.
            set_timeout(std::time::Duration::from_millis(30), move || {
                let end = draft.with_untracked(|d| d.0.encode_utf16().count()) as u32;
                field.set_selection(end, end);
            });
        }
    });
    let stop = offers.stop.clone();
    let stopping = computed(move || stop.as_ref().is_some_and(|(working, _)| working.get()) && none());
    view()
        .flex_row()
        .items_end()
        .gap(px(8.0))
        .px(px(12.0))
        .pt(px(8.0))
        .pb(px(8.0))
        .border_t(px(1.0))
        .border_color(theme::BORDER)
        .bg(theme::CHROME)
        .child(match offers.attach.clone() {
            None => view().into_element(),
            Some((busy, attach)) => view()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(px(20.0))
                .items_center()
                .justify_center()
                .active(|s| s.bg(theme::HOVER).scale(ui::PRESSED))
                .transition()
                .duration(ui::QUICK)
                .role(AccessibilityRole::Button)
                .label(t!("Send a picture"))
                .test_id("attach")
                .disabled(move || busy.get())
                .on_press(move || attach())
                .child(dynamic(move || if busy.get() {
                    ui::glyph("spinner", 18.0, theme::MUTED).animate_spin().into_element()
                } else {
                    ui::glyph("image", 22.0, theme::MUTED).into_element()
                }))
                .into_element(),
        })
        .child(
            input()
                .element_ref(&field)
                .test_id("talk")
                .multiline(true)
                .value(move || draft.with(|d| d.0.clone()))
                .on_change(move |t: &str| draft.set(Unsent(t.to_owned())))
                .placeholder(placeholder)
                .placeholder_color(move || theme::now(theme::DIM))
                .flex_1()
                .min_h(px(40.0))
                .max_h(move || px((window().height.0 * 0.25).clamp(64.0, 132.0)))
                .px(px(14.0))
                .py(px(9.0))
                .rounded(px(20.0))
                .border_1()
                .border_color(theme::INPUT)
                .bg(theme::BACKGROUND)
                .text_size(theme::BODY)
                .line_height(px(22.0))
                .text_color(theme::FOREGROUND),
        )
        // Send; while the agent works and nothing is written, stop it.
        .child(dynamic(move || {
            let round = || view()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(px(20.0))
                .items_center()
                .justify_center()
                .bg(theme::SOLID)
                .active(|s| s.opacity(0.7).scale(ui::PRESSED))
                .transition()
                .duration(ui::QUICK)
                .role(AccessibilityRole::Button);
            match offers.stop.clone().filter(|_| stopping.get()) {
                Some((_, stop)) => round()
                    .label(t!("Stop"))
                    .test_id("stop")
                    .on_press(move || stop())
                    .child(view().w(px(13.0)).h(px(13.0)).rounded(px(3.0)).bg(theme::SOLID_FOREGROUND))
                    .into_element(),
                None => {
                    let send = send.clone();
                    round()
                        .label(t!("Send"))
                        .disabled(none)
                        .disabled_style(|s| s.opacity(0.35))
                        .on_press(move || send())
                        .child(ui::glyph("arrow-up", 20.0, theme::SOLID_FOREGROUND))
                        .into_element()
                }
            }
        }))
}

/// How many rows a question shows at most.
const ASKED_ROWS: usize = 14;

/// What a program waiting for an answer asks: the bottom of its terminal
/// with something on it, drawn as the terminal draws it, watched as it is
/// (nothing resized; too wide, it scrolls across). Keys go to it through
/// the viewer given back, at once.
pub fn question(engine: Engine, address: &str) -> (View, Rc<Viewer>) {
    let viewer = {
        let _entered = link::runtime().enter();
        Rc::new(engine.viewer(address, Mode::Watch, untracked(theme::palette)))
    };
    let shown = signal(Arc::new(asked(&viewer.snapshot())));
    {
        let (viewer, mut changes) = (viewer.clone(), viewer.changes());
        spawn(async move {
            while changes.changed().await.is_ok() {
                next_frame().await;
                changes.borrow_and_update();
                shown.set(Arc::new(asked(&viewer.snapshot())));
            }
        });
    }
    let font = use_context::<Prefs>().expect("prefs").font;
    let cell = computed(move || Cell::at(font.get()));
    let width = signal(0.0f32);
    // The whole width in the card, never larger, nor too small to read.
    let scale = computed(move || {
        let (cols, w) = (shown.with(|s| s.cols) as f32, width.get());
        if w <= 0.0 || cols <= 0.0 { 1.0 } else { (w / (cols * cell.get().width)).clamp(0.6, 1.0) }
    });
    let background = computed(move || shown.with(|s| theme::rgb_of(s.background)));
    let drawn = view()
        .w_full()
        .rounded(px(10.0))
        .overflow_hidden()
        .bg(move || background.get())
        .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0 - 16.0))
        .test_id("question")
        .child(
            scroll()
                .horizontal()
                .scrollbar(false)
                .child(view().p(px(8.0)).child(terminal_rows(shown, cell, scale).pointer_events(PointerEvents::None))),
        );
    (drawn, viewer)
}

/// The rows of `s` a question shows: up to [`ASKED_ROWS`] ending at the
/// last with something on it, as wide as what is on them.
fn asked(s: &Snapshot) -> Snapshot {
    let empty = |r: &herdc_term::Row| r.runs.iter().all(|run| run.text.trim().is_empty() && run.style.bg.is_none());
    let end = s.lines.iter().rposition(|r| !empty(r)).map_or(0, |i| i + 1);
    let mut start = end.saturating_sub(ASKED_ROWS);
    while start < end && empty(&s.lines[start]) {
        start += 1;
    }
    let lines = s.lines[start..end].to_vec();
    let cols = lines
        .iter()
        .flat_map(|r| r.runs.iter().filter(|run| !run.text.trim_end().is_empty() || run.style.bg.is_some()).map(|run| run.col + run.width))
        .max()
        .unwrap_or(1);
    Snapshot { cols, rows: lines.len().max(1) as u16, lines, cursor: None, scrolled: 0, history: 0, background: s.background, foreground: s.foreground }
}

/// A message written whole before it goes in (more than a few words, a
/// pasted text, what the keyboard's voice typing heard): sent as one
/// paste, then Enter. Let go unsent, it is kept for next time.
fn compose(viewer: Rc<Viewer>, writing: Signal<bool>, draft: Signal<String>) -> impl IntoElement {
    // The terminal's: the sheet's own goes as it closes, its Enter with it.
    let owner = Owner::current();
    let send = move || {
        let words = draft.get_untracked();
        if words.trim().is_empty() {
            return;
        }
        viewer.paste(words.trim_end());
        let viewer = viewer.clone();
        owner.run(|| set_timeout(ENTER_AFTER, move || viewer.key(Key::Enter, Mods::NONE)));
        draft.set(String::new());
        writing.set(false);
        dismiss_keyboard();
    };
    // Let go, the keyboard goes with it: nothing is left to type in.
    let done = move || {
        writing.set(false);
        dismiss_keyboard();
    };
    let none = move || draft.with(|d| d.trim().is_empty());
    nsh_base::sheet()
        .open(move || Some(writing.get()))
        .close_on_swipe(true)
        .backdrop_class("bg-black/40")
        .on_close(done)
        .on_cancel(done)
        .child(when(move || writing.get(), move || {
            let send = send.clone();
            view().w_full().items_center().child(
                view()
                    .w_full()
                    .max_w(px(560.0))
                    .rounded_t(px(18.0))
                    .bg(theme::CHROME)
                    .pt(px(8.0))
                    .px(px(16.0))
                    .pb(move || ui::bottom_inset() + px(12.0))
                    .gap(px(10.0))
                    .child(view().w(px(36.0)).h(px(4.0)).rounded(px(2.0)).bg(theme::BORDER).self_center())
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .child(text(t!("Write a message")).flex_1().text_size(theme::FOOTNOTE).font_weight(FontWeight::SEMIBOLD).text_color(theme::MUTED))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px(px(10.0))
                                    .h(px(32.0))
                                    .rounded(px(8.0))
                                    .active(|s| s.bg(theme::HOVER))
                                    .role(AccessibilityRole::Button)
                                    .label(t!("Paste"))
                                    .on_press(move || {
                                        spawn(async move {
                                            let pasted = clipboard_string().await;
                                            if !pasted.is_empty() {
                                                draft.update(|d| d.push_str(&pasted));
                                            }
                                        });
                                    })
                                    .child(ui::glyph("clipboard", 15.0, theme::MUTED))
                                    .child(text(t!("Paste")).text_size(theme::CALLOUT).text_color(theme::MUTED)),
                            ),
                    )
                    .child(
                        input()
                            .test_id("compose")
                            .multiline(true)
                            .auto_focus(true)
                            .value(move || draft.get())
                            .on_change(move |t: &str| draft.set(t.to_owned()))
                            .placeholder(t!("Type, paste or speak it"))
                            .placeholder_color(move || theme::now(theme::DIM))
                            .min_h(px(96.0))
                            .max_h(move || px(window().height.0 * 0.4))
                            .px(px(12.0))
                            .py(px(10.0))
                            .rounded(px(12.0))
                            .border_1()
                            .border_color(theme::INPUT)
                            .bg(theme::BACKGROUND)
                            .text_size(theme::BODY)
                            .line_height(px(22.0))
                            .text_color(theme::FOREGROUND),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .child(text(t!("It goes in as one paste, then Enter.")).flex_1().text_size(theme::CAPTION).text_color(theme::DIM))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px(px(16.0))
                                    .h(px(40.0))
                                    .rounded(px(20.0))
                                    .bg(theme::SOLID)
                                    .active(|s| s.opacity(0.7))
                                    .role(AccessibilityRole::Button)
                                    .label(t!("Send"))
                                    .disabled(none)
                                    .disabled_style(|s| s.opacity(0.35))
                                    .on_press(send)
                                    .child(ui::glyph("send", 15.0, theme::SOLID_FOREGROUND))
                                    .child(text(t!("Send")).text_size(theme::ROW).font_weight(FontWeight::SEMIBOLD).text_color(theme::SOLID_FOREGROUND)),
                            ),
                    ),
            )
        }))
}

/// How many links a held terminal offers.
const LINKS: usize = 3;

/// The links nearest `row` (one it is on first), at most [`LINKS`].
fn nearest_links(mut links: Vec<herdc_term::Link>, row: u16) -> Vec<herdc_term::Link> {
    let away = |l: &herdc_term::Link| if (l.rows.0..=l.rows.1).contains(&row) { 0 } else { l.rows.0.abs_diff(row).min(l.rows.1.abs_diff(row)) };
    links.sort_by_key(|l| away(l));
    links.truncate(LINKS);
    links
}

/// How many files named on screen a held terminal offers.
const PATHS: usize = 2;

/// The page of a file the terminal at `address` names: from its pane's
/// folder (the space's, if it says none), at the line said with it.
fn file_route(address: &str, said: &str) -> Option<Route> {
    let live = crate::live::live()?;
    let (path, line) = herdc_term::file_place(said);
    let place = live.view.with_untracked(|v| {
        let space = v.active_space()?;
        let pane = space.tabs.iter().flat_map(|t| &t.panes).find(|p| [&p.terminal, &p.shown].iter().any(|t| t.as_deref() == Some(address)));
        let folder = pane.and_then(|p| p.cwd.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| space.folder.clone());
        Some(herdc_engine::tools::Place { on: space.machine.clone(), folder })
    })?;
    // From `/`, its own folder is the place.
    Some(match path.strip_prefix('/').and(path.rsplit_once('/')) {
        Some((folder, name)) => Route::File { place: herdc_engine::tools::Place { folder: folder.to_owned(), ..place }, path: name.to_owned(), line },
        None => Route::File { place, path, line },
    })
}

/// A file as a menu says it: as said, or, too long for a row, its name
/// and line with as many folders before them as fit.
fn short_path(said: &str) -> String {
    const ROOM: usize = 32;
    let said = said.trim_start_matches("./");
    if said.chars().count() <= ROOM {
        return said.to_owned();
    }
    let mut kept = String::new();
    for part in said.rsplit('/') {
        let longer = if kept.is_empty() { part.to_owned() } else { format!("{part}/{kept}") };
        if !kept.is_empty() && longer.chars().count() + 2 > ROOM {
            break;
        }
        kept = longer;
    }
    let n = kept.chars().count();
    if n + 2 > ROOM { format!("…{}", kept.chars().skip(n + 1 - ROOM).collect::<String>()) } else { format!("…/{kept}") }
}

/// A link as a menu says it: no scheme, and not too long.
fn short_url(url: &str) -> String {
    let bare = url.split_once("://").map_or(url, |(_, rest)| rest).trim_end_matches('/');
    if bare.chars().count() > 44 { format!("{}…", bare.chars().take(43).collect::<String>()) } else { bare.to_owned() }
}

/// Keep the native display list of an unchanged line when scrolling moves it.
/// Duplicate lines get distinct keys, so empty/repeated rows cannot disappear.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LineKey {
    row: herdc_term::Row,
    occurrence: usize,
}

fn line_keys(s: &Snapshot) -> Vec<LineKey> {
    let mut counts = std::collections::HashMap::new();
    s.lines.iter().map(|row| {
        let occurrence = counts.entry(row).or_insert(0);
        let key = LineKey { row: row.clone(), occurrence: *occurrence };
        *occurrence += 1;
        key
    }).collect()
}

fn terminal_rows(snapshot: Signal<Arc<Snapshot>>, cell: Memo<Cell>, scale: Memo<f32>) -> View {
    let grid = computed(move || snapshot.with(|s| (s.cols, s.rows, s.background, s.foreground)));
    view()
        .w(move || px(grid.get().0 as f32 * cell.get().width * scale.get()))
        .h(move || px(grid.get().1 as f32 * cell.get().height * scale.get()))
        .child(each(
            move || snapshot.with(|s| line_keys(s)),
            |line| line.clone(),
            move |line, index| {
                let cursor = computed(move || snapshot.with(|s| {
                    s.cursor.filter(|c| c.row as usize == index.get()).map(|mut c| { c.row = 0; c })
                }));
                canvas(move || {
                    let (cols, _, background, foreground) = grid.get();
                    let row = Snapshot {
                        cols, rows: 1, lines: vec![line.get().row], cursor: cursor.get(),
                        scrolled: 0, history: 0, background, foreground,
                    };
                    paint(&row, cell.get(), scale.get(), theme::rgb_of(theme::palette().cursor))
                })
                .absolute()
                .left(px(0.0))
                .top(move || px(index.get() as f32 * cell.get().height * scale.get()))
                .w(move || px(grid.get().0 as f32 * cell.get().width * scale.get()))
                .h(move || px(cell.get().height * scale.get()))
            },
        ))
}

/// A swipe let go at `vy` points a second goes on, slowing, a line at a
/// time, until it stops or another touch begins.
fn fling(viewer: Rc<Viewer>, snapshot: Signal<Arc<Snapshot>>, flung: Stored<u32>, vy: f32, at: Point<Points>, cell: Cell, scale: f32) {
    let this = flung.get().wrapping_add(1);
    flung.replace(this);
    let line = cell.height * scale;
    let (col, row) = ((at.x.0 / (cell.width * scale)) as u16, (at.y.0 / line) as u16);
    spawn(async move {
        let (mut v, mut lines) = (vy, 0.0f32);
        let mut then = std::time::Instant::now();
        while v.abs() > FLING_LEAST && flung.get() == this {
            next_frame().await;
            let dt = then.elapsed().as_secs_f32().min(0.05);
            then = std::time::Instant::now();
            lines += v * dt / line;
            let turn = lines.trunc() as i32;
            if turn != 0 {
                viewer.wheel(turn, col, row);
                snapshot.set(Arc::new(viewer.snapshot()));
                lines -= turn as f32;
            }
            v *= FLING_DECAY.powf(dt);
        }
    });
}

/// A hardware keyboard's keys, while the terminal has the keyboard: Ctrl
/// and Alt with letters, arrows, Esc, Tab and the F keys go to it as keys
/// (the field would move the focus, or select all). Ctrl+Q and F11 the
/// system keeps.
fn hardware_keys(viewer: Rc<Viewer>) {
    let mut keys: Vec<(String, Key, Mods)> = Vec::new();
    for c in ('a'..='z').filter(|c| *c != 'q').chain(['[', ']', '\\', '/']) {
        keys.push((format!("ctrl+{c}"), Key::Char(c), Mods::CTRL));
    }
    for c in ('a'..='z').chain('0'..='9') {
        keys.push((format!("alt+{c}"), Key::Char(c), Mods::ALT));
    }
    let named = [
        ("up", Key::Up),
        ("down", Key::Down),
        ("left", Key::Left),
        ("right", Key::Right),
        ("home", Key::Home),
        ("end", Key::End),
        ("pageup", Key::PageUp),
        ("pagedown", Key::PageDown),
        ("delete", Key::Delete),
        ("insert", Key::Insert),
        ("escape", Key::Escape),
        ("tab", Key::Tab),
    ];
    for (name, key) in named {
        for (held, mods) in [("", Mods::NONE), ("shift+", Mods::SHIFT), ("ctrl+", Mods::CTRL), ("alt+", Mods::ALT)] {
            keys.push((format!("{held}{name}"), key, mods));
        }
    }
    for n in (1..=12).filter(|n| *n != 11) {
        keys.push((format!("f{n}"), Key::F(n), Mods::NONE));
    }
    let options = KeyOptions { when: nsh_core::keymap::When::Always, repeat: true };
    for (stroke, key, mods) in keys {
        let viewer = viewer.clone();
        on_key_with(&stroke, options, move |_| viewer.key(key, mods));
    }
}

/// Back in history: how far, and a tap back to the latest output.
fn latest(viewer: Rc<Viewer>, snapshot: Signal<Arc<Snapshot>>) -> impl IntoElement {
    let back = computed(move || snapshot.with(|s| s.scrolled));
    when(move || back.get() > 0, move || {
        let viewer = viewer.clone();
        view()
            .absolute()
            .bottom(px(12.0))
            .right(px(12.0))
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(40.0))
            .px(px(14.0))
            .rounded(px(20.0))
            .bg(theme::POPOVER)
            .border_1()
            .border_color(theme::BORDER)
            .active(|s| s.bg(theme::HOVER))
            .role(AccessibilityRole::Button)
            .label(t!("Back to the latest output"))
            .on_press(move || viewer.scroll(-(back.get_untracked() as i32)))
            .child(text(move || t!("{} lines back", back.get())).text_size(theme::FOOTNOTE).text_color(theme::MUTED))
            .child(text("↓").text_size(theme::BODY).text_color(theme::FOREGROUND))
    })
}

/// Ended or lost: said over the screen.
fn status(info: Signal<Info>) -> impl IntoElement {
    let said = move || {
        let i = info.get();
        if let Some(ended) = &i.ended {
            return Some(match ended["exit_code"].as_i64() {
                Some(code) => t!("Ended (exit {})", code),
                None => t!("Ended").to_owned(),
            });
        }
        i.error
    };
    when(move || said().is_some(), move || {
        view()
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(12.0))
            .bg(theme::POPOVER)
            .border_1()
            .border_color(theme::BORDER)
            .pointer_events(PointerEvents::None)
            .child(text(move || said().unwrap_or_default()).text_size(theme::CAPTION).text_color(theme::MUTED))
    })
}

/// The hidden field the keyboard types into. It holds what was typed
/// since it was last cleared; changes go to the terminal as keys, and a
/// composition (pinyin, kana) goes once it is committed.
/// Focusing it, however it comes, takes the terminal to type in; it stays
/// taken (and fitted to the pane) when the keyboard goes.
fn keyboard_field(viewer: Rc<Viewer>, field: ElementRef, focused: Signal<bool>, typing: Signal<bool>, latch: Latch) -> impl IntoElement {
    let holds = signal(String::new());
    let sent = stored(String::new());
    let composing = stored(false);
    let flush = {
        let viewer = viewer.clone();
        move || {
            let now = holds.get_untracked();
            let (back, rest) = typed(&sent.replace(now.clone()), &now);
            for _ in 0..back {
                viewer.key(Key::Backspace, Mods::NONE);
            }
            if !rest.is_empty() {
                send_text(&viewer, latch, &rest);
            }
            if now.len() > 64 {
                holds.set(String::new());
                sent.replace(String::new());
            }
        }
    };
    let flush = Rc::new(flush);
    input()
        .test_id("terminal-input")
        .element_ref(&field)
        .absolute()
        .left(px(0.0))
        .bottom(px(0.0))
        .w(px(2.0))
        .h(px(2.0))
        .opacity(0.0)
        .caret(false)
        // A URL's keyboard: IMEs start it in English (no pinyin composing
        // `ls`), with `/` on it; switching to write a prompt still works.
        .keyboard_type(KeyboardType::Url)
        .auto_correct(false)
        .auto_capitalize(AutoCapitalize::None)
        .submit_behavior(SubmitBehavior::Submit)
        .return_key_type(ReturnKeyType::Default)
        .value(move || holds.get())
        .on_focus({
            let viewer = viewer.clone();
            move || {
                focused.set(true);
                typing.set(true);
                viewer.focus(true);
            }
        })
        .on_blur({
            let viewer = viewer.clone();
            move || {
                focused.set(false);
                viewer.focus(false);
            }
        })
        .on_change({
            let flush = flush.clone();
            move |t: &str| {
                holds.set(t.to_owned());
                if !composing.get() {
                    flush();
                }
            }
        })
        .on_composition_change({
            let flush = flush.clone();
            move |e: &Composition| {
                composing.replace(e.composing);
                if !e.composing {
                    flush();
                }
            }
        })
        .on_key_press({
            let viewer = viewer.clone();
            move |key: &str| {
                // An empty field has nothing to change: the key itself.
                if key == "Backspace" && holds.get_untracked().is_empty() {
                    let mods = latch.take();
                    viewer.key(Key::Backspace, mods);
                }
            }
        })
        .on_submit(move |_: &str| {
            flush();
            let mods = latch.take();
            viewer.key(Key::Enter, mods);
            holds.set(String::new());
            sent.replace(String::new());
        })
}

/// A terminal's text, its history too, to select with the system's own
/// handles; or all of it copied at once. Opens at the end, as the screen.
pub fn text_page(live: crate::live::Live, words: Arc<String>) -> nsh_core::element::Screen {
    let all = words.clone();
    let cell = Cell::at(use_context::<Prefs>().expect("prefs").font.get_untracked());
    let list = element_ref();
    set_timeout(std::time::Duration::from_millis(50), move || list.scroll_to_end(false));
    crate::ui::page(t!("Select text"))
        .header_right(
            view()
                .px(px(12.0))
                .h(px(40.0))
                .justify_center()
                .role(AccessibilityRole::Button)
                .label(t!("Copy all"))
                .on_press(move || {
                    set_clipboard(all.as_str());
                    live.say(herdc_engine::Note::info(t!("Copied"), t!("The terminal's text is on the clipboard.")));
                    nav::back();
                })
                .child(text(t!("Copy all")).text_size(theme::BODY).text_color(theme::ACCENT)),
        )
        .child(
            scroll().element_ref(&list).flex_1().bg(theme::BACKGROUND).child(
                view().selectable(true).p(px(12.0)).pb(move || crate::ui::bottom_inset() + px(12.0)).child(
                    text(words.as_str().to_owned())
                        .font_family(theme::MONO)
                        .text_size(px(cell.font))
                        .line_height(px(cell.height))
                        .text_color(theme::FOREGROUND),
                ),
            ),
        )
}

/// The keys a phone keyboard lacks: Enter at one end, Hide keyboard at
/// the other, the chosen ones scrolling between.
fn key_bar(viewer: Rc<Viewer>, latch: Latch, writing: Signal<bool>) -> impl IntoElement {
    let enter_viewer = viewer.clone();
    let keys = use_context::<Prefs>().expect("prefs").keys;
    view()
        .flex_row()
        .items_center()
        .h(px(44.0))
        .bg(theme::CHROME)
        .border_t_1()
        .border_color(theme::BORDER)
        // Always visible, including with an existing customized key bar.
        .child(bar_key("↵", move || enter_viewer.key(Key::Enter, latch.take())))
        .child(
            // Its keys are pressed with the keyboard up (a scroll view's
            // tap would otherwise only dismiss it).
            scroll()
                .horizontal()
                .flex_1()
                .scrollbar(false)
                .keyboard_taps(KeyboardTaps::Always)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .px(px(4.0))
                        .child(each(move || keys.get(), |id| id.clone(), move |id, _| bar_item(&id.get_untracked(), viewer.clone(), latch))),
                ),
        )
        .child(view().w(px(1.0)).h(px(24.0)).bg(theme::BORDER))
        .child(bar_key("✎", move || writing.set(true)))
        .child(bar_key("⌄", dismiss_keyboard))
}

/// Keys that type again while held.
fn repeats(key: Key) -> bool {
    matches!(key, Key::Left | Key::Right | Key::Up | Key::Down | Key::Delete | Key::PageUp | Key::PageDown | Key::Backspace)
}

/// Key `id` of the bar (Settings, Key bar), as it does.
fn bar_item(id: &str, viewer: Rc<Viewer>, latch: Latch) -> View {
    // One's own types its words, and Enter a moment after: at once, an
    // agent's prompt (Codex's) takes it as a pasted line's end.
    if let Some(words) = keybar::said(id) {
        let (words, enter) = match words.strip_suffix('\n') {
            Some(words) => (words.to_owned(), true),
            None => (words.to_owned(), false),
        };
        return bar_key(&keybar::face(id).unwrap_or_default(), move || {
            send_text(&viewer, latch, &words);
            if enter {
                let viewer = viewer.clone();
                set_timeout(ENTER_AFTER, move || viewer.key(Key::Enter, Mods::NONE));
            }
        });
    }
    let Some((shows, does)) = keybar::find(id) else { return view() };
    match does {
        Does::Key(key) if repeats(key) => repeating_key(shows, move || {
            let mods = latch.take();
            viewer.key(key, mods);
        }),
        Does::Key(key) => bar_key(shows, move || {
            let mods = latch.take();
            viewer.key(key, mods);
        }),
        Does::Ctrl => sticky_key(shows, latch.ctrl),
        Does::Alt => sticky_key(shows, latch.alt),
        Does::Text => bar_key(shows, move || send_text(&viewer, latch, shows)),
        Does::Paste => bar_key(t!("Paste"), move || {
            let viewer = viewer.clone();
            spawn(async move {
                let text = clipboard_string().await;
                if !text.is_empty() {
                    viewer.paste(&text);
                }
            });
        }),
    }
}

/// How long one's own key waits to press Enter after its words.
pub const ENTER_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

/// A key's face: what it shows, in `ink`.
fn key_face(label: &str, ink: impl Fn() -> Hsla + 'static) -> View {
    view()
        .min_w(px(42.0))
        .h(px(36.0))
        .px(px(8.0))
        .mx(px(1.0))
        .rounded(px(6.0))
        .items_center()
        .justify_center()
        .active(|s| s.bg(theme::HOVER))
        .role(AccessibilityRole::Button)
        .label(keybar::spoken(label))
        // The ends' signs are thin in the terminal's font: drawn as glyphs.
        .child(match label {
            "↵" => ui::glyph("corner-down-left", 18.0, theme::FOREGROUND).into_element(),
            "⌄" => ui::glyph("chevron-down", 18.0, theme::FOREGROUND).into_element(),
            "✎" => ui::glyph("pencil", 17.0, theme::FOREGROUND).into_element(),
            // One's own key that presses Enter after its words.
            _ if keybar::words(label).1 => view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(text(keybar::words(label).0.to_owned()).text_size(theme::CALLOUT).font_family(theme::MONO).text_color(ink))
                .child(ui::glyph("corner-down-left", 13.0, theme::MUTED))
                .into_element(),
            _ => text(label.to_owned()).text_size(theme::CALLOUT).font_family(theme::MONO).text_color(ink).into_element(),
        })
}

fn bar_key(label: &str, press: impl Fn() + 'static) -> View {
    key_face(label, || theme::now(theme::FOREGROUND)).on_press(press)
}

/// A key that types again and again while held (an arrow through a line).
fn repeating_key(label: &'static str, press: impl Fn() + 'static) -> View {
    let press = Rc::new(press);
    // Which hold is going (a new touch ends the one before), and whether
    // this touch repeated (its press is then not one more).
    let (turn, repeated) = (stored(0u32), stored(false));
    let (tap, hold) = (press.clone(), press);
    key_face(label, || theme::now(theme::FOREGROUND))
        .on_press_down(move || {
            turn.replace(turn.get().wrapping_add(1));
            repeated.replace(false);
        })
        .on_press_up(move || {
            turn.replace(turn.get().wrapping_add(1));
        })
        .on_press(move || {
            if !repeated.replace(false) {
                tap();
            }
        })
        .long_press_delay(REPEAT_AFTER.as_millis() as f32)
        .on_long_press(move || {
            repeated.replace(true);
            let (mine, hold) = (turn.get(), hold.clone());
            spawn(async move {
                // At most a while: a lost finger doesn't type forever.
                for _ in 0..400 {
                    if turn.get() != mine {
                        break;
                    }
                    hold();
                    timer(REPEAT_EVERY).await;
                }
            });
        })
}

/// Ctrl or Alt: a tap holds it for the next key, a second tap at once
/// until tapped again.
fn sticky_key(label: &'static str, hold: Signal<Hold>) -> View {
    let last = stored(None::<std::time::Instant>);
    key_face(label, move || match hold.get() {
        Hold::Off => theme::now(theme::FOREGROUND),
        Hold::Once => theme::now(theme::ACCENT),
        Hold::Locked => theme::now(theme::ACCENT_FOREGROUND),
    })
    .bg(move || match hold.get() {
        Hold::Off => Hsla::transparent_black(),
        Hold::Once => theme::now(theme::SELECTED),
        Hold::Locked => theme::now(theme::ACCENT),
    })
    .border_1()
    .border_color(move || if hold.get() == Hold::Once { theme::now(theme::ACCENT) } else { Hsla::transparent_black() })
    .accessibility_state(move || AccessibilityState { selected: hold.get() != Hold::Off, ..Default::default() })
    .on_press(move || {
        let quick = last.replace(Some(std::time::Instant::now())).is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400));
        hold.update(|h| {
            *h = match (*h, quick) {
                (Hold::Off, _) => Hold::Once,
                (Hold::Once, true) => Hold::Locked,
                _ => Hold::Off,
            }
        });
    })
}

/// The screen as drawing operations: backgrounds, then text, then the
/// cursor, at `scale`.
fn paint(s: &Snapshot, cell: Cell, scale: f32, cursor: Hsla) -> Painter {
    let (cw, ch) = (cell.width, cell.height);
    let color = theme::rgb_of;
    let mut p = Painter::new()
        .scale(scale, scale)
        .fill_color(color(s.background))
        .fill_rect(0.0, 0.0, s.cols as f32 * cw, s.rows as f32 * ch);
    for (y, row) in s.lines.iter().enumerate() {
        for run in row.runs.iter().filter(|r| r.style.bg.is_some()) {
            let bg = run.style.bg.unwrap();
            p = p.fill_color(color(bg)).fill_rect(run.col as f32 * cw, y as f32 * ch, run.width as f32 * cw, ch);
        }
    }
    if let Some(c) = s.cursor.filter(|c| c.kind == CursorKind::Block) {
        p = p.fill_color(cursor).fill_rect(c.col as f32 * cw, c.row as f32 * ch, cw, ch);
    }
    let font = |bold: bool, italic: bool| {
        format!("{}{}{}px {}", if italic { "italic " } else { "" }, if bold { "bold " } else { "" }, cell.font, theme::MONO)
    };
    let mut shown = (false, false);
    p = p.font(&font(false, false)).text_baseline(CanvasTextBaseline::Middle);
    for (y, row) in s.lines.iter().enumerate() {
        let mid = y as f32 * ch + ch / 2.0;
        for run in &row.runs {
            let st = run.style;
            if (st.bold, st.italic) != shown {
                shown = (st.bold, st.italic);
                p = p.font(&font(st.bold, st.italic));
            }
            let x = run.col as f32 * cw;
            let w = run.width as f32 * cw;
            let under_cursor = s.cursor.is_some_and(|c| c.kind == CursorKind::Block && c.row as usize == y && c.col == run.col && run.width <= 2);
            let fg = if under_cursor { color(s.background) } else { color(st.fg) };
            p = p.fill_color(fg).fill_text_max_width(&run.text, x, mid, px(w + cw * 0.5));
            if st.underline {
                p = p.fill_rect(x, y as f32 * ch + ch - 1.5, w, 1.0);
            }
            if st.strike {
                p = p.fill_rect(x, mid, w, 1.0);
            }
        }
    }
    if let Some(c) = s.cursor {
        let (x, y) = (c.col as f32 * cw, c.row as f32 * ch);
        p = match c.kind {
            CursorKind::Block => p,
            CursorKind::Beam => p.fill_color(cursor).fill_rect(x, y, 2.0, ch),
            CursorKind::Underline => p.fill_color(cursor).fill_rect(x, y + ch - 2.0, cw, 2.0),
            CursorKind::Hollow => p.stroke_color(cursor).line_width(1.0).stroke_rect(Bounds {
                origin: point(px(x + 0.5), px(y + 0.5)),
                size: size(px(cw - 1.0), px(ch - 1.0)),
            }),
        };
    }
    p
}

#[cfg(test)]
mod tests {
    use super::{typed, line_keys};

    #[test]
    fn scrolling_reuses_lines_without_collapsing_duplicates() {
        let mut screen = herdc_term::Screen::new(20, 4, herdc_term::Palette::default());
        screen.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let before = line_keys(&screen.snapshot());
        screen.scroll(1);
        let after = line_keys(&screen.snapshot());
        assert_eq!(&before[..3], &after[1..]);
        screen.feed(b"\x1b[2J");
        screen.scroll_to_bottom();
        let blank = line_keys(&screen.snapshot());
        assert_eq!(blank.iter().collect::<std::collections::HashSet<_>>().len(), 4);
    }


    #[test]
    fn links_nearest_the_finger_come_first_and_read_short() {
        let link = |url: &str, rows| herdc_term::Link { url: url.into(), rows };
        let all = vec![link("http://a", (0, 0)), link("http://b", (5, 6)), link("http://c", (9, 9)), link("http://d", (20, 20))];
        let near: Vec<_> = super::nearest_links(all, 6).into_iter().map(|l| l.url).collect();
        assert_eq!(near, ["http://b", "http://c", "http://a"]);
        assert_eq!(super::short_url("https://github.com/x/"), "github.com/x");
        assert_eq!(super::short_url(&format!("https://{}", "a".repeat(60))).chars().count(), 44);
        assert_eq!(super::short_path("./src/main.rs:4"), "src/main.rs:4");
        assert_eq!(super::short_path("/Users/me/workspace/yonder/app/src/terminal.rs:400:5"), "…/app/src/terminal.rs:400:5");
        assert_eq!(super::short_path(&format!("/a/{}.rs", "b".repeat(40))).chars().count(), 32);
    }

    #[test]
    fn typing_is_the_difference() {
        assert_eq!(typed("", "ls"), (0, "ls".into()));
        assert_eq!(typed("ls", "l"), (1, "".into()));
        assert_eq!(typed("teh", "the "), (2, "he ".into()));
        assert_eq!(typed("你", "你好"), (0, "好".into()));
    }
}
