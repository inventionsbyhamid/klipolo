//! Each offscreen node's own box this frame, so its target and its effects' passes are only that big:
//! the quads of every draw its subtree makes (shapes, images, fills and backdrops with their reach, text pieces with
//! their growth, path meshes with their stroke's), its offscreen children's boxes as they are laid back (a tilted
//! one's as its perspective throws it), grown for its own blur (three deviations) and its shadow (its offset plus three
//! deviations), in the scene target's pixels and cut to that target (what lies past it was never drawn, nor blurred
//! or shadowed back in). The same for the scene's own drawing (`Packed::root`): every draw outside an
//! offscreen node and every top offscreen node as it is laid back, so the scene's own target is only that big too.
//! Arithmetic over the frame's packed draws, after `pack`: nothing allocates.

use super::composite::Tilt;
use super::groups::{Iso, MAX_ISOS};
use super::meshes::Meshes;
use super::pack::{Packed, Run};
use super::path_pipeline::PathDraw;
use super::shapes::ShapeInstance;
use super::text_pipeline::{TextDraw, TextPaintDraw};

/// An offscreen node laid back into the scene's own target.
pub(super) const NO_PARENT: u8 = u8::MAX;
/// A box holding nothing (x0, y0, x1, y1).
pub(super) const EMPTY: [f32; 4] = [
    f32::INFINITY,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
];
/// Pixels a box grows by past what it holds: antialiased edges, and a blur's or a shadow's last bilinear tap.
const EDGE: f32 = 2.0;

pub(super) fn is_empty(b: [f32; 4]) -> bool {
    !(b[2] > b[0] && b[3] > b[1])
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn grow(b: [f32; 4], by: f32) -> [f32; 4] {
    if is_empty(b) {
        return b;
    }
    [b[0] - by, b[1] - by, b[2] + by, b[3] + by]
}

/// `b` within `whole`; the whole of it when `b` isn't a number (nothing is lost).
fn cut(b: [f32; 4], whole: [f32; 4]) -> [f32; 4] {
    if b.iter().any(|v| v.is_nan()) {
        return whole;
    }
    let c = [
        b[0].max(whole[0]),
        b[1].max(whole[1]),
        b[2].min(whole[2]),
        b[3].min(whole[3]),
    ];
    if is_empty(c) { EMPTY } else { c }
}

/// The box `rect` (x0, y0, x1, y1) covers under x' = a x + c y + tx, y' = b x + d y + ty (`m` = a, b, c, d).
fn mapped(m: [f32; 4], t: [f32; 2], rect: [f32; 4]) -> [f32; 4] {
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let (hx, hy) = ((rect[2] - rect[0]) / 2.0, (rect[3] - rect[1]) / 2.0);
    let (x, y) = (m[0] * cx + m[2] * cy + t[0], m[1] * cx + m[3] * cy + t[1]);
    let ex = m[0].abs() * hx + m[2].abs() * hy;
    let ey = m[1].abs() * hx + m[3].abs() * hy;
    [x - ex, y - ey, x + ex, y + ey]
}

/// A shape's quad (`shapes.wgsl`: its half size grown by its reach).
fn shape_box(s: &ShapeInstance) -> [f32; 4] {
    let r = s.shape[3].max(0.0);
    let (hx, hy) = (s.at_half[2].abs() + r, s.at_half[3].abs() + r);
    mapped(s.xf, [s.at_half[0], s.at_half[1]], [-hx, -hy, hx, hy])
}

/// A text piece's or background's quad (`text.wgsl`: grown for a background's edge and a blurring piece's blur, and
/// over a piece's baked shadow where it falls), its paint among `paints`.
fn text_box(d: &TextDraw, paints: &[TextPaintDraw]) -> [f32; 4] {
    let blur = d.misc[2].max(0.0);
    let by = if d.t[2] > 0.5 { 1.5 } else { 0.0 } + if blur > 0.0 { 3.0 * blur + 1.0 } else { 0.0 };
    let r = d.rect;
    let mut quad = [r[0] - by, r[1] - by, r[2] + by, r[3] + by];
    let shadow = paints.get(d.misc[1] as usize);
    if let Some(p) = shadow.filter(|p| p.shadow[3] > 0.0 && d.t[2] < 0.5) {
        let ([x, y, ..], s) = (p.shadow_at, p.shadow_rect);
        quad = union(quad, [s[0] + x, s[1] + y, s[2] + x, s[3] + y]);
    }
    mapped(d.m, [d.t[0], d.t[1]], quad)
}

/// A path draw over its mesh's box (`paths.wgsl`: each vertex pushed out along its direction by the draw's reach).
fn path_box(d: &PathDraw, mesh: [f32; 5]) -> [f32; 4] {
    let by = d.t[2].abs() * mesh[4];
    let r = [mesh[0] - by, mesh[1] - by, mesh[2] + by, mesh[3] + by];
    mapped(d.m, [d.t[0], d.t[1]], r)
}

/// Where a tilted node's box `b` lands, seen through its perspective (`composite.wgsl`'s `untilt`, forwards): the whole
/// target when a corner passes behind the eye.
fn projected(t: &Tilt, b: [f32; 4], whole: [f32; 4]) -> [f32; 4] {
    let mut out = EMPTY;
    for (x, y) in [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])] {
        let u = [x - t.origin[0], y - t.origin[1]];
        let w = t.d - (t.r3[0] * u[0] + t.r3[1] * u[1]);
        if w.is_nan() || w <= 1e-3 {
            return whole;
        }
        let s = t.d / w;
        let q = [
            t.origin[0] + s * (t.r2[0] * u[0] + t.r2[1] * u[1]),
            t.origin[1] + s * (t.r2[2] * u[0] + t.r2[3] * u[1]),
        ];
        out = union(out, [q[0], q[1], q[0], q[1]]);
    }
    cut(grow(out, EDGE), whole)
}

