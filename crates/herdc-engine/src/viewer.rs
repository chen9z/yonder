//! One terminal on screen: attached to its core, its bytes fed to a
//! [`Screen`], keys sent back. It watches (an observer: the terminal keeps
//! the size the desktop gave it) or takes the terminal at a size, from the
//! start or once it types ([`crate::Engine::first_mode`]). A terminal
//! nobody holds (at the core's headless grid) is taken at the screen's
//! grid, and one another screen takes is watched again. A dropped line
//! reattaches by itself once the engine has a new one.

use herdc_term::{Key, Mods, Palette, Screen, Snapshot};
use crate::state::Here;
use crate::view::View;
use herdc_wire::{Attach, Client, Item, Stream};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{oneshot, watch};

/// Scrollback lines the first paint brings.
pub const HISTORY: u32 = 2000;
/// Output a viewer takes, in bytes a second: a flood past it is shown as
/// its screen, attached again once behind, not byte by byte (a phone's
/// data and battery; nobody reads that fast).
pub const PACE: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Observe: never type or resize.
    Watch,
    /// The active viewer, at this grid: the terminal takes it.
    Type { cols: u16, rows: u16 },
}

/// What a viewer knows besides the screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub attached: bool,
    pub title: Option<String>,
    /// The core ended the stream: `{"reason": "exited", "exit_code": 0, ...}`.
    pub ended: Option<Value>,
    pub bells: u64,
    /// The program copied this (OSC 52), last first.
    pub copied: Option<String>,
    pub error: Option<String>,
}

enum Keys {
    Typing(Arc<Stream>),
    /// Typed before a typing stream could take it.
    Waiting(Vec<u8>),
}

/// What a viewer needs from its engine.
pub(crate) struct Seat {
    pub tasks: crate::tasks::Tasks,
    pub clients: watch::Receiver<Option<Client>>,
    pub view: watch::Receiver<Arc<View>>,
    pub here: Arc<Mutex<Here>>,
    pub shown: watch::Receiver<bool>,
}

struct Shared {
    address: String,
    mode: watch::Sender<Mode>,
    /// The grid the screen has room for, once it says.
    fit: Mutex<Option<(u16, u16)>>,
    /// Taken at that grid once it is known, the once ([`Viewer::eager`]).
    eager: Mutex<bool>,
    here: Arc<Mutex<Here>>,
    screen: Mutex<Screen>,
    /// Where typing goes: the stream once it is attached to type (a
    /// watching one drops input), else what waits for it.
    keys: Mutex<Keys>,
    info: Mutex<Info>,
    changed: watch::Sender<u64>,
    /// When this screen last typed or scrolled.
    touched: Mutex<Option<std::time::Instant>>,
}

impl Shared {
    fn notify(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    fn info(&self, change: impl FnOnce(&mut Info)) {
        change(&mut self.info.lock().unwrap());
        self.notify();
    }

    fn set_mode(&self, mode: Mode) {
        self.mode.send_if_modified(|m| std::mem::replace(m, mode) != mode);
    }

    /// Watching a terminal at the core's headless grid, nobody types in it:
    /// the screen takes it at its own. So does an eager one, at any grid.
    fn claim(&self) {
        let fit = *self.fit.lock().unwrap();
        if let Some((cols, rows)) = fit
            && *self.mode.borrow() == Mode::Watch
            && std::mem::take(&mut *self.eager.lock().unwrap())
        {
            return self.set_mode(Mode::Type { cols, rows });
        }
        let headless = self.here.lock().unwrap().headless;
        let grid = self.screen.lock().map(|s| (s.cols(), s.rows())).unwrap();
        if let Some((cols, rows)) = fit
            && *self.mode.borrow() == Mode::Watch
            && headless == Some(grid)
            && self.info.lock().unwrap().attached
        {
            self.set_mode(Mode::Type { cols, rows });
        }
    }
}

pub struct Viewer {
    shared: Arc<Shared>,
    /// Dropped with the viewer: its stream ends.
    _gone: oneshot::Sender<()>,
}

impl Viewer {
    /// Starts showing `address`; must run inside the engine's tokio runtime.
    pub(crate) fn open(seat: Seat, address: impl Into<String>, mode: Mode, palette: Palette, display: Value) -> Viewer {
        let (cols, rows) = match mode {
            Mode::Watch => (80, 24),
            Mode::Type { cols, rows } => (cols, rows),
        };
        let shared = Arc::new(Shared {
            address: address.into(),
            mode: watch::Sender::new(mode),
            fit: Mutex::new(None),
            eager: Mutex::new(false),
            here: seat.here,
            screen: Mutex::new(Screen::new(cols, rows, palette)),
            keys: Mutex::new(Keys::Waiting(Vec::new())),
            info: Mutex::new(Info::default()),
            changed: watch::Sender::new(0),
            touched: Mutex::new(None),
        });
        let (gone, kept) = oneshot::channel();
        let pumping = pump(shared.clone(), seat.clients, seat.view, seat.shown, display);
        seat.tasks.spawn(async move {
            tokio::select! {
                _ = pumping => {}
                _ = kept => {}
            }
        });
        Viewer { shared, _gone: gone }
    }

