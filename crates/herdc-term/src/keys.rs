//! Keys → the bytes a real terminal sends, in the modes the program set:
//! application cursor keys, bracketed paste, the kitty keyboard protocol
//! (core restores a program's flags on attach) and mouse wheel reports.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A character as typed (shift already applied: `A`, `!`).
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1..=F12.
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { shift: false, alt: false, ctrl: false };
    pub const CTRL: Mods = Mods { shift: false, alt: false, ctrl: true };
    pub const ALT: Mods = Mods { shift: false, alt: true, ctrl: false };
    pub const SHIFT: Mods = Mods { shift: true, alt: false, ctrl: false };

    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }

    /// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

/// Kitty keyboard flags (progressive enhancement).
pub mod kitty {
    pub const DISAMBIGUATE: u8 = 1;
    pub const EVENT_TYPES: u8 = 2;
    pub const ALTERNATE_KEYS: u8 = 4;
    pub const ALL_AS_ESCAPES: u8 = 8;
    pub const ASSOCIATED_TEXT: u8 = 16;
}

/// What the program asked the terminal for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// Kitty keyboard flags ([`kitty`]).
    pub kitty: u8,
    /// The program wants mouse reports (clicks at least).
    pub mouse: bool,
    pub sgr_mouse: bool,
    pub alt_screen: bool,
    /// On the alternate screen, the wheel sends arrow keys.
    pub alternate_scroll: bool,
    pub focus_events: bool,
}

pub fn encode(key: Key, mods: Mods, modes: Modes) -> Vec<u8> {
    let kitty = modes.kitty & (kitty::DISAMBIGUATE | kitty::ALL_AS_ESCAPES) != 0;
    let all = modes.kitty & kitty::ALL_AS_ESCAPES != 0;
    let csi_u = |code: u32, mods: Mods| -> Vec<u8> {
        if mods.any() { format!("\x1b[{code};{}u", mods.param()) } else { format!("\x1b[{code}u") }.into_bytes()
    };
    match key {
        Key::Char(c) => {
            if kitty && (all || mods.ctrl || mods.alt) {
                // Kitty reports the unshifted key; shift stays a modifier.
                let base = if mods.shift { c.to_lowercase().next().unwrap_or(c) } else { c };
                return csi_u(base as u32, mods);
            }
            let mut out = Vec::new();
            if mods.alt {
                out.push(0x1b);
            }
            match (mods.ctrl, control(c)) {
                (true, Some(b)) => out.push(b),
                _ => out.extend(c.to_string().as_bytes()),
            }
            out
        }
        Key::Escape if kitty => csi_u(27, mods),
        Key::Enter if kitty && (all || mods.any()) => csi_u(13, mods),
        Key::Tab if kitty && (all || mods.any()) => csi_u(9, mods),
        Key::Backspace if kitty && (all || mods.any()) => csi_u(127, mods),
        Key::Escape => alt_prefixed(mods, b"\x1b"),
        Key::Enter => alt_prefixed(mods, b"\r"),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefixed(mods, b"\t"),
        Key::Backspace if mods.ctrl => alt_prefixed(mods, b"\x08"),
        Key::Backspace => alt_prefixed(mods, b"\x7f"),
        Key::Up => cursor(b'A', mods, modes),
        Key::Down => cursor(b'B', mods, modes),
        Key::Right => cursor(b'C', mods, modes),
        Key::Left => cursor(b'D', mods, modes),
        Key::Home => cursor(b'H', mods, modes),
        Key::End => cursor(b'F', mods, modes),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::F(n @ 1..=4) => {
            let letter = b"PQRS"[n as usize - 1];
            if mods.any() { format!("\x1b[1;{}{}", mods.param(), letter as char).into_bytes() } else { vec![0x1b, b'O', letter] }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5], mods),
        Key::F(_) => Vec::new(),
    }
}

