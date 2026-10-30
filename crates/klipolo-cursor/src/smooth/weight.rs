//! How far the drawn cursor eases onto the raw path at a time, from the take's presses: clicks land
//! where they happened, and drags are the real path. It is read when the path is sampled, so a press lands exactly
//! wherever it falls on the step grid. O(log n), no allocation.

use super::{DRAG_RAMP_US, PIN_US};
use crate::ease::smootherstep;
use crate::{CursorTrack, Us};

/// 0 at `from`, rising to 1 at `from + len`.
fn ramp(from: Us, t: Us, len: Us) -> f64 {
    smootherstep(t.saturating_sub(from) as f64 / len as f64)
}

/// The raw path's weight at `t`: the larger of
/// - the nearest press's pin: 1 at its down, easing to 0 `PIN_US` either side;
/// - the drag: from 0 where a hold begins to 1 after `DRAG_RAMP_US`, then from where it was back to 0 over
///   `DRAG_RAMP_US` after the release.
///
/// Exactly 1 at a press, and while a button is held once its ramp is over.
pub(super) fn onto_raw(track: &CursorTrack, t: Us) -> f64 {
    let drag = match track.held_span(t) {
        Some((down, up)) if t < up => ramp(down, t, DRAG_RAMP_US),
        Some((down, up)) if t.saturating_sub(up) < DRAG_RAMP_US => {
            ramp(down, up, DRAG_RAMP_US) * (1.0 - ramp(up, t, DRAG_RAMP_US))
        }
        _ => 0.0,
    };
    if drag >= 1.0 {
        return 1.0;
    }
    let presses = track.presses();
    let i = presses.partition_point(|p| p.down <= t);
    let nearest = [i.checked_sub(1), (i < presses.len()).then_some(i)]
        .into_iter()
        .flatten()
        .map(|i| presses[i].down.abs_diff(t))
        .min();
    let pin = nearest.map_or(0.0, |d| smootherstep(1.0 - d as f64 / PIN_US as f64));
    pin.max(drag)
}
