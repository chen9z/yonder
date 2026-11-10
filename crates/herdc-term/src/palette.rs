//! Terminal colors: a theme's 16, xterm's 256-color cube and grays.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(hex: &str) -> Option<Rgb> {
        let h = hex.strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        let n = u32::from_str_radix(h, 16).ok()?;
        Some(Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Two thirds of the way to `toward` kept: how dim text is drawn.
    pub fn mix(self, toward: Rgb, amount: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        Rgb(m(self.0, toward.0), m(self.1, toward.1), m(self.2, toward.2))
    }
}

/// A terminal theme, in the format of Herdr's `themes/*.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    pub ansi: [Rgb; 16],
}

impl Default for Palette {
    /// Herdr's shipped `atom-one-dark`.
    fn default() -> Self {
        let ansi = [
            "#454c59", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf", "#767676", "#e06c75",
            "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
        ];
        Palette {
            foreground: Rgb(0xab, 0xb2, 0xbf),
            background: Rgb(0x21, 0x25, 0x2b),
            cursor: Rgb(0xab, 0xb2, 0xbf),
            selection: Rgb(0x32, 0x38, 0x44),
            ansi: ansi.map(|h| Rgb::parse(h).unwrap()),
        }
    }
}

impl Palette {
    /// Index 0..=255 of the xterm palette.
    pub fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            232..=255 => {
                let v = 8 + (i - 232) * 10;
                Rgb(v, v, v)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xterm_cube_and_grays() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), Rgb(0, 0, 0));
        assert_eq!(p.indexed(196), Rgb(255, 0, 0));
        assert_eq!(p.indexed(231), Rgb(255, 255, 255));
        assert_eq!(p.indexed(232), Rgb(8, 8, 8));
        assert_eq!(p.indexed(1), Rgb::parse("#e06c75").unwrap());
        assert_eq!(Rgb(1, 2, 255).hex(), "#0102ff");
    }
}
