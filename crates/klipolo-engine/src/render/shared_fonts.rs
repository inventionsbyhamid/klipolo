//! The compositor's one font system, shared by the text and caption drawers. In the preview it starts
//! with the managed faces only (built in milliseconds, every built-in style's face included) and takes the scanned
//! template once the background scan (`TextDrawer::scan_fonts_in_background`, on its own thread) is done, then follows
//! the font generation: the render thread never waits on the system fonts. An export (`wait`) scans before frame 0,
//! on the worker's own thread, as it did before this task.
//!
//! Memory: one `FontSystem` per compositor (its database is a clone of the template's face records; faces are
//! memory-mapped once used), where the text drawer and a caption drawer would each have held one.

use glyphon::FontSystem;

use crate::vendor::cap::rendering::layers::{
    fallback_font_system, fonts_scanned, new_font_system_at, template_generation,
};

pub struct SharedFonts {
    system: FontSystem,
    scanned: bool,
    generation: u64,
}

impl SharedFonts {
    /// `wait` (export): run the blocking scan now if no one has, as the export's `TextDrawer` did. The preview
    /// never waits: it starts with the managed faces and swaps in the template once the background scan is done.
    pub fn new(wait: bool) -> Self {
        let scanned = wait || fonts_scanned();
        // With `wait`, the scan runs (blocking) when the template isn't built yet, else it is cloned.
        let (system, generation) = build(scanned);
        Self {
            system,
            scanned,
            generation,
        }
    }

    pub fn system(&mut self) -> &mut FontSystem {
        &mut self.system
    }

    /// Whether this computer's own fonts are in the system yet (a text in an installed font waits for them).
    pub fn scanned(&self) -> bool {
        self.scanned
    }

    /// Called once per frame before any text is drawn. `true`: the faces changed, every cached layout and atlas goes.
    /// It never scans itself.
    pub fn refresh(&mut self) -> bool {
        let now_scanned = fonts_scanned();
        let generation = template_generation();
        if (now_scanned && !self.scanned) || generation != self.generation {
            // Only clones the finished template here: `fonts_scanned()` is true, so it never scans.
            (self.system, self.generation) = build(now_scanned);
            self.scanned = now_scanned;
            return true;
        }
        false
    }
}

/// A font system, and the fonts' generation its faces are from: the scanned template's own, or
/// for the managed faces the generation read before they were registered. A font registered while it was built then
/// shows as a newer generation at the next `refresh`, never goes unseen.
fn build(scanned: bool) -> (FontSystem, u64) {
    if scanned {
        new_font_system_at()
    } else {
        let generation = template_generation();
        (fallback_font_system(), generation)
    }
}

/// Tests: how long the background font scan waits before it starts (`delay_background_scan_for_tests`),
/// in milliseconds.
#[cfg(any(test, feature = "test-support"))]
static SCAN_DELAY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tests: the background font scan every compositor starts (`TextDrawer::scan_fonts_in_background`) waits `delay`
/// first, so only a font system made waiting scans meanwhile. Once a process: a test that needs it has its own binary.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn delay_background_scan_for_tests(delay: std::time::Duration) {
    let ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
    SCAN_DELAY_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/// The background scan's wait (`delay_background_scan_for_tests`; none outside tests).
pub(crate) fn background_scan_delay() -> std::time::Duration {
    #[cfg(any(test, feature = "test-support"))]
    {
        std::time::Duration::from_millis(SCAN_DELAY_MS.load(std::sync::atomic::Ordering::Relaxed))
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        std::time::Duration::ZERO
    }
}

/// Tests: whether this computer's fonts were scanned in this process.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn fonts_scanned_for_tests() -> bool {
    fonts_scanned()
}