/// Offscreen node `iso`'s box from what it draws (`raw`): grown for its blur, then its shadow under it.
fn padded(iso: &Iso, raw: [f32; 4], k: f64, whole: [f32; 4]) -> [f32; 4] {
    if is_empty(raw) {
        return EMPTY;
    }
    let mut b = grow(raw, EDGE);
    if iso.blur > 0.0 {
        b = grow(b, (3.0 * iso.blur * k) as f32 + EDGE);
    }
    if let Some(sh) = iso.shadow {
        let [dx, dy] = sh.offset;
        let moved = [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy];
        b = union(b, grow(moved, (3.0 * sh.sigma.max(0.0) * k) as f32 + EDGE));
    }
    cut(b, whole)
}

/// The frame's offscreen nodes' boxes (`Iso::bounds`) and parents (`Iso::parent`), and what the scene draws in its own
/// target (`Packed::root`), from its runs after `pack`, in a target of `size`, `k` of its pixels per 1080p
/// pixel.
pub(super) fn of_frame(p: &mut Packed, meshes: &Meshes, size: (u32, u32), k: f64) {
    let whole = [0.0, 0.0, size.0 as f32, size.1 as f32];
    let mut root = EMPTY;
    let mut raw = [EMPTY; MAX_ISOS];
    let mut open = [NO_PARENT; MAX_ISOS];
    let mut depth = 0usize;
    for j in 0..p.mask_from.min(p.runs.len()) {
        let drawn = match p.runs[j] {
            Run::Begin(s) => {
                let parent = depth.checked_sub(1).map_or(NO_PARENT, |d| open[d]);
                if let Some(iso) = p.isos.get_mut(usize::from(s)) {
                    iso.parent = parent;
                }
                if depth < MAX_ISOS {
                    open[depth] = s;
                    depth += 1;
                }
                continue;
            }
            Run::End(s) => {
                let Some(d) = depth.checked_sub(1) else {
                    continue;
                };
                depth = d;
                let Some(iso) = p.isos.get_mut(usize::from(s)) else {
                    continue;
                };
                iso.bounds = padded(iso, raw[usize::from(s) % MAX_ISOS], k, whole);
                match &iso.lay.tilt {
                    Some(t) if !is_empty(iso.bounds) => projected(t, iso.bounds, whole),
                    _ => iso.bounds,
                }
            }
            Run::Shapes(a, b)
            | Run::Image { from: a, to: b, .. }
            | Run::Fill { from: a, to: b, .. }
            | Run::Backdrop { from: a, to: b, .. } => p
                .shapes
                .get(a as usize..b as usize)
                .map_or(EMPTY, |s| s.iter().map(shape_box).fold(EMPTY, union)),
            Run::Text(a, b) => p.texts.get(a as usize..b as usize).map_or(EMPTY, |t| {
                let boxes = t.iter().map(|d| text_box(d, &p.text_paints));
                boxes.fold(EMPTY, union)
            }),
            Run::Path { node, draw, .. } => {
                let mesh = meshes.get(node as usize).and_then(|m| m.mesh.as_ref());
                match (mesh, p.draws.get(draw as usize)) {
                    (Some(m), Some(d)) => path_box(d, m.bounds),
                    _ => EMPTY,
                }
            }
            Run::Mask(_) | Run::MaskEnd(_) => EMPTY,
        };
        match depth.checked_sub(1) {
            Some(d) => {
                let top = usize::from(open[d]) % MAX_ISOS;
                raw[top] = union(raw[top], drawn);
            }
            None => root = union(root, drawn),
        }
    }
    p.root = if is_empty(root) {
        EMPTY
    } else {
        cut(grow(root, EDGE), whole)
    };
    p.backdrop = backdrops(p, k, whole);
}

/// The box the frame's backdrop nodes read of the frame under the layer (what the compositor
/// grabs): each one's quad grown by its blur's reach (three deviations) and a pixel's edge, cut to the target.
fn backdrops(p: &Packed, k: f64, whole: [f32; 4]) -> [f32; 4] {
    let mut b = EMPTY;
    for run in &p.runs[..p.mask_from.min(p.runs.len())] {
        if let Run::Backdrop { from, to, slot } = *run {
            let sigma = p
                .backdrop_uses
                .get(usize::from(slot))
                .map_or(0.0, |u| u.sigma);
            let reach = (3.0 * sigma.max(0.0) * k) as f32 + EDGE;
            let shapes = p.shapes.get(from as usize..to as usize).unwrap_or_default();
            b = shapes
                .iter()
                .map(shape_box)
                .fold(b, |b, s| union(b, grow(s, reach)));
        }
    }
    if is_empty(b) { EMPTY } else { cut(b, whole) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quad's box turns with it: a 10 × 4 box turned a quarter is 4 × 10 about the same centre.
    #[test]
    fn a_turned_quad_covers_its_turned_box() {
        let b = mapped([0.0, 1.0, -1.0, 0.0], [50.0, 50.0], [-5.0, -2.0, 5.0, 2.0]);
        assert_eq!(b, [48.0, 45.0, 52.0, 55.0]);
        let whole = [0.0, 0.0, 100.0, 100.0];
        assert_eq!(
            cut([-10.0, 20.0, 30.0, 140.0], whole),
            [0.0, 20.0, 30.0, 100.0]
        );
        assert!(is_empty(cut([120.0, 0.0, 130.0, 10.0], whole)));
        assert_eq!(cut([f32::NAN, 0.0, 1.0, 1.0], whole), whole);
    }
}
