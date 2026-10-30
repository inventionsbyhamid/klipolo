//! Where a node's laid-out text stands in its box this frame: fitted to the box as
//! the frame has it, evenly, never squashed, and shifted by the constant that puts it on whole target pixels
//! at its rest pose. Arithmetic only.

use super::place::{Affine, IDENTITY, World, mul};
use super::text::TextUnits;
use super::text_nodes::NodeText;

/// How a text laid out in its node's box stands in the box as this frame has it: `m` (box pixels to box pixels) scales
/// it evenly by `u`, the smaller of the box's width and height ratios, about its anchor (its alignment's edge across,
/// its middle down). The identity at rest; under motion a box's px parts, or a stand-in of another size, scale it
/// without squashing its glyphs.
pub(super) struct Fitted {
    pub m: Affine,
    pub u: f64,
}

/// Text `text` of node `nt` fitted to the box `here` has.
pub(super) fn fit(nt: &NodeText, text: &TextUnits, here: &World) -> Fitted {
    let ratio = |now: f64, laid: f32| {
        let r = now / f64::from(laid);
        // Within the box's f32 rounding of 1: exactly 1.
        if laid > 0.0 && r.is_finite() && r > 0.0 && (r - 1.0).abs() > 1e-6 {
            r
        } else {
            1.0
        }
    };
    let (fx, fy) = (ratio(here.w, nt.box_px.0), ratio(here.h, nt.box_px.1));
    if fx == 1.0 && fy == 1.0 {
        return Fitted {
            m: IDENTITY,
            u: 1.0,
        };
    }
    let u = fx.min(fy);
    let (bw, bh) = (f64::from(nt.box_px.0), f64::from(nt.box_px.1));
    let share = f64::from(text.align_share);
    Fitted {
        m: [
            u,
            0.0,
            0.0,
            u,
            share * (here.w - u * bw),
            (here.h - u * bh) / 2.0,
        ],
        u,
    }
}

/// The constant shift, in target pixels, that puts text `text` of node `nt` on whole target pixels at its rest pose
/// (`rest`: its place with nothing moving), when that pose only translates it. Every frame takes it, so text at rest
/// is sampled texel for pixel and text in motion glides by fractions of a pixel, in step with its card.
pub(super) fn rest_snap(nt: &NodeText, text: &TextUnits, rest: &World) -> [f64; 2] {
    let r = mul(&rest.m, &fit(nt, text, rest).m);
    let near = |v: f64, to: f64| (v - to).abs() < 1e-9;
    let moves_only = near(r[0], 1.0) && near(r[1], 0.0) && near(r[2], 0.0) && near(r[3], 1.0);
    if moves_only && r[4].is_finite() && r[5].is_finite() {
        [r[4].round() - r[4], r[5].round() - r[5]]
    } else {
        [0.0, 0.0]
    }
}
