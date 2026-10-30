//! A fill's antialiased edge: each contour's outward normal at every point (the side outside the fill,
//! by its fill rule), so the shader draws the inside half a pixel in and a one-pixel fringe across the edge whose
//! coverage falls from 1 to 0. The fringe and the inside share their edge vertices, so nothing overlaps or gaps; a
//! translucent fill shows no seam. A contour whose two sides are both filled (or neither) gets no fringe.

use klipolo_model::scene::FillRule;

use super::mesh_cache::{MAX_MESH_VERTICES, MeshData};
use super::path_pipeline::PathVertex;

/// The longest a corner's normal reaches (a sharper corner's fringe is cut there).
const MITER: f64 = 4.0;

/// Each contour point's outward normal, every contour's in turn (lyon's endpoint order), sized for a corner (its
/// miter); zero where the contour has no outside.
pub(super) fn normals(contours: &[&[[f64; 2]]], rule: FillRule) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(contours.iter().map(|c| c.len()).sum());
    for c in contours {
        let outward = outward(contours, c, rule);
        let n = c.len();
        for i in 0..n {
            if outward == 0.0 {
                out.push([0.0; 2]);
                continue;
            }
            let (prev, here, next) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n]);
            let (a, b) = (left(prev, here), left(here, next));
            let m = [a[0] + b[0], a[1] + b[1]];
            let d = 1.0 + a[0] * b[0] + a[1] * b[1];
            let mut v = if d > 1e-9 { [m[0] / d, m[1] / d] } else { a };
            let reach = v[0].hypot(v[1]);
            if reach > MITER {
                v = [v[0] / reach * MITER, v[1] / reach * MITER];
            }
            out.push([(v[0] * outward) as f32, (v[1] * outward) as f32]);
        }
    }
    out
}

/// The unit normal on the left of `a → b` (x right, y down: rotated a quarter clockwise on screen), or zero.
fn left(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dy);
    if l > 0.0 { [dy / l, -dx / l] } else { [0.0; 2] }
}

/// Which way is out of the fill along `c`'s left normals: 1 when its right side is filled and its left isn't, −1 for
/// the reverse, 0 when both or neither are (tested beside its longest edge).
fn outward(contours: &[&[[f64; 2]]], c: &[[f64; 2]], rule: FillRule) -> f64 {
    let n = c.len();
    let Some((i, l)) = (0..n)
        .map(|i| (i, dist(c[i], c[(i + 1) % n])))
        .max_by(|a, b| a.1.total_cmp(&b.1))
    else {
        return 0.0;
    };
    if l <= 0.0 {
        return 0.0;
    }
    let (a, b) = (c[i], c[(i + 1) % n]);
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let nl = left(a, b);
    let e = l * 1e-4;
    let filled = |p: [f64; 2]| {
        let w = winding(contours, p);
        match rule {
            FillRule::NonZero => w != 0,
            FillRule::EvenOdd => w % 2 != 0,
        }
    };
    let on_left = filled([mid[0] + nl[0] * e, mid[1] + nl[1] * e]);
    let on_right = filled([mid[0] - nl[0] * e, mid[1] - nl[1] * e]);
    match (on_left, on_right) {
        (false, true) => 1.0,
        (true, false) => -1.0,
        _ => 0.0,
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// The winding number of every closed contour about `p`.
fn winding(contours: &[&[[f64; 2]]], p: [f64; 2]) -> i32 {
    let mut w = 0;
    for c in contours {
        let n = c.len();
        for i in 0..n {
            let (a, b) = (c[i], c[(i + 1) % n]);
            let side = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= p[1] {
                if b[1] > p[1] && side > 0.0 {
                    w += 1;
                }
            } else if b[1] <= p[1] && side < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

/// The fringe's quads round every contour with an outside, after the fill's triangles: inner vertices (half a pixel
/// in, coverage 1, shared with the fill) and outer ones (half a pixel out, coverage 0). None past the vertex cap.
pub(super) fn push(
    contours: &[&[[f64; 2]]],
    normals: &[[f32; 2]],
    mesh: &mut MeshData,
) -> Option<()> {
    let mut at = 0;
    for c in contours {
        let n = c.len();
        let dirs = &normals[at..at + n];
        at += n;
        if dirs.iter().all(|d| *d == [0.0; 2]) {
            continue;
        }
        let first = mesh.vertices.len();
        if first + 2 * n > MAX_MESH_VERTICES {
            return None;
        }
        for (p, dir) in c.iter().zip(dirs) {
            let pos = [p[0] as f32, p[1] as f32];
            for (side, along) in [(-1.0, 1.0), (1.0, 0.0)] {
                mesh.vertices.push(PathVertex {
                    pos,
                    dir: *dir,
                    side,
                    along,
                    sub: 0.0,
                });
            }
        }
        for i in 0..n {
            let j = (i + 1) % n;
            let (in_i, out_i) = ((first + 2 * i) as u16, (first + 2 * i + 1) as u16);
            let (in_j, out_j) = ((first + 2 * j) as u16, (first + 2 * j + 1) as u16);
            mesh.indices
                .extend_from_slice(&[in_i, out_i, out_j, in_i, out_j, in_j]);
        }
    }
    Some(())
}
