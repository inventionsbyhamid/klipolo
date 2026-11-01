//! How a take records the cursor: decided before Record, from what this computer can do and the
//! ship gate, never from a device (no monitor, no cursor API here: the consent scan's capture list pins it).

use klipolo_model::coded::CodedError;
use klipolo_model::project::RecordingKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::backend::CursorCaps;
use crate::english::msg;

/// How a screen source records the cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CursorPlan {
    /// No cursor at all (a game whose profile doesn't ask for it).
    #[default]
    None,
    /// The system draws it into the frames; the cursor file records positions and clicks.
    Burned,
    /// No cursor in the frames: Klipolo draws it from the cursor file (the watchdog burns it in if following fails).
    Separate,
}

/// The ship gate: on for macOS (its cursor source records the cursor as its own layer); off on
/// Windows and Linux until their sources ship, where only a preview build's `separate-cursor` feature turns it on.
/// Tests never turn the feature on, so CI on Linux plans with it off.
pub const SEPARATE_ENABLED: bool = cfg!(target_os = "macos") || cfg!(feature = "separate-cursor");

/// Games: Burned when the game's profile asks for the cursor, else None. Screens: Separate when the gate is
/// on and the platform hides the cursor, gives positions and shapes; else Burned. Camera-only takes record none.
pub fn cursor_plan(kind: RecordingKind, caps: &CursorCaps, profile_cursor: bool) -> CursorPlan {
    match kind {
        RecordingKind::Gameplay if profile_cursor => CursorPlan::Burned,
        RecordingKind::Gameplay | RecordingKind::Camera => CursorPlan::None,
        RecordingKind::Screen if SEPARATE_ENABLED && separable(caps) => CursorPlan::Separate,
        RecordingKind::Screen => CursorPlan::Burned,
    }
}

/// `capture.preflight.cursorBurned`: the build draws cursors, but this computer can't keep a screen's cursor apart
/// from its frames, so it stays in the picture. None while the gate is off (Windows and Linux: every take keeps it in
/// the picture).
pub fn cursor_note(caps: &CursorCaps) -> Option<CodedError> {
    (SEPARATE_ENABLED && !separable(caps)).then(|| msg!("capture.preflight.cursorBurned"))
}

fn separable(caps: &CursorCaps) -> bool {
    caps.hide && caps.positions && caps.shapes
}

#[cfg(test)]
mod tests {
    use super::*;
    use klipolo_cursor::ClickSource;

    /// What a Mac can do: hide the cursor from the frames, its positions, its shapes (`platform::macos`'s caps).
    fn separable_caps() -> CursorCaps {
        CursorCaps {
            hide: true,
            positions: true,
            shapes: true,
            clicks: ClickSource::Events,
        }
    }

    /// macOS records a screen's cursor as its own layer, and no note says it stays in the picture.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_gate_is_on_for_macos() {
        // `black_box`: the gate is a constant, and this pins its value on this OS.
        assert!(std::hint::black_box(SEPARATE_ENABLED), "on for macOS");
        let plan = cursor_plan(RecordingKind::Screen, &separable_caps(), true);
        assert_eq!(plan, CursorPlan::Separate);
        assert_eq!(cursor_note(&separable_caps()), None);
    }

    /// Elsewhere (Windows and Linux, CI included) the gate stays off until their sources ship: every screen take keeps
    /// the cursor in its picture, and no note says so.
    #[cfg(not(any(target_os = "macos", feature = "separate-cursor")))]
    #[test]
    fn the_gate_stays_off_elsewhere() {
        // `black_box`: the gate is a constant, and this pins its value on this OS.
        assert!(
            !std::hint::black_box(SEPARATE_ENABLED),
            "off on Windows and Linux"
        );
        let plan = cursor_plan(RecordingKind::Screen, &separable_caps(), true);
        assert_eq!(plan, CursorPlan::Burned);
        assert_eq!(cursor_note(&separable_caps()), None);
    }
}
