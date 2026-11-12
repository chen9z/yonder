//! One terminal's screen as herdc draws it: alacritty's VT fed with the
//! bytes of a core terminal's attach stream, read back as rows of styled
//! runs that a canvas paints cell by cell.

pub mod keys;
mod palette;

pub use keys::{Key, Mods, Modes};
pub use palette::{Palette, Rgb};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor, StdSyncHandler};
use alacritty_terminal::Term;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

/// What feeding bytes produced besides the screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fed {
    /// Answers to the program's queries: send back as automatic input.
    pub replies: Vec<u8>,
    /// The title changed (None: reset).
    pub title: Option<Option<String>>,
    pub bell: bool,
    /// The program copied this (OSC 52).
    pub copied: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Rgb,
    /// None: the screen's background.
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Cells in one row drawn as one piece of text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Run {
    pub col: u16,
    /// Cells it covers (a wide character covers two).
    pub width: u16,
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Row {
    pub runs: Vec<Run>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Block,
    Underline,
    Beam,
    Hollow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    pub kind: CursorKind,
}

/// What is on screen now, ready to paint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<Row>,
    pub cursor: Option<Cursor>,
    /// Lines scrolled back into history (0: following the output).
    pub scrolled: usize,
    pub history: usize,
    pub background: Rgb,
    pub foreground: Rgb,
}

impl Snapshot {
    /// The visible text, one line per row, trailing spaces trimmed.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|r| {
                let mut line = String::new();
                for run in &r.runs {
                    while line.chars().count() < run.col as usize {
                        line.push(' ');
                    }
                    line.push_str(&run.text);
                }
                line.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub struct Screen {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    events: Listener,
    palette: Palette,
    /// Cell size in pixels, for programs that ask.
    cell: (u16, u16),
}

impl Screen {
    pub fn new(cols: u16, rows: u16, palette: Palette) -> Screen {
        let events = Listener::default();
        let config = Config { scrolling_history: 10_000, kitty_keyboard: true, osc52: Osc52::OnlyCopy, ..Default::default() };
        let size = Size { cols: cols.max(1) as usize, rows: rows.max(1) as usize };
        Screen { term: Term::new(config, &size, events.clone()), parser: Processor::new(), events, palette, cell: (9, 18) }
    }

    pub fn cols(&self) -> u16 {
        self.term.columns() as u16
    }

