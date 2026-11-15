//! Herdr's look: graphite in the dark, paper in the light, and the chrome
//! around them stepped from the terminal's background as herdr-app steps
//! it, so the phone and the desktop look like one app. Colors are
//! adaptive: the platform switches them with the appearance natively; a
//! canvas takes [`now`] of one.
//!
//! Adapted from herdr-app `src/settings.rs` (`GRAPHITE`, `PAPER`,
//! `resolve`), Apache-2.0.

use herdc_term::{Palette, Rgb};
use serde::{Deserialize, Serialize};
use nsh_core::color::{rgb, rgba};
use nsh_core::prelude::*;

const fn mix(a: u32, b: u32, t: f32) -> u32 {
    let mut out = 0;
    let mut shift = 0;
    while shift <= 16 {
        let (x, y) = (((a >> shift) & 255) as f32, ((b >> shift) & 255) as f32);
        out |= ((x * (1.0 - t) + y * t + 0.5) as u32) << shift;
        shift += 8;
    }
    out
}

const fn tone(light: u32, dark: u32) -> AdaptiveColor {
    AdaptiveColor { light: rgb(light).to_hsla(), dark: rgb(dark).to_hsla() }
}

const fn tone_a(light: u32, dark: u32) -> AdaptiveColor {
    AdaptiveColor { light: rgba(light).to_hsla(), dark: rgba(dark).to_hsla() }
}

const GRAPHITE_BG: u32 = 0x17191d;
const GRAPHITE_ANSI: [u32; 16] = [
    0x45484f, 0xdc8c95, 0xa6bd91, 0xd8be87, 0x96b2d7, 0xb5a2ce, 0x8cbdc3, 0xd2d4d9, 0x818793, 0xf0a5ad, 0xbad5a2, 0xead2a1, 0xb0cbef,
    0xcbb7e6, 0xa4d5db, 0xf0f0f2,
];
const PAPER_BG: u32 = 0xffffff;
const PAPER_ANSI: [u32; 16] = [
    0x25272c, 0xc62a2a, 0x1a7f37, 0x8f5d00, 0x1f62d0, 0x7a45c8, 0x0f6f74, 0x5f6068, 0x7b7e87, 0xd1242f, 0x1f883d, 0x9a6700, 0x0969da,
    0x8250df, 0x1b7c83, 0x111214,
];
/// The interface's text on each (not the terminal's).
const DARK_FG: u32 = 0xeeeeef;
const LIGHT_FG: u32 = 0x1b1b1e;
const WARM: u32 = 0xe8e8e2;

/// Behind terminals.
/// Light or dark as chosen in Settings: the phone's, unless one is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Look {
    #[default]
    System,
    Light,
    Dark,
}

impl Look {
    /// Asked of the platform: the colors here and its own controls follow.
    pub fn apply(self) {
        set_color_scheme(match self {
            Look::System => Appearance::System,
            Look::Light => Appearance::Light,
            Look::Dark => Appearance::Dark,
        });
    }
}

/// The status bar's icons, dark on light and light on dark: the
/// platform's own follow the phone, not the app.
pub fn status_bar() -> nsh_core::StatusBarSetting {
    nsh_core::StatusBarSetting::default().foreground(if dark() { nsh_core::StatusBarStyle::Light } else { nsh_core::StatusBarStyle::Dark })
}

