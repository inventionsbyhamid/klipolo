//! The cursor's effects at one instant: the press squish, the ripples after real clicks and the smear
//! over the shutter. Each is O(log presses) or O(1), with no allocation, and holds nothing between frames: a pure
//! function of the take and the time, so scrubbing lands on the same effect as playback. (The idle fade is
//! `klipolo_cursor::facts::idle_opacity`, shared with chat's facts.)

use klipolo_cursor::ease::{ease_out, smootherstep};
use klipolo_cursor::{CursorTrack, Us};
use klipolo_model::cursor_style::Rgba;

use super::{BLUR_REACH, PRESS, Place, RIPPLE_REACH, RIPPLE_US, SHUTTER};
use crate::plan::Ripple;

/// How long the squish takes to go down, and to come back.
const PRESS_EASE_US: Us = 60_000;

/// 1 at rest. Once the primary button goes down it eases to `PRESS` over 60 ms; once it comes up it eases back over 60
/// ms from wherever it got to, so a release never jumps. Screen time: `speed` source µs a timeline µs. O(log presses).
pub(super) fn press(track: &CursorTrack, t: Us, speed: f64) -> f64 {
    let Some((down, up)) = track.primary_press_at(t) else {
        return 1.0;
    };
    let span = PRESS_EASE_US as f64 * speed;
    let ease = |us: Us| smootherstep(us as f64 / span);
    let into = if t < up {
        ease(t.saturating_sub(down))
    } else {
        ease(up.saturating_sub(down)) * (1.0 - ease(t.saturating_sub(up)))
    };
    1.0 - (1.0 - PRESS) * into
}

/// How long a ripple runs in the take's own time at clip speed `speed` (`RIPPLE_US` of screen time), at least 1 µs.
fn window(speed: f64) -> Us {
    ((RIPPLE_US as f64 * speed).round() as Us).max(1)
}

/// Some press (any button) went down within a ripple's run up to `t`. O(log presses).
pub(super) fn rippling(track: &CursorTrack, t: Us, speed: f64) -> bool {
    let presses = track.presses();
    let end = presses.partition_point(|p| p.down <= t);
    end > 0 && presses[end - 1].down > t.saturating_sub(window(speed))
}

/// A ring for each of the newest four presses (any button) in the ripple's run up to `t` (`RIPPLE_US` of screen time,
/// at clip speed `speed`), oldest first: from where it was clicked, growing to `RIPPLE_REACH` cursor heights (eased
/// out) while its alpha fades from the colour's own. O(log presses).
pub(super) fn ripples(
    track: &CursorTrack,
    t: Us,
    speed: f64,
    height: f64,
    color: Rgba,
    place: &Place,
) -> ([Ripple; 4], u8) {
    let window = window(speed);
    let presses = track.presses();
    let end = presses.partition_point(|p| p.down <= t);
    let start = presses[..end].partition_point(|p| p.down <= t.saturating_sub(window));
    let mut out = [Ripple::default(); 4];
    let newest = &presses[start.max(end.saturating_sub(out.len()))..end];
    for (ring, p) in out.iter_mut().zip(newest) {
        let u = t.saturating_sub(p.down) as f64 / window as f64;
        *ring = Ripple {
            center: place.point((p.x, p.y)),
            radius: ease_out(u) * RIPPLE_REACH * height,
            alpha: (1.0 - u) * f64::from(color.0[3]) / 255.0,
            color: color.0,
        };
    }
    (out, newest.len() as u8)
}

/// The smear over the shutter: the drawn path's `velocity` (per second of the take) times the clip's `speed` (per
/// second of the timeline), over a 180° shutter at the project's `fps`, times the style's `amount`, as the footage draws
/// it, and at most `BLUR_REACH` cursor heights long. 0 at rest, and for anything not finite.
pub(super) fn blur(
    velocity: (f32, f32),
    speed: f64,
    fps: u32,
    amount: f64,
    place: &Place,
    height: f64,
) -> (f64, f64) {
    let over = speed * SHUTTER / f64::from(fps.max(1)) * amount;
    let (x, y) = place.vector((f64::from(velocity.0) * over, f64::from(velocity.1) * over));
    let length = place.length((x, y));
    let reach = BLUR_REACH * height;
    if !length.is_finite() || !reach.is_finite() {
        (0.0, 0.0)
    } else if length > reach {
        (x * reach / length, y * reach / length)
    } else {
        (x, y)
    }
}
