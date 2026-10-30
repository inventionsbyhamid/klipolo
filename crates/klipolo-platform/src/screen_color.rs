//! The eyedropper: the person presses it, then clicks anywhere on the screen, and that colour is
//! picked.
//! - It needs no Screen Recording permission: the system draws the loupe, outside the app, and hands back only the
//!   colour clicked. Klipolo reads no pixels, takes no screenshot and records nothing.
//! - It runs only on the person's press (the capture rule: a press is an explicit
//!   start, and the eyedropper isn't capture). Nothing samples on its own, and it is no tool: chat, plugins and MCP
//!   apps can't start it.
//! - macOS: `NSColorSampler`. Windows gets the webview's `EyeDropper` in W, and Linux has none: both say it isn't
//!   supported, so the UI shows no eyedropper there.

/// This computer has an eyedropper.
pub fn supported() -> bool {
    crate::imp::screen_color_supported()
}

/// Shows the eyedropper (macOS: call it on the main thread). It returns at once; `done` later gets the colour clicked
/// as sRGB `#RRGGBB`, or None when the person cancelled, one is already showing, or this computer has none. `done`
/// runs exactly once, unless the app quits while the loupe shows: AppKit then still holds it, and it never runs.
pub fn sample(done: Box<dyn FnOnce(Option<String>) + Send + 'static>) {
    crate::imp::sample_screen_color(done)
}

/// `#RRGGBB` for sRGB components in 0–1: each is clamped to 0–1, then rounded to 0–255.
pub fn hex(r: f64, g: f64, b: f64) -> String {
    let byte = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::hex;
    #[test]
    fn hex_rounds_and_clamps_components() {
        assert_eq!(hex(1.0, 0.839, 0.039), "#FFD60A");
        assert_eq!(hex(-0.2, 1.4, 0.5), "#00FF80");
    }
}
