//! How caption words and pages move: an entrance's opacity, scale and rise from its
//! fixed easings, a page's seeded tilt, and colour mixes. Pure and allocation-free (`frame`'s helpers).

use klipolo_model::captions::style::Move;
use klipolo_model::motion::Ease;
use klipolo_model::time::Us;

pub(super) fn ms(v: u32) -> Us {
    Us::from(v) * 1_000
}

/// 0 → 1 over `len_ms` from `from`.
pub(super) fn progress(t: Us, from: Us, len_ms: u32) -> f32 {
    if len_ms == 0 {
        return if t >= from { 1.0 } else { 0.0 };
    }
    ((t - from) as f64 / (f64::from(len_ms) * 1000.0)).clamp(0.0, 1.0) as f32
}

/// (opacity, scale, dy in em) of an entrance at progress `p` (fixed easings).
pub(super) fn enter(m: &Move, p: f32) -> (f32, f32, f32) {
    let e = |ease: Ease, x: f32| ease.apply(f64::from(x)) as f32;
    match *m {
        Move::None => (1.0, 1.0, 0.0),
        Move::Fade { .. } => (e(Ease::Power1Out, p), 1.0, 0.0),
        Move::Rise { dy_em, .. } => (
            e(Ease::Power1Out, p),
            1.0,
            dy_em as f32 * (1.0 - e(Ease::Power2Out, p)),
        ),
        Move::Pop {
            from, peak, dy_em, ..
        } => {
            let (from, peak) = (from as f32, peak as f32);
            let scale = if p < 0.6 {
                from + (peak - from) * e(Ease::Power2Out, p / 0.6)
            } else {
                peak + (1.0 - peak) * e(Ease::EaseInOut, (p - 0.6) / 0.4)
            };
            (
                (p / 0.4).min(1.0),
                scale,
                dy_em as f32 * (1.0 - e(Ease::Power2Out, p)),
            )
        }
        Move::Slam { from, .. } => (
            (p / 0.3).min(1.0),
            from as f32 + (1.0 - from as f32) * e(Ease::Power2In, p),
            0.0,
        ),
    }
}

pub(super) fn move_ms(m: &Move) -> u32 {
    match *m {
        Move::None => 0,
        Move::Fade { ms }
        | Move::Rise { ms, .. }
        | Move::Pop { ms, .. }
        | Move::Slam { ms, .. } => ms,
    }
}

/// −1…1 from the page's index (preview and export agree).
pub(super) fn unit(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

pub(super) fn mix(a: [u8; 3], b: [u8; 3], k: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * k).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}
