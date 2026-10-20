//! Tokyo Night Storm, to match nvim, Ghostty and the lavender popup frames.

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
}