/// The C0 byte Ctrl makes of a character, as xterm does.
fn control(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' | 'A'..='Z' => (c as u8) & 0x1f,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

fn alt_prefixed(mods: Mods, bytes: &[u8]) -> Vec<u8> {
    let mut out = if mods.alt { vec![0x1b] } else { Vec::new() };
    out.extend_from_slice(bytes);
    out
}

fn cursor(letter: u8, mods: Mods, modes: Modes) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[1;{}{}", mods.param(), letter as char).into_bytes()
    } else if modes.app_cursor {
        vec![0x1b, b'O', letter]
    } else {
        vec![0x1b, b'[', letter]
    }
}

fn tilde(code: u8, mods: Mods) -> Vec<u8> {
    if mods.any() { format!("\x1b[{code};{}~", mods.param()) } else { format!("\x1b[{code}~") }.into_bytes()
}

/// Pasted text: bracketed when the program asked, line ends as Enter. Any
/// ESC inside is dropped so a paste can't end the bracket early.
pub fn paste(text: &str, modes: Modes) -> Vec<u8> {
    let body: String = text.replace("\r\n", "\r").replace('\n', "\r").chars().filter(|c| *c != '\x1b').collect();
    if modes.bracketed_paste { format!("\x1b[200~{body}\x1b[201~").into_bytes() } else { body.into_bytes() }
}

/// A wheel turn of `lines` (positive: up) at a cell: what to send the
/// program, or None to scroll this screen's own history.
pub fn wheel(lines: i32, col: u16, row: u16, modes: Modes) -> Option<Vec<u8>> {
    if lines == 0 {
        return None;
    }
    let up = lines > 0;
    let n = lines.unsigned_abs() as usize;
    if modes.mouse {
        let button = if up { 64 } else { 65 };
        let one = if modes.sgr_mouse {
            format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
        } else {
            let (x, y) = ((col + 33).min(255) as u8, (row + 33).min(255) as u8);
            vec![0x1b, b'[', b'M', (button + 32) as u8, x, y]
        };
        return Some(one.repeat(n));
    }
    if modes.alt_screen && modes.alternate_scroll {
        let key = if modes.app_cursor { if up { b"\x1bOA" } else { b"\x1bOB" } } else if up { b"\x1b[A" } else { b"\x1b[B" };
        return Some(key.repeat(n));
    }
    None
}

