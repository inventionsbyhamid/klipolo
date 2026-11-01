//! The window's own colour, which shows through the see-through page around the panels (and through the preview's
//! hole for a frame or two while the window resizes): the canvas of the theme in use, `--color-night` in the page's
//! tokens. The theme is the person's pick in Settings (`prefs::Theme`), or the system's appearance while that is
//! System.

use crate::prefs::Theme;

/// `--color-night` in the light theme (0xRRGGBB).
pub const CANVAS_LIGHT: u32 = 0xE1E7EF;
/// `--color-night` in the dark theme (0xRRGGBB).
pub const CANVAS_DARK: u32 = 0x070A14;

/// Whether `theme` shows dark, the system's appearance being dark or not.
pub fn is_dark(theme: Theme, system_dark: bool) -> bool {
    match theme {
        Theme::System => system_dark,
        Theme::Light => false,
        Theme::Dark => true,
    }
}

/// The window's colour for `theme` as red, green and blue.
pub fn canvas(theme: Theme, system_dark: bool) -> [u8; 3] {
    let rgb = if is_dark(theme, system_dark) {
        CANVAS_DARK
    } else {
        CANVAS_LIGHT
    };
    let [_, r, g, b] = rgb.to_be_bytes();
    [r, g, b]
}
