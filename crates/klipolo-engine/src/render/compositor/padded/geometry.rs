//! A padded layer's geometry: the grown target, sized once for the largest box the
//! layer's motion reaches so every frame of it reuses one pooled pair, where the layer lands in it this frame, and the
//! quad that places it. Arithmetic only.

use super::super::staged::EffectSize;
use crate::render::effects::{FrameInfo, Inner, MAX_EFFECT_SIDE, PadLook, Padding};
use crate::vendor::cap::compat::XY;
use crate::vendor::cap::rendering::layers::image::ImageQuad;

/// How a padded layer is drawn this frame.
#[derive(Clone, Copy, Debug)]
pub(in crate::render::compositor) struct Plan {
    /// The grown target, where the layer lands in it, and which of its sides have no margin (PAD's look, less the turn
    /// and corners the caller knows).
    pub(in crate::render::compositor) padding: Padding,
    /// The grown target placed: the layer's own pixels land where they did, the margin around them.
    pub(in crate::render::compositor) quad: ImageQuad,
    /// The grown target's pixels per drawn pixel (its passes' `kl.px` is the frame's times this).
    scale: f32,
    /// The layer's box in the grown target's uv (its passes' `kl.content`).
    content: [f32; 4],
}

impl Plan {
    /// The frame's `info` for the passes on the grown target.
    pub(in crate::render::compositor) fn info(&self, info: FrameInfo) -> FrameInfo {
        FrameInfo {
            px: info.px * self.scale,
            content: self.content,
            ..info
        }
    }

    /// PAD's look: `turn`, and `quad`'s corners in the grown target's pixels (within half the layer's shorter side).
    pub(in crate::render::compositor) fn padding(&self, turn: u16, quad: &ImageQuad) -> Padding {
        let Inner { w, h, .. } = self.padding.inner;
        let radius = (quad.rounding * f64::from(self.scale)).min(f64::from(w.min(h)) / 2.0);
        Padding {
            look: PadLook {
                turn,
                radius: if radius > 0.0 { radius as f32 } else { 0.0 },
                ..self.padding.look
            },
            ..self.padding
        }
    }
}

