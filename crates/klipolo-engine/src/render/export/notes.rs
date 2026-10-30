//! What an export draws reduced and reports instead of stopping: a motion graphic
//! past its runtime caps is a property of the scene, drawn the same in preview and export on every computer, with no
//! setting the person could change to draw more. Each is kept once, with the first frame's time, and the export job
//! records it (`JobHandle::note`) so the finished export says it.

use klipolo_model::time::Us;

use super::FormatTargets;
use crate::api::{
    EXPORT_SCENE_REDUCED, EXPORT_SCENE_STILL, EXPORT_SCENE_TEXT_CUT, EXPORT_SCENE_TOO_DETAILED,
    EXPORT_SCENE_TOO_MANY_GROUPS, FRAME_SCENE_REDUCED, FRAME_SCENE_STILL, FRAME_SCENE_TEXT_CUT,
    FRAME_SCENE_TOO_DETAILED, FRAME_SCENE_TOO_MANY_GROUPS,
};

/// The note an export reports for a frame's note `warning`, if it is one it draws through.
fn reported_as(warning: &str) -> Option<&'static str> {
    match warning {
        FRAME_SCENE_REDUCED => Some(EXPORT_SCENE_REDUCED),
        FRAME_SCENE_STILL => Some(EXPORT_SCENE_STILL),
        FRAME_SCENE_TOO_DETAILED => Some(EXPORT_SCENE_TOO_DETAILED),
        FRAME_SCENE_TEXT_CUT => Some(EXPORT_SCENE_TEXT_CUT),
        FRAME_SCENE_TOO_MANY_GROUPS => Some(EXPORT_SCENE_TOO_MANY_GROUPS),
        _ => None,
    }
}

/// Whether the frame at `at_us` draws through `warning` (kept in `notes`, once per code, to report).
pub(super) fn reported(notes: &mut Vec<(String, Us)>, warning: &str, at_us: Us) -> bool {
    let Some(code) = reported_as(warning) else {
        return false;
    };
    if !notes.iter().any(|(c, _)| c == code) {
        notes.push((code.to_owned(), at_us));
    }
    true
}

impl FormatTargets {
    /// The notes since the last call: each code once, with the time of the first frame that drew it.
    pub fn take_notes(&mut self) -> Vec<(String, Us)> {
        std::mem::take(&mut self.notes)
    }
}
