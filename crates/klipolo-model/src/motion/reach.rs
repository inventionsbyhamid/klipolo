//! How far keys reach once their eases are counted: an ease that overshoots carries
//! the value past the key it eases into (a spring between scale keys 1 and 4 reaches about 6.57), so the largest
//! value is found per pair of keys, from the ease's own range, not from the keys alone.

use super::{spring_peak, Ease, Key, EASE_Y};

/// The lowest and highest an ease reaches over 0–1, as bounds: back eases pass their ends by about 10 %, elastic ones
/// by under 40 %, a cubic Bézier stays within its control points (inside [`EASE_Y`]), and a spring rises from 0 to its
/// first swing ([`spring_peak`]). Every other ease stays within 0–1.
pub fn ease_range(e: Ease) -> (f64, f64) {
    match e {
        Ease::BackIn => (-0.11, 1.0),
        Ease::BackOut => (0.0, 1.11),
        Ease::BackInOut => (-0.11, 1.11),
        Ease::ElasticIn => (-0.4, 1.0),
        Ease::ElasticOut => (0.0, 1.4),
        Ease::ElasticInOut => (-0.4, 1.4),
        Ease::CubicBezier { y1, y2, .. } => (
            y1.min(y2).clamp(EASE_Y.0, 0.0),
            y1.max(y2).clamp(1.0, EASE_Y.1),
        ),
        Ease::Spring {
            stiffness,
            damping,
            mass,
        } => (0.0, spring_peak(stiffness, damping, mass)),
        _ => (0.0, 1.0),
    }
}

/// The largest value `keys` reach, with each key's ease into it counted; `rest` with no keys.
pub fn keys_max(keys: &[Key], rest: f64) -> f64 {
    let Some(first) = keys.first() else {
        return rest;
    };
    keys.windows(2).fold(first.v, |most, pair| {
        let (a, b) = (pair[0].v, pair[1].v);
        let (lo, hi) = ease_range(pair[1].ease.unwrap_or_default());
        most.max(a)
            .max(b)
            .max(a + (b - a) * lo)
            .max(a + (b - a) * hi)
    })
}
