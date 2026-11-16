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

/// Lines a screen keeps above itself, as many as its first paint brings.
/// More is memory a phone pays for each screen (some 5 MB a thousand
/// lines at 200 columns).
pub const SCROLLBACK: usize = 2000;

impl Screen {
    pub fn new(cols: u16, rows: u16, palette: Palette) -> Screen {
        let events = Listener::default();
        let config = Config { scrolling_history: SCROLLBACK, kitty_keyboard: true, osc52: Osc52::OnlyCopy, ..Default::default() };
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

    /// The links on screen as shown (scrolled back or not): web addresses
    /// in the text, wrapped lines joined, and the program's own (OSC 8);
    /// each with the rows it covers, top first, once each.
    pub fn links(&self) -> Vec<Link> {
        self.found(web_addresses, true)
    }

    /// The files named on screen as shown (`src/main.rs:42:7`, as a
    /// compiler or an agent says where), as [`links`](Self::links) are.
    pub fn file_paths(&self) -> Vec<Link> {
        self.found(file_paths, false)
    }

    /// What `find` finds in the screen's lines, wrapped ones joined, and
    /// with `programs`, the program's own links (OSC 8) too.
    fn found(&self, find: fn(&str) -> Vec<(usize, usize)>, programs: bool) -> Vec<Link> {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        let (rows, cols) = (grid.screen_lines(), grid.columns());
        let mut found: Vec<Link> = Vec::new();
        let mut add = |url: String, first: u16, last: u16| match found.iter_mut().find(|l| l.url == url) {
            Some(l) => l.rows.1 = l.rows.1.max(last),
            None => found.push(Link { url, rows: (first, last) }),
        };
        // A line as written: its text, and the row each char is on.
        let (mut text, mut at) = (String::new(), Vec::<u16>::new());
        for row in 0..rows {
            let line = &grid[Line(row as i32 - offset)];
            for col in 0..cols {
                let cell = &line[Column(col)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                if let Some(link) = cell.hyperlink().filter(|_| programs) {
                    add(link.uri().to_owned(), row as u16, row as u16);
                }
                text.push(cell.c);
                at.push(row as u16);
            }
            // A line goes on in the next row when it wrapped there, or, as
            // a screen redrawn row by row says nothing of wrapping, when it
            // fills its row.
            let last = &line[Column(cols - 1)];
            let goes_on = last.flags.contains(Flags::WRAPLINE) || !matches!(last.c, ' ' | '\0');
            if !goes_on || row + 1 == rows {
                for (start, end) in find(&text) {
                    let chars = text[..start].chars().count();
                    let len = text[start..end].chars().count();
                    add(text[start..end].to_owned(), at[chars], at[chars + len - 1]);
                }
                text.clear();
                at.clear();
            }
        }
        found.sort_by_key(|l| l.rows);
        found
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

/// A link on screen: where it goes, and its first and last rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub rows: (u16, u16),
}

/// Where web addresses are in `text` (byte ranges): `http://` or
/// `https://` up to a space, without what usually follows one in prose
/// (a full stop, a closing bracket it didn't open).
pub fn web_addresses(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = ["http://", "https://"].iter().filter_map(|p| text[from..].find(p)).min() {
        let start = from + found;
        let mut end = text[start..].find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '│' | '\u{0}')).map_or(text.len(), |e| start + e);
        loop {
            let url = &text[start..end];
            let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '*']);
            let unopened = |open: char, close: char| trimmed.ends_with(close) && trimmed.matches(close).count() > trimmed.matches(open).count();
            let trimmed = if unopened('(', ')') || unopened('[', ']') || unopened('{', '}') { &trimmed[..trimmed.len() - 1] } else { trimmed };
            if trimmed.len() == url.len() {
                break;
            }
            end = start + trimmed.len();
        }
        // More than the scheme: a host at least.
        if text[start..end].split_once("://").is_some_and(|(_, rest)| rest.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '[')) {
            out.push((start, end));
        }
        from = end.max(start + 1);
    }
    out
}

/// Where files are named in `text` (byte ranges): a path with a folder
/// or a line after it (`src/a.rs`, `a.rs:42`, `./a/b.py:7:3`), its name
/// with an extension; not a web address, nor one from the home folder
/// (whose home is the computer's to say).
pub fn file_paths(text: &str) -> Vec<(usize, usize)> {
    // Chinese and Japanese prose runs into a name without a space: its
    // letters end one.
    let wide = |c: char| matches!(c, '\u{2e80}'..='\u{a4cf}' | '\u{ac00}'..='\u{d7af}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ffef}');
    let part = |c: char| (c.is_alphanumeric() && !wide(c)) || matches!(c, '_' | '-' | '.' | '/' | '~' | '@' | '+' | ':');
    let mut out = Vec::new();
    let mut from = None;
    // Runs of what a path is made of, each a candidate (a space ends the last).
    for (end, c) in text.char_indices().chain([(text.len(), ' ')]) {
        if part(c) {
            from = from.or(Some(end));
            continue;
        }
        let Some(start) = from.take() else { continue };
        let run = text[start..end].trim_end_matches(['.', ':']);
        if run.contains("://") || run.starts_with('~') {
            continue;
        }
        // Then `:line` or `:line:column`, if any.
        let mut path = run;
        let mut line = false;
        for _ in 0..2 {
            if let Some((before, n)) = path.rsplit_once(':')
                && !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
            {
                path = before;
                line = true;
            }
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        let named = name.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.trim_start_matches('.').is_empty() && (1..=10).contains(&ext.len()) && ext.starts_with(|c: char| c.is_ascii_alphabetic()) && ext.bytes().all(|b| b.is_ascii_alphanumeric())
        });
        if named && !path.contains(':') && (line || path.contains('/')) {
            out.push((start, start + run.len()));
        }
    }
    out
}

