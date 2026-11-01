//! The cursor shapes an export range draws: what export and headless renders preload into
//! the GPU atlas before frame 1 (`CursorPass::preload`), so their cursor is the real shape from the first frame.

use std::sync::Arc;

use klipolo_model::cursor_style::{CursorCapture, CursorLook};
use klipolo_model::project::Project;
use klipolo_model::query::clip_starts;
use klipolo_model::time::{Us, unscaled};
use klipolo_model::timeline::ClipSource;

use crate::media::cursor::CursorTracks;

/// Every shape the main clips playing in `[from, to)` may draw, once each, in timeline order: (the take's key, the
/// shape's id, its checked PNG path). A clip draws its take's recorded shapes where its style's look is Recorded and
/// shown and the take's frames don't all show the cursor; each clip's range is widened by the longest transition, as a
/// transition shows a clip past its ends, so this is a superset (a strict frame loads anything it still misses).
/// Takes not loaded (past the budget, unreadable) give none. O(clips × log n + shapes²) for a few shapes.
pub fn shapes_in(
    p: &Project,
    cursors: &CursorTracks,
    from: Us,
    to: Us,
) -> Vec<(Arc<str>, u16, Arc<str>)> {
    let main = &p.timeline.main;
    let pad = main
        .iter()
        .filter_map(|c| Some(c.transition_out.as_ref()?.duration_us))
        .max()
        .unwrap_or(0);
    let mut out: Vec<(Arc<str>, u16, Arc<str>)> = Vec::new();
    for (clip, start) in main.iter().zip(clip_starts(main)) {
        let end = start.saturating_add(clip.duration_us());
        let (lo, hi) = (from.max(start - pad), to.min(end + pad));
        let ClipSource::Recording { id } = &clip.source else {
            continue;
        };
        let (Some(rec), true) = (p.recording(id), lo < hi) else {
            continue;
        };
        let style = p.cursor_for(clip).effective();
        let Some(cursor) = rec
            .cursor
            .as_ref()
            .filter(|c| c.capture != CursorCapture::Burned)
        else {
            continue;
        };
        if style.hidden || style.look != CursorLook::Recorded {
            continue;
        }
        let nudge = rec.screen().map_or(0, |s| s.nudge_us);
        let at = |t: Us| (clip.source_in_us + unscaled(t - start, clip.speed)).max(0) - nudge;
        // A freeze frame shows the cursor's shape at its frame only.
        let (from_us, to_us) = match clip.freeze_at_us {
            Some(frame) => (frame.max(0) - nudge, frame.max(0) - nudge + 1),
            None => (at(lo), at(hi)),
        };
        cursors.with_take(&cursor.path, |key, take| {
            for shape in take.track.shape_ids_in(from_us, to_us) {
                let Some(file) = take.shape_file(shape) else {
                    continue;
                };
                if !out.iter().any(|(k, s, _)| k == key && *s == shape) {
                    out.push((key.clone(), shape, file.clone()));
                }
            }
        });
    }
    out
}