/// A tap at a cell as a left click, pressed and let go: what to send the
/// program, or None when it didn't ask for the mouse.
pub fn click(col: u16, row: u16, modes: Modes) -> Option<Vec<u8>> {
    if !modes.mouse {
        return None;
    }
    if modes.sgr_mouse {
        let (x, y) = (col + 1, row + 1);
        return Some(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").into_bytes());
    }
    let (x, y) = ((col + 33).min(255) as u8, (row + 33).min(255) as u8);
    Some(vec![0x1b, b'[', b'M', 32, x, y, 0x1b, b'[', b'M', 35, x, y])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: Vec<u8>) -> String {
        String::from_utf8(b).unwrap()
    }

    #[test]
    fn keys_encode_like_herdr_cores_own_table() {
        // herdr-core keys.rs: the same names, the same bytes.
        let m = Modes::default();
        let cases = [
            (Key::Enter, Mods::NONE, "\r"),
            (Key::Escape, Mods::NONE, "\x1b"),
            (Key::Tab, Mods::NONE, "\t"),
            (Key::Backspace, Mods::NONE, "\x7f"),
            (Key::Char('a'), Mods::NONE, "a"),
            (Key::Char('A'), Mods::SHIFT, "A"),
            (Key::Char('c'), Mods::CTRL, "\x03"),
            (Key::Char('x'), Mods::ALT, "\x1bx"),
            (Key::Up, Mods::NONE, "\x1b[A"),
            (Key::F(1), Mods::NONE, "\x1bOP"),
            (Key::Char(' '), Mods::NONE, " "),
        ];
        for (key, mods, want) in cases {
            assert_eq!(s(encode(key, mods, m)), want, "{key:?} {mods:?}");
        }
        let app = Modes { app_cursor: true, ..m };
        assert_eq!(s(encode(Key::Up, Mods::NONE, app)), "\x1bOA");
        assert_eq!(s(encode(Key::Left, Mods::NONE, app)), "\x1bOD");
        let kitty = Modes { kitty: kitty::DISAMBIGUATE, ..m };
        assert_eq!(s(encode(Key::Escape, Mods::NONE, kitty)), "\x1b[27u");
        assert_eq!(s(encode(Key::Char('\\'), Mods::CTRL, kitty)), "\x1b[92;5u");
    }

    #[test]
    fn modifiers_on_special_keys() {
        let m = Modes::default();
        let ctrl_shift = Mods { ctrl: true, shift: true, alt: false };
        assert_eq!(s(encode(Key::Right, ctrl_shift, m)), "\x1b[1;6C");
        assert_eq!(s(encode(Key::Tab, Mods::SHIFT, m)), "\x1b[Z");
        assert_eq!(s(encode(Key::PageDown, Mods::NONE, m)), "\x1b[6~");
        assert_eq!(s(encode(Key::Delete, Mods::ALT, m)), "\x1b[3;3~");
        assert_eq!(s(encode(Key::F(5), Mods::NONE, m)), "\x1b[15~");
        assert_eq!(s(encode(Key::F(2), Mods::CTRL, m)), "\x1b[1;5Q");
        assert_eq!(s(encode(Key::Char('['), Mods::CTRL, m)), "\x1b");
        assert_eq!(s(encode(Key::Char('?'), Mods::CTRL, m)), "\x7f");
        let all = Modes { kitty: kitty::DISAMBIGUATE | kitty::ALL_AS_ESCAPES, ..m };
        assert_eq!(s(encode(Key::Enter, Mods::NONE, all)), "\x1b[13u");
        assert_eq!(s(encode(Key::Char('a'), Mods::NONE, all)), "\x1b[97u");
        assert_eq!(s(encode(Key::Char('A'), Mods::SHIFT, all)), "\x1b[97;2u");
        let kitty = Modes { kitty: kitty::DISAMBIGUATE, ..m };
        assert_eq!(s(encode(Key::Enter, Mods::NONE, kitty)), "\r");
        assert_eq!(s(encode(Key::Enter, Mods::SHIFT, kitty)), "\x1b[13;2u");
        assert_eq!(s(encode(Key::Char('a'), Mods::NONE, kitty)), "a");
    }

    #[test]
    fn pastes_bracket_and_cannot_escape() {
        let m = Modes::default();
        assert_eq!(s(paste("a\nb", m)), "a\rb");
        let b = Modes { bracketed_paste: true, ..m };
        assert_eq!(s(paste("x\x1b[201~y\r\n", b)), "\x1b[200~x[201~y\r\x1b[201~");
    }

    #[test]
    fn the_wheel_goes_where_the_program_wants_it() {
        let m = Modes::default();
        assert_eq!(wheel(2, 0, 0, m), None);
        let sgr = Modes { mouse: true, sgr_mouse: true, ..m };
        assert_eq!(s(wheel(1, 4, 9, sgr).unwrap()), "\x1b[<64;5;10M");
        assert_eq!(s(wheel(-1, 0, 0, sgr).unwrap()), "\x1b[<65;1;1M");
        let alt = Modes { alt_screen: true, alternate_scroll: true, ..m };
        assert_eq!(s(wheel(-2, 0, 0, alt).unwrap()), "\x1b[B\x1b[B");
    }

    #[test]
    fn a_tap_clicks_only_where_the_program_wants_the_mouse() {
        let m = Modes::default();
        assert_eq!(click(3, 4, m), None);
        let sgr = Modes { mouse: true, sgr_mouse: true, ..m };
        assert_eq!(s(click(3, 4, sgr).unwrap()), "\x1b[<0;4;5M\x1b[<0;4;5m");
        let x10 = Modes { mouse: true, ..m };
        assert_eq!(click(0, 0, x10).unwrap(), b"\x1b[M !!\x1b[M#!!");
    }
}