/// To a thousandth, as `effect_size` rounds the largest box, so values that are whole pixels up to rounding stay whole.
fn thousandth(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The share of a margin a side keeps, `room` (what lies between the layer's edge and the frame's, as a share of the
/// margin) rounded up to a quarter: coarse, so a layer sliding across a frame edge changes its target a few times, not
/// every frame, and never cut short of the frame.
fn quarters(room: f64) -> f64 {
    thousandth(room.clamp(0.0, 1.0) * 4.0).ceil() / 4.0
}

/// The plan of a layer placed as `quad`, its effects at `at` (its largest box, `reach` times this frame's), whose
/// filters draw `pad_px` 1080p pixels past it in an `out`-pixel frame of `px` pixels per 1080p pixel. None when no
/// margin is left (none declared, or all of it outside the frame).
///
/// - **The target, once per motion.** Its margin is the one at the layer's largest box, at most the margin times one
///   target pixel per drawn pixel (and within `MAX_EFFECT_SIDE`), so it is the same on every frame of a scale motion
///   and bounded as the scale nears 0.
/// - **Clipped to the frame.** Each side keeps only what lies between the layer's edge and the frame's, in
///   quarters of the margin; a full-frame layer never grows. A turned layer keeps all of it (turned, its box may reach
///   back into the frame).
/// - **This frame.** At its largest box the layer fills its own target's pixels exactly, the margin around it. Drawn
///   smaller, it lands in the same target at one scale both ways (at most its own resolution) with its margin around
///   it, the rest transparent room, and the passes' `kl.px` follows that scale, so a glow keeps its drawn size.
pub(in crate::render::compositor) fn plan(
    quad: &ImageQuad,
    at: EffectSize,
    reach: f64,
    pad_px: f64,
    px: f32,
    out: (u32, u32),
) -> Option<Plan> {
    let m = pad_px * f64::from(px);
    let (w, h) = (quad.size.x, quad.size.y);
    let finite = [m, w, h, quad.center.x, quad.center.y, quad.rotation_deg];
    if !(m > 0.0 && w > 0.0 && h > 0.0 && finite.iter().all(|v| v.is_finite())) {
        return None;
    }
    let reach = if reach.is_finite() && reach > 1.0 {
        reach
    } else {
        1.0
    };
    let turned = quad.rotation_deg % 360.0 != 0.0;
    let keep = |room: f64| if turned { 1.0 } else { quarters(room / m) };
    let (cx, cy) = (quad.center.x, quad.center.y);
    let (ow, oh) = (f64::from(out.0), f64::from(out.1));
    let share = [
        keep(cx - w / 2.0),
        keep(cy - h / 2.0),
        keep(ow - (cx + w / 2.0)),
        keep(oh - (cy + h / 2.0)),
    ];
    let (lw, lh) = (f64::from(at.size.0), f64::from(at.size.1));
    // Target pixels per drawn pixel at the largest box (the same on every frame of a motion), at most 1 for the
    // margin.
    let (rx, ry) = (lw / thousandth(w * reach), lh / thousandth(h * reach));
    let margin = |share: f64, r: f64| thousandth(m * share * r.min(1.0)).ceil() as u32;
    let (left, right) = fit(margin(share[0], rx), margin(share[2], rx), at.size.0);
    let (top, bottom) = fit(margin(share[1], ry), margin(share[3], ry), at.size.1);
    if left + right + top + bottom == 0 {
        return None;
    }
    let grown = (at.size.0 + left + right, at.size.1 + top + bottom);
    let (gw, gh) = (f64::from(grown.0), f64::from(grown.1));
    let (sx, sy, x0, y0, scale) = if reach == 1.0 {
        // At its largest box: its own target's pixels exactly, at whole pixels, as without a margin.
        (lw / w, lh / h, f64::from(left), f64::from(top), at.scale)
    } else {
        let need = share.map(|q| m * q);
        let s = (gw / (w + need[0] + need[2]))
            .min(gh / (h + need[1] + need[3]))
            .min(lw / w)
            .min(lh / h);
        // The room left over goes to the sides with a margin, in proportion (evenly when neither has one).
        let start = |lo: u32, hi: u32, need_lo: f64, room: f64| {
            let part = if lo + hi > 0 {
                f64::from(lo) / f64::from(lo + hi)
            } else {
                0.5
            };
            need_lo * s + room * part
        };
        let x0 = start(left, right, need[0], gw - (w + need[0] + need[2]) * s);
        let y0 = start(top, bottom, need[1], gh - (h + need[1] + need[3]) * s);
        (s, s, x0, y0, s as f32)
    };
    let inner = Inner {
        x: x0 as f32,
        y: y0 as f32,
        w: (w * sx) as f32,
        h: (h * sy) as f32,
    };
    // The margins in drawn pixels; the centre moves by half their difference, turned with the quad.
    let (dl, dr) = (x0 / sx, (gw - x0 - w * sx) / sx);
    let (dt, db) = (y0 / sy, (gh - y0 - h * sy) / sy);
    let (ox, oy) = ((dr - dl) / 2.0, (db - dt) / 2.0);
    let (sin, cos) = (quad.rotation_deg % 360.0).to_radians().sin_cos();
    // A side whose inner edge meets the target's has no margin: the quad's edge is the layer's there.
    let open = |gap: f64| if gap < 0.5 { 1.0 } else { 0.0 };
    Some(Plan {
        padding: Padding {
            grown,
            inner,
            look: PadLook {
                open: [open(x0), open(y0), open(dr * sx), open(db * sy)],
                ..PadLook::default()
            },
        },
        quad: ImageQuad {
            center: XY::new(cx + cos * ox - sin * oy, cy + sin * ox + cos * oy),
            size: XY::new(w + dl + dr, h + dt + db),
            ..*quad
        },
        scale,
        content: [
            (x0 / gw) as f32,
            (y0 / gh) as f32,
            (w * sx / gw) as f32,
            (h * sy / gh) as f32,
        ],
    })
}

/// Two margins around a side of `inner` target pixels, shrunk in proportion so the grown side stays within
/// `MAX_EFFECT_SIDE`.
fn fit(a: u32, b: u32, inner: u32) -> (u32, u32) {
    let room = u64::from(MAX_EFFECT_SIDE.saturating_sub(inner));
    let both = u64::from(a) + u64::from(b);
    if both <= room {
        return (a, b);
    }
    (
        (u64::from(a) * room / both) as u32,
        (u64::from(b) * room / both) as u32,
    )
}

/// The box of a text texture `size` pixels whose layout holds a filter margin of `pad_px` 1080p pixels on every side
/// (`render::text`, laid out `up` texture pixels per drawn pixel at `px` drawn pixels per 1080p pixel), in its uv: what
/// geometry-bearing passes measure from. The whole texture without a margin.
pub(in crate::render::compositor) fn text_content(
    size: (u32, u32),
    pad_px: f64,
    px: f32,
    up: f64,
) -> [f32; 4] {
    let m = pad_px * f64::from(px) * up;
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    if !(m > 0.0 && m.is_finite() && 2.0 * m < w && 2.0 * m < h) {
        return FrameInfo::WHOLE;
    }
    [
        (m / w) as f32,
        (m / h) as f32,
        (1.0 - 2.0 * m / w) as f32,
        (1.0 - 2.0 * m / h) as f32,
    ]
}

#[cfg(test)]
mod tests;
