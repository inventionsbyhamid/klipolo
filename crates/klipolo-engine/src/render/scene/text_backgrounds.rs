//! A frame's span backgrounds: one rounded rect per span and line under its pieces, as wide as they
//! stand this frame (a marker that grows as its letters do), padded and rounded. Arithmetic only, into the frame's
//! lists (`text_pack.rs`): each its draw and its paint.

use super::place::{Affine, World, mul};
use super::text::{NO_UNIT, TextUnits};
use super::text_move::{Placing, moved};
use super::text_nodes::NodeText;
use super::text_pack::{NodeFrame, TextOut};
use super::text_paint::clips;
use super::text_pipeline::{TextDraw, TextPaintDraw};

/// Each span background under its pieces, one rect per span and line, as wide as its pieces stand now (a marker that
/// grows as its letters do), padded and rounded.
pub(super) fn backgrounds(
    out: &mut TextOut,
    nt: &NodeText,
    text: &TextUnits,
    f: &NodeFrame,
    steps: Option<usize>,
) {
    let at = Placing::of(nt, text, f);
    let mut open: Option<(u8, u16, [f64; 4])> = None;
    let flush = |open: Option<(u8, u16, [f64; 4])>, out: &mut TextOut| {
        let Some((look, _, r)) = open else {
            return;
        };
        let Some(bg) = nt
            .paint
            .backgrounds
            .get(usize::from(look))
            .copied()
            .flatten()
        else {
            return;
        };
        if out.full() || r[0] > r[2] {
            return;
        }
        // In the node's box as this frame has it: the layout's pixels, fitted.
        let u = at.u;
        let (px, py) = (
            f64::from(bg.padding) * f.k * u,
            f64::from(bg.padding) * f.k * u,
        );
        let m = f.here.m;
        out.paints.push(TextPaintDraw {
            fill: bg.color,
            paint: [-1.0, 0.0, 0.0, 0.0],
            gbox: [f.here.w as f32, f.here.h as f32, 0.0, 0.0],
            clips: clips(f.here, text.clip),
            ..TextPaintDraw::default()
        });
        out.draws.push(TextDraw {
            m: [m[0], m[1], m[2], m[3]].map(|v| v as f32),
            t: [
                m[4] as f32,
                m[5] as f32,
                1.0,
                (f64::from(bg.radius) * f.k * u) as f32,
            ],
            rect: [r[0] - px, r[1] - py, r[2] + px, r[3] + py].map(|v| v as f32),
            uv: [0.0; 4],
            misc: [
                f.here.opacity as f32,
                (out.paints.len() - 1) as f32,
                0.0,
                0.0,
            ],
            blur_a: [0.0; 4],
            blur_b: [0.0; 4],
        });
    };
    for p in &text.pieces {
        if steps.is_some_and(|n| p.reveal != NO_UNIT && usize::from(p.reveal) >= n) {
            continue;
        }
        let has = nt
            .paint
            .backgrounds
            .get(usize::from(p.look))
            .is_some_and(Option::is_some);
        let (m, opacity) = moved(nt, text, p, p.rect, f, p.unit, true, &at);
        if !has || opacity < 0.01 {
            continue;
        }
        // The piece's box now, in the node's box: its rest box through this frame's moves (less the node's place).
        let local = inverse_place(f.here, &m);
        let r = aabb(&local, p.rect);
        match &mut open {
            Some((look, line, u)) if *look == p.look && *line == p.line => {
                *u = [
                    u[0].min(r[0]),
                    u[1].min(r[1]),
                    u[2].max(r[2]),
                    u[3].max(r[3]),
                ];
            }
            _ => {
                flush(open.take(), out);
                open = Some((p.look, p.line, r));
            }
        }
    }
    flush(open, out);
}

/// `m` (box pixels → target) without its node's place: box pixels → box pixels.
fn inverse_place(here: &World, m: &Affine) -> Affine {
    let h = here.m;
    let det = h[0] * h[3] - h[1] * h[2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let (a, b, c, d) = (h[3] / det, -h[1] / det, -h[2] / det, h[0] / det);
    let inv = [a, b, c, d, -(a * h[4] + c * h[5]), -(b * h[4] + d * h[5])];
    mul(&inv, m)
}

/// The box holding `r` through `m`.
fn aabb(m: &Affine, r: [f32; 4]) -> [f64; 4] {
    let mut out = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (x, y) = (f64::from(x), f64::from(y));
        let (px, py) = (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        out = [
            out[0].min(px),
            out[1].min(py),
            out[2].max(px),
            out[3].max(py),
        ];
    }
    out
}
