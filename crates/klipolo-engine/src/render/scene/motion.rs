//! A node on its motion path: where its `progress` puts its origin in its parent's box, and which way
//! the path heads there (`autoRotate`). The path's table is built with the instance (`klipolo_model::scene::eval::
//! motion_path`); here it is fitted into the box it moves in, as a `path` node's `d` is (its `viewBox`, else its own
//! bounds; `meet` unless `none`). `"#id"` rides that path node's `d`, `viewBox` and box (as placed in the mover's
//! parent). A frame reads one table and allocates nothing.

use klipolo_model::scene::eval::{NodeState, motion_path};
use klipolo_model::scene::instance::{Field, SceneInstance};
use klipolo_model::scene::{AspectRatio, Props};

use super::geometry::Fit;

/// How a node's motion path fits the box it moves in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MotionFit {
    pub view_box: [f64; 4],
    pub meet: bool,
    /// The box the path fills, in the parent's: x, y, w and h, each a fraction of the parent's box and 1080p pixels.
    pub bounds: [[f64; 2]; 4],
    pub auto_rotate: bool,
}

/// The whole parent's box.
const WHOLE: [[f64; 2]; 4] = [[0.0, 0.0], [0.0, 0.0], [1.0, 0.0], [1.0, 0.0]];

/// Instance node `i`'s motion path fit (`p` its properties), when it has one that draws.
pub(super) fn motion_fit(inst: &SceneInstance, i: usize, p: &Props) -> Option<MotionFit> {
    let mp = p.motion_path.as_ref()?;
    let table = inst.motion_table(i)?;
    let auto_rotate = mp.auto_rotate == Some(true);
    let Some(id) = mp.path.strip_prefix('#') else {
        return Some(MotionFit {
            view_box: mp.view_box.unwrap_or(table.bounds),
            meet: mp.preserve_aspect_ratio != Some(AspectRatio::Stretch),
            bounds: WHOLE,
            auto_rotate,
        });
    };
    let c = inst.compiled().find(id)?;
    let target = &inst.compiled().nodes.get(c)?.node;
    let at = inst.instances_of(c).next();
    let mut bounds = WHOLE;
    if let (Some(b), Some(at)) = (&target.bounds, at) {
        let fields = [Field::BoxX, Field::BoxY, Field::BoxW, Field::BoxH];
        for ((slot, v), field) in bounds.iter_mut().zip(b).zip(fields) {
            if let Some(l) = inst.length_of(at, field, v) {
                *slot = l;
            }
        }
    }
    let tp = &target.props;
    Some(MotionFit {
        view_box: tp.view_box.unwrap_or(table.bounds),
        meet: tp.preserve_aspect_ratio != Some(AspectRatio::Stretch),
        bounds,
        auto_rotate,
    })
}

/// Where node `i` (state `s`) is on its motion path this frame, in its parent's box pixels (`pw` × `ph`, `k` pixels per
/// 1080p pixel), and the turn the path adds (degrees, 0 without `autoRotate`).
pub(super) fn motion_at(
    inst: &SceneInstance,
    i: usize,
    s: &NodeState,
    fit: &MotionFit,
    (pw, ph): (f64, f64),
    k: f64,
) -> Option<(f64, f64, f64)> {
    let (x, y, heading) = motion_path(inst, i, s.progress)?;
    let [bx, by, bw, bh] = fit.bounds;
    let (x0, y0) = (bx[0] * pw + bx[1] * k, by[0] * ph + by[1] * k);
    let (w, h) = (
        (bw[0] * pw + bw[1] * k).max(0.0),
        (bh[0] * ph + bh[1] * k).max(0.0),
    );
    let f = Fit::of(fit.view_box, fit.meet, w, h);
    let (cx, cy) = f.apply(x, y);
    let turn = if fit.auto_rotate {
        let (sin, cos) = heading.to_radians().sin_cos();
        (f.sy * sin).atan2(f.sx * cos).to_degrees()
    } else {
        0.0
    };
    Some((x0 + w / 2.0 + cx, y0 + h / 2.0 + cy, turn))
}
