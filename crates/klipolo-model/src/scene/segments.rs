//! Where a scene is in its own time: a layer longer or shorter than the scene plays its entrance
//! (`in`) from the layer's start and its exit (`out`) up to its end, and the hold between them stretches or squeezes.
//! A split piece reads the span its layer had (`PieceTime`), so the second piece continues the hold where the first
//! left it instead of replaying the entrance.

use super::Segments;
use crate::time::Us;
use crate::timeline::PieceTime;

/// Scene time (seconds) `u_us` into a piece's span: `in` plays from the span's start and `out` up to its end; the hold
/// between them stretches or squeezes; a span shorter than in + out squeezes both. Without segments the scene plays at
/// its own speed and holds its last frame.
pub fn scene_time(span_us: Us, u_us: Us, duration: f64, seg: Option<Segments>) -> f64 {
    let s = span_us.max(1) as f64 / 1e6;
    let u = (u_us.max(0) as f64 / 1e6).min(s);
    let Some(Segments { enter, out }) = seg else {
        return u.min(duration);
    };
    let a = enter.clamp(0.0, duration);
    let b = out.clamp(0.0, duration - a);
    if s >= a + b {
        if u <= a {
            u
        } else if u >= s - b {
            duration - (s - u)
        } else {
            a + (u - a) * (duration - a - b) / (s - a - b).max(1e-9)
        }
    } else {
        let k = s / (a + b).max(1e-9);
        if u <= a * k {
            u / k
        } else {
            duration - (s - u) / k
        }
    }
}

/// Scene time `local_us` into a piece of a layer: its offset into the span its layer had, then [`scene_time`]. When
/// a split's first piece is made longer its span grows and the second piece's doesn't, so a moving hold jumps a
/// little at the split point (accepted).
pub fn piece_scene_time(
    piece: PieceTime,
    local_us: Us,
    duration: f64,
    seg: Option<Segments>,
) -> f64 {
    scene_time(
        piece.span_us,
        piece.offset_us.saturating_add(local_us.max(0)),
        duration,
        seg,
    )
}

/// When the scene has settled into its hold: the end of `in`, else its end. A still of the scene is drawn here.
pub fn settled_time(duration: f64, seg: Option<Segments>) -> f64 {
    match seg {
        Some(Segments { enter, .. }) => enter.clamp(0.0, duration),
        None => duration,
    }
}