    pub fn address(&self) -> &str {
        &self.shared.address
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.screen.lock().unwrap().snapshot()
    }

    pub fn with_screen<R>(&self, f: impl FnOnce(&mut Screen) -> R) -> R {
        f(&mut self.shared.screen.lock().unwrap())
    }

    pub fn info(&self) -> Info {
        self.shared.info.lock().unwrap().clone()
    }

    /// Bumped on every change worth a redraw.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.shared.changed.subscribe()
    }

    pub fn mode(&self) -> Mode {
        *self.shared.mode.borrow()
    }

    /// Typing at another grid resizes in place; watching and typing switch
    /// by attaching again.
    pub fn set_mode(&self, mode: Mode) {
        self.shared.set_mode(mode);
    }

    /// The grid the screen has room for: what it takes a terminal nobody
    /// holds at.
    pub fn fit(&self, cols: u16, rows: u16) {
        *self.shared.fit.lock().unwrap() = Some((cols, rows));
        self.shared.claim();
    }

    /// Taken at the grid the screen has room for as soon as it says, even
    /// from another screen: shown before the screen's grid was known, it
    /// could only watch. Once: another screen typing still takes it back.
    pub fn eager(&self) {
        *self.shared.eager.lock().unwrap() = true;
        self.shared.claim();
    }

    /// The terminal as big as it is now, taken to type in.
    pub fn take(&self) {
        if self.mode() == Mode::Watch {
            let (cols, rows) = self.with_screen(|s| (s.cols(), s.rows()));
            self.set_mode(Mode::Type { cols, rows });
        }
    }

    /// Bytes as typed. A watching viewer takes the terminal first; they go
    /// once it is attached.
    pub fn input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.touch();
        self.with_screen(|s| s.scroll_to_bottom());
        let waiting = match &mut *self.shared.keys.lock().unwrap() {
            Keys::Typing(stream) => {
                stream.input(bytes);
                false
            }
            Keys::Waiting(typed) => {
                typed.extend_from_slice(bytes);
                true
            }
        };
        if waiting {
            self.take();
        }
        self.shared.notify();
    }

    /// A key, encoded in the modes the program asked for.
    pub fn key(&self, key: Key, mods: Mods) {
        let bytes = self.with_screen(|s| s.key(key, mods));
        self.input(&bytes);
    }

    /// Text as a paste (bracketed when the program asked).
    pub fn paste(&self, text: &str) {
        let bytes = self.with_screen(|s| s.paste(text));
        self.input(&bytes);
    }

    /// Moves through history: positive goes back.
    pub fn scroll(&self, lines: i32) {
        self.touch();
        self.with_screen(|s| s.scroll(lines));
        self.shared.notify();
    }

    /// A wheel turn (a swipe) of `lines` at a cell, positive going back:
    /// the program's when it asked for the mouse (or for arrows on the
    /// alternate screen), else this screen's own history.
    pub fn wheel(&self, lines: i32, col: u16, row: u16) {
        match self.with_screen(|s| herdc_term::keys::wheel(lines, col, row, s.modes())) {
            Some(bytes) => self.input(&bytes),
            None => self.scroll(lines),
        }
    }

    /// A tap at a cell: a click for a program that asked for the mouse
    /// (true), else nothing.
    pub fn click(&self, col: u16, row: u16) -> bool {
        match self.with_screen(|s| herdc_term::keys::click(col, row, s.modes())) {
            Some(bytes) => {
                self.input(&bytes);
                true
            }
            None => false,
        }
    }

    /// How long since this screen typed or scrolled: what changes soon
    /// after is the answer, drawn at once.
    pub fn idle(&self) -> Duration {
        self.shared.touched.lock().unwrap().map_or(Duration::MAX, |t| t.elapsed())
    }

    fn touch(&self) {
        *self.shared.touched.lock().unwrap() = Some(std::time::Instant::now());
    }

    pub fn focus(&self, focused: bool) {
        if let Keys::Typing(stream) = &*self.shared.keys.lock().unwrap() {
            stream.focus(focused);
        }
    }
}