pub const BACKGROUND: AdaptiveColor = tone(PAPER_BG, GRAPHITE_BG);
/// Bars and sidebars.
pub const CHROME: AdaptiveColor = tone(mix(PAPER_BG, WARM, 0.6), mix(GRAPHITE_BG, 0, 0.32));
/// Behind cards and lists.
pub const CANVAS: AdaptiveColor = tone(mix(PAPER_BG, WARM, 0.2), mix(GRAPHITE_BG, 0, 0.16));
/// Dialogs and sheets.
pub const POPOVER: AdaptiveColor = tone(PAPER_BG, mix(GRAPHITE_BG, 0xffffff, 0.06));
pub const RAISED: AdaptiveColor = tone(mix(PAPER_BG, 0, 0.035), mix(GRAPHITE_BG, 0xffffff, 0.09));
pub const HOVER: AdaptiveColor = tone_a(0x0000000c, 0xffffff0e);
pub const SELECTED: AdaptiveColor = tone_a(0x00000012, 0xffffff17);
pub const BORDER: AdaptiveColor = tone_a(0x00000017, 0xffffff18);
pub const INPUT: AdaptiveColor = tone_a(0x00000026, 0xffffff24);
pub const FOREGROUND: AdaptiveColor = tone(LIGHT_FG, DARK_FG);
pub const MUTED: AdaptiveColor = tone(mix(PAPER_BG, LIGHT_FG, 0.66), mix(GRAPHITE_BG, DARK_FG, 0.6));
pub const DIM: AdaptiveColor = tone(mix(PAPER_BG, LIGHT_FG, 0.46), mix(GRAPHITE_BG, DARK_FG, 0.38));
pub const ACCENT: AdaptiveColor = tone(PAPER_ANSI[4], GRAPHITE_ANSI[4]);
pub const ACCENT_FOREGROUND: AdaptiveColor = tone(PAPER_BG, GRAPHITE_BG);
/// The main action's button.
pub const SOLID: AdaptiveColor = tone(LIGHT_FG, DARK_FG);
pub const SOLID_FOREGROUND: AdaptiveColor = tone(0xffffff, mix(GRAPHITE_BG, 0, 0.28));
/// Lines a diff adds and removes, and their rows.
pub const ADDED: AdaptiveColor = tone(PAPER_ANSI[2], GRAPHITE_ANSI[2]);
pub const REMOVED: AdaptiveColor = tone(PAPER_ANSI[1], GRAPHITE_ANSI[1]);
pub const ADDED_ROW: AdaptiveColor = tone(mix(PAPER_BG, PAPER_ANSI[2], 0.12), mix(GRAPHITE_BG, GRAPHITE_ANSI[2], 0.14));
pub const REMOVED_ROW: AdaptiveColor = tone(mix(PAPER_BG, PAPER_ANSI[1], 0.12), mix(GRAPHITE_BG, GRAPHITE_ANSI[1], 0.14));
pub const WORKING: AdaptiveColor = tone(0x9a6700, 0xe5c07b);
pub const BLOCKED: AdaptiveColor = tone(0xc93c2c, 0xf0705f);
pub const DONE: AdaptiveColor = tone(0x2f6fe4, 0x76a8ff);
pub const IDLE: AdaptiveColor = tone(mix(PAPER_BG, LIGHT_FG, 0.38), mix(GRAPHITE_BG, DARK_FG, 0.38));

/// What an agent's status word shows as.
pub fn status(word: &str) -> AdaptiveColor {
    match word {
        "working" => WORKING,
        "blocked" => BLOCKED,
        "done" => DONE,
        _ => IDLE,
    }
}

/// One color of an adaptive pair for the appearance shown (reactive).
pub fn now(color: AdaptiveColor) -> Hsla {
    if dark() { color.dark } else { color.light }
}

/// The terminal palette for the appearance shown (reactive).
pub fn palette() -> Palette {
    let hex = |c: u32| Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    if dark() {
        Palette {
            foreground: hex(0xd2d4d9),
            background: hex(GRAPHITE_BG),
            cursor: hex(0xd2d4d9),
            selection: hex(0x394454),
            ansi: GRAPHITE_ANSI.map(hex),
        }
    } else {
        Palette {
            foreground: hex(0x25272c),
            background: hex(PAPER_BG),
            cursor: hex(0x25272c),
            selection: hex(0xcfdcf5),
            ansi: PAPER_ANSI.map(hex),
        }
    }
}

pub fn rgb_of(c: Rgb) -> Hsla {
    rgb(((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32).to_hsla()
}

/// The terminal's font family.
pub const MONO: &str = "monospace";

// The words' sizes, by what they are (after Apple's text styles, a step
// smaller: a phone of terminals). One role, one size, on every page.
/// A page's own title (home, the computers).
pub const TITLE: Points = px(20.0);
/// A sheet's or a dialog's title.
pub const HEADLINE: Points = px(18.0);
/// What is typed, and the choices and buttons on sheets.
pub const BODY: Points = px(16.0);
/// A list row's title.
pub const ROW: Points = px(15.0);
/// Words beside a title or inside a row; small buttons.
pub const CALLOUT: Points = px(14.0);
/// Notes, empty lists, what went wrong.
pub const FOOTNOTE: Points = px(13.0);
/// Under a row's title; headings, times, footers.
pub const CAPTION: Points = px(12.0);
/// Badges, counts, the bottom bar's labels.
pub const SMALL: Points = px(11.0);

/// A card's room from the screen's edge, and its corners.
pub const INSET: Points = px(16.0);
pub const CARD_RADIUS: Points = px(14.0);
