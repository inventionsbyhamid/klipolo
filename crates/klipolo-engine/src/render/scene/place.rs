//! Where a scene's nodes land in its target: each node's box composed down the tree in target pixels
//! (a box is a fraction of its parent's plus 1080p pixels; a node moves by a fraction of its parent's box, and scales
//! and turns about its origin in its own), its world opacity and the clips that hold it: its own and its nearest
//! clipped ancestor's, which intersect, each in its owner's node space. A clip deeper still (a third
//! clipped ancestor) is left to the offscreen groups.

use klipolo_model::scene::eval::{ClipShape, NodeState};
use klipolo_model::scene::instance::SceneInstance;

use super::Drawn;
use super::statics::NodeStatic;

/// x' = a x + c y + tx, y' = b x + d y + ty, as `[a, b, c, d, tx, ty]`.
pub(super) type Affine = [f64; 6];

pub(super) const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub(super) fn mul(p: &Affine, q: &Affine) -> Affine {
    [
        p[0] * q[0] + p[2] * q[1],
        p[1] * q[0] + p[3] * q[1],
        p[0] * q[2] + p[2] * q[3],
        p[1] * q[2] + p[3] * q[3],
        p[0] * q[4] + p[2] * q[5] + p[4],
        p[1] * q[4] + p[3] * q[5] + p[5],
    ]
}

pub(super) fn translate(x: f64, y: f64) -> Affine {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// The inverse, or None for a transform that flattens (a scale of 0).
fn inverse(m: &Affine) -> Option<Affine> {
    let det = m[0] * m[3] - m[1] * m[2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

/// A clip as the shader reads it, in its owner's node space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct GpuClip {
    pub m: [f32; 4],
    pub t: [f32; 4],
    pub c: [f32; 4],
}

/// A node's place in the target: its box's transform (box-local pixels → target pixels), size and opacity, and the
/// two innermost clips that hold it and its children (the nearer first).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct World {
    pub m: Affine,
    pub w: f64,
    pub h: f64,
    pub opacity: f64,
    pub clips: [GpuClip; 2],
}

/// Node `s`'s box in its parent's `pw` × `ph` pixels (static part `st`): where it is and how big, before it moves.
pub(super) fn box_of(st: &NodeStatic, (pw, ph): (f64, f64), k: f64) -> [f64; 4] {
    let [bx, by, bw, bh] = st.bounds;
    [
        bx[0] * pw + bx[1] * k,
        by[0] * ph + by[1] * k,
        (bw[0] * pw + bw[1] * k).max(0.0),
        (bh[0] * ph + bh[1] * k).max(0.0),
    ]
}

/// Node `s` (static part `st`) in its parent's place; on its motion path, `motion` puts its origin at that
/// point of the parent's box and adds the path's turn.
pub(super) fn place(
    parent: &World,
    s: &NodeState,
    st: &NodeStatic,
    k: f64,
    motion: Option<(f64, f64, f64)>,
) -> World {
    let (pw, ph) = (parent.w, parent.h);
    let [x0, y0, w, h] = box_of(st, (pw, ph), k);
    let (tx, ty) = (s.x * pw + s.x_px * k, s.y * ph + s.y_px * k);
    let (ox, oy) = (
        s.origin[0] * w + s.origin_px[0] * k,
        s.origin[1] * h + s.origin_px[1] * k,
    );
    let (sx, sy) = (s.scale * s.scale_x, s.scale * s.scale_y);
    let (at, turned) = match motion {
        Some((mx, my, turn)) => ((mx + tx, my + ty), s.rotate + turn),
        None => ((x0 + tx + ox, y0 + ty + oy), s.rotate),
    };
    let (sin, cos) = turned.to_radians().sin_cos();
    let turn: Affine = [cos * sx, sin * sx, -sin * sy, cos * sy, 0.0, 0.0];
    let local = mul(&mul(&translate(at.0, at.1), &turn), &translate(-ox, -oy));
    let m = mul(&parent.m, &local);
    let opacity = parent.opacity * s.opacity;
    let clips = if s.clip.shape == ClipShape::None {
        parent.clips
    } else {
        own_clip(&m, w, h, s, k).map_or(parent.clips, |own| [own, parent.clips[0]])
    };
    World {
        m,
        w,
        h,
        opacity,
        clips,
    }
}

/// `s`'s own clip in its centred node space (`m` its box's transform, `w` × `h` its box).
fn own_clip(m: &Affine, w: f64, h: f64, s: &NodeState, k: f64) -> Option<GpuClip> {
    let centred = mul(m, &translate(w / 2.0, h / 2.0));
    let inv = inverse(&centred)?;
    let v = s.clip.v;
    let len = |l: [f64; 2], of: f64| l[0] * of + l[1] * k;
    let (kind, c, round) = match s.clip.shape {
        ClipShape::Inset => (
            1.0,
            [
                -w / 2.0 + len(v[3], w),
                -h / 2.0 + len(v[0], h),
                w / 2.0 - len(v[1], w),
                h / 2.0 - len(v[2], h),
            ],
            len(v[4], w.min(h)),
        ),
        ClipShape::Circle => (
            2.0,
            [
                len(v[1], w) - w / 2.0,
                len(v[2], h) - h / 2.0,
                len(v[0], ((w * w + h * h) / 2.0).sqrt()),
                0.0,
            ],
            0.0,
        ),
        ClipShape::None => return None,
    };
    Some(GpuClip {
        m: [inv[0], inv[1], inv[2], inv[3]].map(|v| v as f32),
        t: [inv[4] as f32, inv[5] as f32, kind, round as f32],
        c: c.map(|v| v as f32),
    })
}

/// `here`'s own box as a clip, in its centred node space: what a fitted text keeps to. None for a box
/// that flattens.
pub(super) fn box_clip(here: &World) -> Option<GpuClip> {
    let (w, h) = (here.w, here.h);
    let inv = inverse(&mul(&here.m, &translate(w / 2.0, h / 2.0)))?;
    Some(GpuClip {
        m: [inv[0], inv[1], inv[2], inv[3]].map(|v| v as f32),
        t: [inv[4] as f32, inv[5] as f32, 1.0, 0.0],
        c: [-w / 2.0, -h / 2.0, w / 2.0, h / 2.0].map(|v| v as f32),
    })
}

/// Node space (centred on the box) → target pixels.
pub(super) fn centred(here: &World) -> Affine {
    mul(&here.m, &translate(here.w / 2.0, here.h / 2.0))
}

/// Every node's box size in pixels where it is `drawn`, before anything moves it, into `out` (its room kept: no
/// allocation once warm).
pub(super) fn sizes(
    inst: &SceneInstance,
    statics: &[NodeStatic],
    Drawn { size, k }: Drawn,
    out: &mut Vec<(f64, f64)>,
) {
    let root = (f64::from(size.0), f64::from(size.1));
    out.clear();
    for i in 0..inst.len() {
        let parent = inst
            .parent(i)
            .and_then(|p| out.get(p))
            .copied()
            .unwrap_or(root);
        let [_, _, w, h] = statics
            .get(i)
            .map_or([0.0, 0.0, parent.0, parent.1], |st| box_of(st, parent, k));
        out.push((w, h));
    }
}
