//! The room a smoothed path is built into, and its cap.
//!
//! **Memory while building:** the points are reserved from the moving time before the first step
//! (each segment runs from a rest's end to the next rest's start plus the ease), so the buffer doesn't double on the
//! loader thread: a 4 MB path no longer peaks at 8 MB (12 MB while a doubling copies). A button held through a rest
//! lengthens a segment past the estimate; the buffer then grows by an eighth at a time.
//!
//! **The cap** (`MAX_STEPS`, 16 MB): a path that reaches it mid-segment eases its last 4/ω onto the raw path, so the
//! hand-back to the raw path there never jumps, and the build stops.

use super::{STEP_US, SmoothedPath};
use crate::ease::smootherstep;
use crate::track::{fix, unfix};
use crate::{CursorTrack, Us};

/// The steps the segments from `starts` take when no button is held through a still: each runs to the next still's
/// start plus the ease (`rest`), and a step more. O(rests).
pub(super) fn steps_expected(rest: Us, stills: &[(Us, Us)], starts: &[Us]) -> usize {
    let mut next = 0;
    let mut steps = 0usize;
    for &start in starts {
        while stills.get(next).is_some_and(|s| s.0 < start) {
            next += 1;
        }
        let to = stills.get(next).map_or(start, |s| s.0).saturating_add(rest);
        let span = to.saturating_sub(start).max(0) / STEP_US;
        steps = steps.saturating_add(
            usize::try_from(span)
                .unwrap_or(usize::MAX)
                .saturating_add(2),
        );
    }
    steps
}

/// Appends a step, growing the buffer by an eighth when full (never doubling it).
pub(super) fn push(path: &mut SmoothedPath, point: [u16; 2]) {
    if path.xy.len() == path.xy.capacity() {
        path.xy.reserve_exact(path.xy.len() / 8 + 1_024);
    }
    path.xy.push(point);
}

/// At the cap, mid-segment: the segment's last 4/ω of steps (or all of them) eased onto the raw path, so the stored path
/// ends on it and the raw path after it continues with no jump. O(ease steps × log samples).
pub(super) fn hand_back(
    track: &CursorTrack,
    rest: Us,
    path: &mut SmoothedPath,
    (first, start): (usize, Us),
) {
    // At least the step just stored.
    let len = path.xy.len() - first;
    let k = usize::try_from(rest / STEP_US).unwrap_or(1).clamp(1, len);
    for j in 0..k {
        let i = path.xy.len() - k + j;
        let t = start.saturating_add(STEP_US.saturating_mul((i - first) as Us));
        let Some(raw) = track.position(t) else {
            continue;
        };
        let w = smootherstep((j + 1) as f64 / k as f64);
        let [x, y] = path.xy[i];
        let (x, y) = (unfix(f64::from(x)), unfix(f64::from(y)));
        let mixed = (
            x + (f64::from(raw.0) - x) * w,
            y + (f64::from(raw.1) - y) * w,
        );
        path.xy[i] = [fix(mixed.0 as f32), fix(mixed.1 as f32)];
    }
}