    pub fn rows(&self) -> u16 {
        self.term.screen_lines() as u16
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    pub fn set_cell_size(&mut self, width: u16, height: u16) {
        self.cell = (width.max(1), height.max(1));
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Fed {
        self.parser.advance(&mut self.term, bytes);
        let events = std::mem::take(&mut *self.events.0.lock().unwrap());
        let mut fed = Fed::default();
        for event in events {
            match event {
                Event::PtyWrite(text) => fed.replies.extend(text.into_bytes()),
                Event::Title(title) => fed.title = Some(Some(title)),
                Event::ResetTitle => fed.title = Some(None),
                Event::Bell => fed.bell = true,
                Event::ClipboardStore(_, text) => fed.copied = Some(text),
                Event::ColorRequest(index, format) => {
                    let Rgb(r, g, b) = self.color_at(index);
                    fed.replies.extend(format(alacritty_terminal::vte::ansi::Rgb { r, g, b }).into_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    let size = WindowSize {
                        num_lines: self.rows(),
                        num_cols: self.cols(),
                        cell_width: self.cell.0,
                        cell_height: self.cell.1,
                    };
                    fed.replies.extend(format(size).into_bytes());
                }
                _ => {}
            }
        }
        fed
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) != (self.cols(), self.rows()) {
            self.term.resize(Size { cols: cols.max(1) as usize, rows: rows.max(1) as usize });
        }
    }

    /// Moves through history: positive goes back.
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn modes(&self) -> Modes {
        let m = *self.term.mode();
        let mut kitty = 0;
        for (flag, bit) in [
            (TermMode::DISAMBIGUATE_ESC_CODES, keys::kitty::DISAMBIGUATE),
            (TermMode::REPORT_EVENT_TYPES, keys::kitty::EVENT_TYPES),
            (TermMode::REPORT_ALTERNATE_KEYS, keys::kitty::ALTERNATE_KEYS),
            (TermMode::REPORT_ALL_KEYS_AS_ESC, keys::kitty::ALL_AS_ESCAPES),
            (TermMode::REPORT_ASSOCIATED_TEXT, keys::kitty::ASSOCIATED_TEXT),
        ] {
            if m.contains(flag) {
                kitty |= bit;
            }
        }
        Modes {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            kitty,
            mouse: m.intersects(TermMode::MOUSE_MODE),
            sgr_mouse: m.contains(TermMode::SGR_MOUSE),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            alternate_scroll: m.contains(TermMode::ALTERNATE_SCROLL),
            focus_events: m.contains(TermMode::FOCUS_IN_OUT),
        }
    }

    /// Keys a person pressed, encoded for the program's modes.
    pub fn key(&self, key: Key, mods: Mods) -> Vec<u8> {
        keys::encode(key, mods, self.modes())
    }

    pub fn paste(&self, text: &str) -> Vec<u8> {
        keys::paste(text, self.modes())
    }

    fn color_at(&self, index: usize) -> Rgb {
        if let Some(c) = self.term.colors()[index] {
            return Rgb(c.r, c.g, c.b);
        }
        match index {
            0..=255 => self.palette.indexed(index as u8),
            i if i == NamedColor::Foreground as usize => self.palette.foreground,
            i if i == NamedColor::Background as usize => self.palette.background,
            i if i == NamedColor::Cursor as usize => self.palette.cursor,
            i if i == NamedColor::BrightForeground as usize => self.palette.foreground,
            i if i == NamedColor::DimForeground as usize => self.palette.foreground.mix(self.palette.background, 0.33),
            i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
                let base = i - NamedColor::DimBlack as usize;
                self.color_at(base).mix(self.palette.background, 0.33)
            }
            _ => self.palette.foreground,
        }
    }

    fn resolve(&self, color: Color) -> Rgb {
        match color {
            Color::Spec(c) => Rgb(c.r, c.g, c.b),
            Color::Indexed(i) => self.color_at(i as usize),
            Color::Named(n) => self.color_at(n as usize),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let content = self.term.renderable_content();
        let (cols, rows) = (self.cols(), self.rows());
        let offset = content.display_offset as i32;
        let mut lines = vec![Row::default(); rows as usize];
        let background = self.color_at(NamedColor::Background as usize);
        let foreground = self.color_at(NamedColor::Foreground as usize);
        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let row = indexed.point.line.0 + offset;
            let Some(line) = usize::try_from(row).ok().and_then(|r| lines.get_mut(r)) else { continue };
            let col = indexed.point.column.0 as u16;
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let (mut fg, mut bg) = (self.resolve(cell.fg), self.resolve(cell.bg));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::DIM) {
                fg = fg.mix(bg, 0.33);
            }
            let hidden = cell.flags.contains(Flags::HIDDEN);
            let style = Style {
                fg,
                bg: (bg != background).then_some(bg),
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                strike: cell.flags.contains(Flags::STRIKEOUT),
            };
            // ASCII shares a run; anything else stands alone at its own cell,
            // so fonts with other advances can't push the grid out of line.
            let simple = !wide && cell.c.is_ascii() && cell.zerowidth().is_none();
            let width = if wide { 2 } else { 1 };
            match line.runs.last_mut() {
                Some(run) if simple && run.style == style && run.col + run.width == col && run.text.is_ascii() && run.width as usize == run.text.len() => {
                    run.text.push(if hidden { ' ' } else { cell.c });
                    run.width += 1;
                }
                _ => {
                    let mut text = String::new();
                    text.push(if hidden { ' ' } else { cell.c });
                    if let Some(extra) = cell.zerowidth() {
                        text.extend(extra);
                    }
                    line.runs.push(Run { col, width, text, style });
                }
            }
        }
        // Blank default cells draw nothing.
        for line in &mut lines {
            for run in &mut line.runs {
                if run.style.bg.is_none() && !run.style.underline && !run.style.strike && run.text.is_ascii() {
                    let kept = run.text.trim_end_matches(' ').len();
                    run.width -= (run.text.len() - kept) as u16;
                    run.text.truncate(kept);
                }
            }
            line.runs.retain(|r| r.width > 0 && (r.style.bg.is_some() || r.style.underline || r.style.strike || !r.text.trim().is_empty()));
        }
        let shown = content.mode.contains(TermMode::SHOW_CURSOR) && content.display_offset == 0;
        let cursor = (shown && content.cursor.shape != CursorShape::Hidden)
            .then(|| {
                let row = content.cursor.point.line.0 + offset;
                let kind = match content.cursor.shape {
                    CursorShape::Underline => CursorKind::Underline,
                    CursorShape::Beam => CursorKind::Beam,
                    CursorShape::HollowBlock => CursorKind::Hollow,
                    _ => CursorKind::Block,
                };
                u16::try_from(row).ok().map(|row| Cursor { row, col: content.cursor.point.column.0 as u16, kind })
            })
            .flatten();
        Snapshot {
            cols,
            rows,
            lines,
            cursor,
            scrolled: content.display_offset,
            history: self.term.grid().history_size(),
            background,
            foreground,
        }
    }

    /// The visible text (for copying and tests).
    pub fn text(&self) -> String {
        self.snapshot().text()
    }