/// A file named on screen: where under `folder` (or from `/`), and the
/// line (from 1) said with it.
pub fn file_place(said: &str) -> (String, Option<u32>) {
    let mut path = said;
    let mut numbers = Vec::new();
    while let Some((before, n)) = path.rsplit_once(':').filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
        numbers.push(n);
        path = before;
    }
    let line = numbers.last().and_then(|n| n.parse().ok()).filter(|n| *n > 0);
    (path.trim_start_matches("./").to_owned(), line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_paths_are_named_with_a_folder_or_a_line() {
        let found = |t: &str| file_paths(t).into_iter().map(|(a, b)| t[a..b].to_owned()).collect::<Vec<_>>();
        assert_eq!(found("  --> src/workspace.rs:161:21"), ["src/workspace.rs:161:21"]);
        assert_eq!(found("see main.rs:42. And ./a/b.py, too"), ["main.rs:42", "./a/b.py"]);
        assert_eq!(found("Edited /tmp/x/notes.md (3 lines)"), ["/tmp/x/notes.md"]);
        assert!(found("v0.167.1 example.com e.g. 1.5 ~/workspace/a.rs https://a.b/c.rs main.rs").is_empty(), "{:?}", found("v0.167.1 example.com e.g. 1.5 ~/workspace/a.rs https://a.b/c.rs main.rs"));
        assert_eq!(found("和 wiki/ubuntu.md重复，改了app/src/main.rs：好"), ["wiki/ubuntu.md", "app/src/main.rs"]);
        assert_eq!(found("café/menü.rs:3"), ["café/menü.rs:3"]);
        assert_eq!(file_place("./src/a.rs:42:7"), ("src/a.rs".to_owned(), Some(42)));
        assert_eq!(file_place("/x/b.py"), ("/x/b.py".to_owned(), None));
    }

    #[test]
    fn a_full_row_goes_on_in_the_next() {
        // As a screen redrawn row by row has it: no wrap marked.
        let mut s = Screen::new(12, 3, Palette::default());
        s.feed(b"at /a/bc/def\x1b[2;1Hg.rs:40 ok\x1b[3;1Hhttps://x");
        assert_eq!(s.file_paths(), [Link { url: "/a/bc/defg.rs:40".into(), rows: (0, 1) }]);
    }

    #[test]
    fn file_paths_on_screen_join_wrapped_lines() {
        let mut s = Screen::new(12, 4, Palette::default());
        s.feed(b"at crates/term/lib.rs:9 ok\r\n\x1b]8;;https://x.y/z.rs\x1b\\z\x1b]8;;\x1b\\");
        assert_eq!(s.file_paths(), [Link { url: "crates/term/lib.rs:9".into(), rows: (0, 1) }]);
    }

    #[test]
    fn web_addresses_lose_what_prose_puts_after_them() {
        let found = |t: &str| web_addresses(t).into_iter().map(|(a, b)| t[a..b].to_owned()).collect::<Vec<_>>();
        assert_eq!(found("see https://github.com/a/b/pull/7."), ["https://github.com/a/b/pull/7"]);
        assert_eq!(found("(at http://localhost:3000/x)"), ["http://localhost:3000/x"]);
        assert_eq!(found("https://en.wikipedia.org/wiki/Rust_(language), ok"), ["https://en.wikipedia.org/wiki/Rust_(language)"]);
        assert_eq!(found("[docs](https://docs.rs/x) and 'http://a.b'"), ["https://docs.rs/x", "http://a.b"]);
        assert!(found("http:// nothing https://").is_empty());
    }

    #[test]
    fn links_on_screen_join_wrapped_lines() {
        let mut s = Screen::new(20, 4, Palette::default());
        s.feed(b"go https://example.com/a/long/path now\r\nhttp://b.c");
        let links = s.links();
        assert_eq!(links, [
            Link { url: "https://example.com/a/long/path".into(), rows: (0, 1) },
            Link { url: "http://b.c".into(), rows: (2, 2) },
        ]);
        // A program's own link (OSC 8) is one too.
        s.feed(b"\r\n\x1b]8;;https://x.y/z\x1b\\here\x1b]8;;\x1b\\");
        assert!(s.links().iter().any(|l| l.url == "https://x.y/z"), "{:?}", s.links());
    }

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
