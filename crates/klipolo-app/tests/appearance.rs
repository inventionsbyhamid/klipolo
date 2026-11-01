//! The window's own colour, behind the see-through page: the canvas of the theme in use, the person's pick or else the
//! system's appearance, and the same colours as the page's tokens.

use klipolo_app::appearance::{self, CANVAS_DARK, CANVAS_LIGHT};
use klipolo_app::prefs::Theme;

#[test]
fn the_window_takes_the_canvas_of_the_theme_in_use() {
    // System follows the system's appearance.
    assert!(appearance::is_dark(Theme::System, true));
    assert!(!appearance::is_dark(Theme::System, false));
    // Light and Dark win over it.
    for system_dark in [false, true] {
        assert!(!appearance::is_dark(Theme::Light, system_dark));
        assert!(appearance::is_dark(Theme::Dark, system_dark));
    }
    assert_eq!(appearance::canvas(Theme::Light, true), [0xE1, 0xE7, 0xEF]);
    assert_eq!(appearance::canvas(Theme::Dark, false), [0x07, 0x0A, 0x14]);
    assert_eq!(appearance::canvas(Theme::System, false), [0xE1, 0xE7, 0xEF]);
}

/// The page's `--color-night` (`light-dark(light, dark)`) is the window's colour in each theme, so the gaps between the
/// panels and the window around them are one colour.
#[test]
fn the_canvas_colours_are_the_page_tokens() {
    let tokens = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../apps/desktop/src/styles/tokens.css"
    ))
    .unwrap();
    let line = tokens
        .lines()
        .find(|l| l.trim_start().starts_with("--color-night:"))
        .expect("--color-night");
    let hex: Vec<u32> = line
        .split('#')
        .skip(1)
        .map(|part| u32::from_str_radix(&part[..6], 16).unwrap())
        .collect();
    assert_eq!(hex, [CANVAS_LIGHT, CANVAS_DARK], "{line}");
}
