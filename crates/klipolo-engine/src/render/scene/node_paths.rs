//! What a node's path mesh is made from: a path's data parsed, hashed and bounded once; a polyline's or
//! an outline's commands for the box it is made for; the buckets its inputs fall in.

use std::sync::Arc;

use klipolo_model::scene::eval::NodeState;
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::path::{PathCmd, parse_d};
use klipolo_model::scene::{LineCap, Props};

use super::geometry::{self, Outline};
use super::meshes::{Made, Units, props};
use super::statics::{Look, NodeStatic};

/// A path node's data, read once: its commands, their hash and its view box.
pub(super) struct Parsed {
    pub cmds: Arc<[PathCmd]>,
    pub hash: u64,
    pub view_box: [f64; 4],
}

/// `v` in steps of 10 % (0 stays 0).
pub(super) fn step(v: f64) -> f64 {
    if v > 1e-9 && v.is_finite() {
        1.1f64.powf((v.ln() / 1.1f64.ln()).round())
    } else {
        0.0
    }
}

/// What a box-pixel mesh is made for (its box, and an outline's corners and depth), from what they are now and were last
/// frame: each exact while it holds still and in 10 % steps while it moves (corners that move on
/// a still box are made on that box, so only their own step shows), the depth always exact.
pub(super) fn made_for(
    last: Option<((f64, f64), [f64; 5])>,
    size: (f64, f64),
    shape: [f64; 5],
) -> ((f64, f64), [f64; 5]) {
    let (box_still, shape_still) = last.map_or((true, true), |(s, c)| (s == size, c == shape));
    let size = if box_still {
        size
    } else {
        (step(size.0), step(size.1))
    };
    let mut made = if shape_still { shape } else { shape.map(step) };
    made[4] = shape[4];
    (size, made)
}

/// `v`'s power of two, rounded by `round`.
pub(super) fn bucket(v: f64, round: fn(f64) -> f64) -> i8 {
    round(v.max(1e-18).log2()).clamp(-60.0, 60.0) as i8
}

/// Node `i`'s commands for what it is made for (a path's, read once; a polyline's or an outline's, in its stepped box)
/// and their hash.
pub(super) fn geometry_of(
    inst: &SceneInstance,
    i: usize,
    st: &NodeStatic,
    parsed: Option<&Parsed>,
    made: &Made,
) -> Option<(Arc<[PathCmd]>, u64)> {
    let (w, h) = made.size;
    let cmds: Arc<[PathCmd]> = match st.look {
        Look::Path => return parsed.map(|p| (p.cmds.clone(), p.hash)),
        Look::Polyline => {
            let p = props(inst, i)?;
            let points = inst.points_of(i, p.points.as_ref()?);
            geometry::polyline(&points, w, h, p.smooth == Some(true)).into()
        }
        look => {
            let [r0, r1, r2, r3, inner] = made.shape;
            let shape = match look {
                Look::Rect => Outline::Rect([r0, r1, r2, r3]),
                Look::Ellipse => Outline::Ellipse,
                Look::Star { points } => Outline::Star {
                    points,
                    inner: Some(inner),
                },
                Look::Polygon { sides } => Outline::Star {
                    points: sides,
                    inner: None,
                },
                Look::Line { .. } => Outline::Line,
                _ => return None,
            };
            geometry::outline(&shape, w, h).into()
        }
    };
    let tag = if st.look == Look::Polyline { 2 } else { 3 };
    let hash = geometry::hash(tag, &cmds);
    Some((cmds, hash))
}

/// A path node's data parsed, hashed and bounded (its view box, else its bounds); None for any other node.
pub(super) fn parse(p: &Props) -> Option<Parsed> {
    let cmds: Arc<[PathCmd]> = Arc::from(parse_d(p.d.as_deref()?).ok()?);
    Some(Parsed {
        hash: geometry::hash(1, &cmds),
        view_box: geometry::view_box(&cmds, p.view_box),
        cmds,
    })
}

/// A line's cap from its code (0 butt, 1 round, 2 square).
pub(super) fn cap_of(code: u8) -> LineCap {
    match code {
        1 => LineCap::Round,
        2 => LineCap::Square,
        _ => LineCap::Butt,
    }
}

/// A stroke's half width this frame in mesh units.
pub(super) fn half_width(s: &NodeState, units: Units, k: f64) -> f64 {
    let per = match units {
        Units::Path { scale: true, .. } => 1.0,
        Units::Path { fit, .. } | Units::Px { fit } => k / fit.max(1e-12),
    };
    s.stroke_width * per / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corners that grow on a still box are made on that box exactly (only the corners step), and
    /// a box that moves under still corners keeps them exact; held still, both are exact.
    #[test]
    fn only_what_moves_is_stepped() {
        let size = (100.0, 50.0);
        let corners = [10.0, 10.0, 10.0, 10.0, 0.5];
        let grown = [12.0, 12.0, 12.0, 12.0, 0.5];
        let stepped = [step(12.0), step(12.0), step(12.0), step(12.0), 0.5];
        assert_eq!(made_for(None, size, corners), (size, corners));
        assert_eq!(
            made_for(Some((size, corners)), size, corners),
            (size, corners)
        );
        assert_eq!(
            made_for(Some((size, corners)), size, grown),
            (size, stepped)
        );
        let moved = (120.0, 60.0);
        assert_eq!(
            made_for(Some((size, corners)), moved, corners),
            ((step(120.0), step(60.0)), corners)
        );
    }
}
