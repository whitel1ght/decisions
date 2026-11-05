//! Tokyo Night Storm, to match nvim, Ghostty and the lavender popup frames, and the icons.

use ratatui::style::Color;

use crate::app::Band;

pub const BG: Color = Color::Rgb(0x24, 0x28, 0x3b);
pub const TEXT: Color = Color::Rgb(0xc0, 0xca, 0xf5);
pub const MUTED: Color = Color::Rgb(0x56, 0x5f, 0x89);
pub const BLUE: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
pub const YELLOW: Color = Color::Rgb(0xe0, 0xaf, 0x68);
pub const ORANGE: Color = Color::Rgb(0xff, 0x9e, 0x64);
pub const RED: Color = Color::Rgb(0xf7, 0x76, 0x8e);
pub const LAVENDER: Color = Color::Rgb(0xbb, 0x9a, 0xf7);
pub const GREEN: Color = Color::Rgb(0x9e, 0xce, 0x6a);
pub const CYAN: Color = Color::Rgb(0x7d, 0xcf, 0xff);
pub const PINK: Color = Color::Rgb(0xff, 0x8f, 0xc7);
pub const SELECT: Color = Color::Rgb(0x2e, 0x3c, 0x64);
pub const CODE: Color = Color::Rgb(0x9a, 0xa5, 0xce);

pub fn band_color(b: Band) -> Color {
    match b {
        Band::Fresh => BLUE,
        Band::Warm => YELLOW,
        Band::Old => ORANGE,
        Band::Stale => RED,
    }
}

/// A session's colour, by the names Claude Code's `/color` takes.
pub fn session_color(name: &str) -> Option<Color> {
    Some(match name {
        "red" => RED,
        "orange" => ORANGE,
        "yellow" => YELLOW,
        "green" => GREEN,
        "cyan" => CYAN,
        "blue" => BLUE,
        "purple" => LAVENDER,
        "pink" => PINK,
        _ => return None,
    })
}

/// The glyphs the deck and `count --badge` draw: text ones, or Nerd Font (Font Awesome)
/// ones for terminals that have them. Key hints, ✓, → and the undo bar stay text in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icons {
    pub hand: &'static str,
    pub answer: &'static str,
    pub reply: &'static str,
    pub age: &'static str,
    pub star: &'static str,
    pub pen: &'static str,
    pub waiting: &'static str,
}

pub const TEXT_ICONS: Icons = Icons {
    hand: "✋",
    answer: "✉",
    reply: "↩",
    age: "⏱",
    star: "★",
    pen: "✎",
    waiting: "⧗",
};

pub const NERD_ICONS: Icons = Icons {
    hand: "\u{f256}",
    answer: "\u{f0e0}",
    reply: "\u{f112}",
    age: "\u{f017}",
    star: "\u{f005}",
    pen: "\u{f040}",
    waiting: "\u{f252}",
};

impl Icons {
    /// `nerd` for the Nerd Font set; anything else is the text set.
    pub fn named(name: &str) -> Self {
        match name.trim() {
            "nerd" => NERD_ICONS,
            _ => TEXT_ICONS,
        }
    }
}

impl Default for Icons {
    fn default() -> Self {
        TEXT_ICONS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_band_has_its_colour() {
        assert_eq!(
            [Band::Fresh, Band::Warm, Band::Old, Band::Stale].map(band_color),
            [BLUE, YELLOW, ORANGE, RED]
        );
    }

    #[test]
    fn every_colour_the_deck_assigns_can_be_drawn() {
        for name in crate::colors::ASSIGNED {
            assert!(session_color(name).is_some(), "{name}");
        }
        assert_eq!(session_color("purple"), Some(LAVENDER));
        assert_eq!(session_color("default"), None);
    }

    #[test]
    fn icons_by_name_default_to_text() {
        assert_eq!(Icons::named("nerd"), NERD_ICONS);
        assert_eq!(Icons::named(" nerd "), NERD_ICONS);
        for other in ["text", "", "emoji", "Nerd"] {
            assert_eq!(Icons::named(other), TEXT_ICONS, "{other:?}");
        }
        assert_eq!(Icons::default(), TEXT_ICONS);
        assert_eq!(NERD_ICONS.star, "\u{f005}");
    }
}