    /// The screen and up to `history` lines above it, wrapped lines joined,
    /// trailing blanks gone: what a person copies from.
    pub fn text_with_history(&self, history: usize) -> String {
        let grid = self.term.grid();
        let top = grid.history_size().min(history) as i32;
        let start = Point::new(Line(-top), Column(0));
        let end = Point::new(Line(grid.screen_lines() as i32 - 1), Column(grid.columns() - 1));
        let text = self.term.bounds_to_string(start, end);
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        let used = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
        lines[..used].join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_with_history_joins_wrapped_lines() {
        let mut s = Screen::new(10, 3, Palette::default());
        s.feed(b"one\r\ntwo\r\nthree\r\nabcdefghijklmno\r\n");
        assert_eq!(s.text_with_history(100), "one\ntwo\nthree\nabcdefghijklmno");
        assert_eq!(s.text_with_history(0), "abcdefghijklmno", "the screen holds both halves");
        assert_eq!(s.text_with_history(1), "three\nabcdefghijklmno");
    }

    fn screen() -> Screen {
        Screen::new(20, 4, Palette::default())
    }

    #[test]
    fn text_lands_in_rows_and_styles_split_runs() {
        let mut s = screen();
        s.feed(b"hello \x1b[1;31mred\x1b[0m!\r\nnext");
        let snap = s.snapshot();
        assert_eq!(snap.text(), "hello red!\nnext\n\n");
        let runs = &snap.lines[0].runs;
        assert_eq!(runs[0].text, "hello");
        assert_eq!((runs[1].text.as_str(), runs[1].col, runs[1].style.bold), ("red", 6, true));
        assert_eq!(runs[1].style.fg, Palette::default().ansi[1]);
        assert_eq!(snap.cursor, Some(Cursor { row: 1, col: 4, kind: CursorKind::Block }));
    }

    #[test]
    fn wide_characters_take_two_cells_and_stand_alone() {
        let mut s = screen();
        s.feed("a中b".as_bytes());
        let runs = &s.snapshot().lines[0].runs;
        assert_eq!(runs.iter().map(|r| (r.col, r.width, r.text.as_str())).collect::<Vec<_>>(), [(0, 1, "a"), (1, 2, "中"), (3, 1, "b")]);
    }

    #[test]
    fn colors_inverse_and_truecolor() {
        let mut s = screen();
        s.feed(b"\x1b[38;2;1;2;3;48;5;196mx\x1b[0m\x1b[7my");
        let runs = &s.snapshot().lines[0].runs;
        assert_eq!(runs[0].style.fg, Rgb(1, 2, 3));
        assert_eq!(runs[0].style.bg, Some(Rgb(255, 0, 0)));
        assert_eq!(runs[1].style.fg, Palette::default().background);
        assert_eq!(runs[1].style.bg, Some(Palette::default().foreground));
    }

    #[test]
    fn queries_are_answered_and_titles_reported() {
        let mut s = screen();
        let fed = s.feed(b"\x1b]2;build\x07\x1b[6n\x07");
        assert_eq!(fed.title, Some(Some("build".into())));
        assert!(fed.bell);
        assert_eq!(fed.replies, b"\x1b[1;1R");
        let fed = s.feed(b"\x1b]11;?\x07");
        assert_eq!(String::from_utf8(fed.replies).unwrap(), "\x1b]11;rgb:2121/2525/2b2b\x07");
    }

    #[test]
    fn modes_follow_the_program() {
        let mut s = screen();
        assert_eq!(s.key(Key::Up, Mods::NONE), b"\x1b[A");
        // Kitty flags belong to a screen: these are the alternate screen's.
        s.feed(b"\x1b[?1049h\x1b[?1h\x1b[?2004h\x1b[>1u\x1b[?1000h\x1b[?1006h");
        let m = s.modes();
        assert!(m.app_cursor && m.bracketed_paste && m.alt_screen && m.mouse && m.sgr_mouse);
        assert_eq!(m.kitty, keys::kitty::DISAMBIGUATE);
        assert_eq!(s.key(Key::Up, Mods::NONE), b"\x1bOA");
        assert_eq!(s.key(Key::Escape, Mods::NONE), b"\x1b[27u");
        assert_eq!(s.paste("x"), b"\x1b[200~x\x1b[201~");
    }

    #[test]
    fn history_scrolls_back_and_follows_again() {
        let mut s = screen();
        for i in 0..10 {
            s.feed(format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(s.snapshot().history, 7);
        s.scroll(3);
        let snap = s.snapshot();
        assert_eq!(snap.scrolled, 3);
        assert!(snap.text().starts_with("line 4"), "{}", snap.text());
        assert_eq!(snap.cursor, None);
        s.scroll_to_bottom();
        assert!(s.snapshot().text().starts_with("line 7"));
    }

    #[test]
    fn resizing_reflows_the_grid() {
        let mut s = screen();
        s.feed(b"abcdefghij");
        s.resize(5, 4);
        assert_eq!((s.cols(), s.rows()), (5, 4));
        // The wrapped head goes into history; the cursor's line stays.
        assert!(s.text().starts_with("fghij"));
        assert_eq!(s.snapshot().history, 1);
    }
}