async fn pump(shared: Arc<Shared>, mut clients: watch::Receiver<Option<Client>>, mut view: watch::Receiver<Arc<View>>, mut shown: watch::Receiver<bool>, display: Value) {
    let mut mode = shared.mode.subscribe();
    let grid_in = |view: &View| {
        let t = view.terminal(&shared.address)?;
        Some((t["cols"].as_u64()? as u16, t["rows"].as_u64()? as u16))
    };
    // Back after a dropped line: the size is taken only once active again.
    let mut resumed = false;
    loop {
        let client = loop {
            let current = clients.borrow_and_update().clone();
            if *shown.borrow_and_update()
                && let Some(c) = current.filter(|c| !c.is_closed())
            {
                break c;
            }
            tokio::select! {
                r = clients.changed() => if r.is_err() { return },
                r = mode.changed() => if r.is_err() { return },
                r = shown.changed() => if r.is_err() { return },
            }
        };
        let m = *mode.borrow_and_update();
        let mut request = match m {
            Mode::Watch => {
                let (cols, rows) = shared.screen.lock().map(|s| (s.cols(), s.rows())).unwrap();
                let mut r = Attach::new(&shared.address, cols, rows);
                (r.observe, r.resize_notifications) = (true, true);
                r
            }
            Mode::Type { cols, rows } => Attach::new(&shared.address, cols, rows),
        };
        request.history = HISTORY;
        request.pace = Some(PACE);
        request.resumed = resumed;
        request.display = display.clone();
        let attached = tokio::select! {
            r = client.attach(request) => r,
            r = mode.changed() => { if r.is_err() { return } continue }
        };
        let attached = match attached {
            Ok(a) => a,
            Err(e) if e.kind() == Some("not_found") => {
                shared.info(|i| i.ended = Some(json!({"reason": "not_found"})));
                return;
            }
            Err(e) => {
                shared.info(|i| i.error = Some(e.to_string()));
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    r = mode.changed() => if r.is_err() { return },
                }
                continue;
            }
        };
        // A fresh paint follows: start from a clean screen of its grid.
        let grid = |k: &str, d: u16| attached.terminal[k].as_u64().map_or(d, |v| v as u16);
        let (cols, rows) = match m {
            Mode::Watch => (grid("cols", 80), grid("rows", 24)),
            Mode::Type { cols, rows } => (cols, rows),
        };
        {
            let mut screen = shared.screen.lock().unwrap();
            let fresh = Screen::new(cols, rows, screen.palette().clone());
            *screen = fresh;
        }
        let stream = Arc::new(attached.stream);
        // The grids this viewer gave the terminal: another is someone else's.
        let mut ours = match m {
            Mode::Watch => vec![],
            Mode::Type { cols, rows } => vec![(cols, rows)],
        };
        let mut seen = grid_in(&view.borrow_and_update());
        let held = (grid("cols", cols), grid("rows", rows));
        if m != Mode::Watch {
            // What waited goes first, under the lock new typing takes.
            let mut keys = shared.keys.lock().unwrap();
            if let Keys::Waiting(typed) = std::mem::replace(&mut *keys, Keys::Typing(stream.clone())) {
                stream.input(&typed);
            }
        }
        let title = attached.terminal["title"].as_str().map(|t| crate::view::shown_title(t).to_owned());
        shared.info(|i| (i.attached, i.error, i.title) = (true, None, title.or(i.title.take())));
        resumed = false;
        match m {
            // Back after a loss while another screen types: that one's.
            Mode::Type { .. } if !ours.contains(&held) => shared.set_mode(Mode::Watch),
            Mode::Watch => shared.claim(),
            _ => {}
        }
        let ended = loop {
            tokio::select! {
                item = stream.next() => {
                    let Some(item) = item else { break None };
                    match take(&shared, &stream, item) {
                        Some(end) => break Some(end),
                        None => {
                            // Whatever else arrived meanwhile, then one redraw.
                            let mut end = None;
                            while end.is_none() && let Some(more) = stream.try_next() {
                                end = take(&shared, &stream, more);
                            }
                            shared.notify();
                            if let Some(end) = end { break Some(end) }
                            // Outrun: attached again, at the screen as it is now.
                            if stream.behind() { break None }
                            if m == Mode::Watch {
                                shared.claim();
                            }
                        }
                    }
                }
                r = mode.changed() => {
                    if r.is_err() { return }
                    let now = *mode.borrow_and_update();
                    match (m, now) {
                        (Mode::Type { .. }, Mode::Type { cols, rows }) => {
                            ours.push((cols, rows));
                            stream.resize(cols, rows);
                            shared.screen.lock().unwrap().resize(cols, rows);
                            shared.notify();
                        }
                        _ => break None,
                    }
                }
                r = shown.changed() => {
                    if r.is_err() { return }
                    // Out of sight: let go, at this size (back, it's taken as it was).
                    if !*shown.borrow_and_update() {
                        resumed = matches!(m, Mode::Type { .. });
                        break None
                    }
                }
                r = view.changed() => {
                    if r.is_err() { return }
                    let now = grid_in(&view.borrow_and_update());
                    if now != seen {
                        seen = now;
                        // Another screen typed at its own grid: it has the terminal now.
                        if let (Mode::Type { .. }, Some(grid)) = (m, now)
                            && !ours.contains(&grid)
                        {
                            shared.set_mode(Mode::Watch);
                        }
                    }
                }
            }
        };
        {
            let mut keys = shared.keys.lock().unwrap();
            if matches!(*keys, Keys::Typing(_)) {
                *keys = Keys::Waiting(Vec::new());
            }
        }
        match ended {
            Some(End::Closed(reason)) => {
                shared.info(|i| (i.attached, i.ended) = (false, Some(reason)));
                return;
            }
            Some(End::Lost) => {
                resumed = matches!(*mode.borrow(), Mode::Type { .. });
                shared.info(|i| i.attached = false);
            }
            None => shared.info(|i| i.attached = false),
        }
    }
}

enum End {
    Closed(Value),
    Lost,
}

/// Applies one item; Some once the stream is over.
fn take(shared: &Shared, stream: &Stream, item: Item) -> Option<End> {
    match item {
        Item::Data(bytes) => {
            let fed = shared.screen.lock().unwrap().feed(&bytes);
            stream.automatic_input(&fed.replies);
            if fed.title.is_some() || fed.bell || fed.copied.is_some() {
                let mut info = shared.info.lock().unwrap();
                if let Some(title) = fed.title {
                    info.title = title.map(|t| crate::view::shown_title(&t).to_owned());
                }
                info.bells += fed.bell as u64;
                if fed.copied.is_some() {
                    info.copied = fed.copied;
                }
            }
            None
        }
        Item::Resize { cols, rows } => {
            shared.screen.lock().unwrap().resize(cols, rows);
            None
        }
        Item::Closed(reason) => Some(End::Closed(reason)),
        Item::Lost => Some(End::Lost),
    }
}
