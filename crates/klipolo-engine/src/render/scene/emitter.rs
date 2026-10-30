//! Particles: an emitter's live particles, from the evaluator's analytic state (seeded: the same in every
//! render), each drawn as its particle shape (a rect, ellipse, star, polygon, path or image, an emoji among them through
//! the text pipeline's emoji path) where the emitter's box puts it, turned and scaled by its own spin and size
//! over life, faded by its opacity over life. Drawn where the shape sits in paint order, after the emitter's children.
//! At most [`MAX_EMITTERS`] emitters a frame draw (past them, the frame says `frame.sceneReduced`), and the evaluator
//! writes at most 500 particles a frame over all of them; consecutive particles share their run. Arithmetic into room
//! made with the instance: nothing allocates.

use klipolo_model::scene::eval::NodeState;

use super::pack::{PackIn, Packed};
use super::place::{Affine, World, box_of, mul, translate};
use super::statics::NodeStatic;

/// The most emitters whose particles a frame draws.
pub(super) const MAX_EMITTERS: usize = 32;

/// Particle shape `node` (its state `s`, static part `st`) drawn at every live particle of its emitter.
pub(super) fn particles(p: &mut Packed, f: &PackIn, node: usize, s: &NodeState, st: &NodeStatic) {
    let Some(emitter) = f.inst.parent(node) else {
        return;
    };
    let Some(e) = p.world.get(emitter).copied() else {
        return;
    };
    let mut first = true;
    let k = f.drawn.k;
    let [_, _, w, h] = box_of(st, (e.w, e.h), k);
    for q in f.frame.particles() {
        if q.emitter as usize != emitter {
            continue;
        }
        if first {
            first = false;
            if p.emitters_drawn >= MAX_EMITTERS {
                p.reduced = true;
                return;
            }
            p.emitters_drawn += 1;
        }
        let opacity = e.opacity * s.opacity * q.opacity;
        if opacity.is_nan() || opacity <= 0.0 {
            continue;
        }
        let (cx, cy) = (q.u * e.w + q.dx_px * k, q.v * e.h + q.dy_px * k);
        let (sin, cos) = (q.rotate + s.rotate).to_radians().sin_cos();
        let sc = q.scale * s.scale;
        let turn: Affine = [cos * sc, sin * sc, -sin * sc, cos * sc, 0.0, 0.0];
        let local = mul(
            &mul(&translate(cx, cy), &turn),
            &translate(-w / 2.0, -h / 2.0),
        );
        let here = World {
            m: mul(&e.m, &local),
            w,
            h,
            opacity,
            clips: e.clips,
        };
        p.node(f, node, &here, &here, s, st);
    }
}
