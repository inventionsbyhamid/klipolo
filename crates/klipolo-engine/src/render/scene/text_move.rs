//! Where a frame's text pieces stand: a node's text placed once a frame
//! (`Placing`: its place, its rest pose's whole-pixel shift, its fit and its animated size, the same for every piece:
//! then each piece moved by its split unit, letter spacing and line height. Arithmetic only.

use klipolo_model::scene::eval::NodeState;

use super::place::{Affine, mul, translate};
use super::text::{NO_UNIT, Piece, TextUnits};
use super::text_fit::{fit, rest_snap};
use super::text_nodes::NodeText;
use super::text_pack::NodeFrame;

/// A node's text placed this frame, for all its pieces: box pixels → target pixels (its node's place, fitted to its
/// box and shifted onto whole pixels at rest), an animated size about the block's anchor, and the fit's scale.
#[derive(Clone, Copy, Debug)]
pub(super) struct Placing {
    pub place: Affine,
    pub sized: Affine,
    pub u: f64,
}

impl Placing {
    /// Text `text` of node `nt` placed for frame `f`.
    pub(super) fn of(nt: &NodeText, text: &TextUnits, f: &NodeFrame) -> Self {
        let [sx, sy] = rest_snap(nt, text, f.rest_here);
        let fitted = fit(nt, text, f.here);
        Self {
            place: mul(&translate(sx, sy), &mul(&f.here.m, &fitted.m)),
            sized: sized(text, f.s, f.rest),
            u: fitted.u,
        }
    }
}

/// Box pixels (where piece `piece`, laid at `rect`, rests) → target pixels this frame, and its unit's opacity: its
/// unit's offsets, scale and turn about the unit's middle, letter spacing and line height that animate, then a size
/// that animates about the block's anchor (`at.sized`), in its node's place (`at.place`).
#[allow(clippy::too_many_arguments)]
pub(super) fn moved(
    nt: &NodeText,
    text: &TextUnits,
    piece: &Piece,
    rect: [f32; 4],
    f: &NodeFrame,
    unit: u16,
    whole_units: bool,
    at: &Placing,
) -> (Affine, f64) {
    let (k, s, rest) = (f.k, f.s, f.rest);
    let finite = |v: f64, or: f64| if v.is_finite() { v } else { or };
    let mut m = at.sized;
    // Letter spacing and line height that animate move each glyph and line from where they were laid out.
    let ls = finite(
        (s.letter_spacing - rest.letter_spacing) * k * f64::from(text.scale),
        0.0,
    );
    let lh = finite(
        (s.line_height - rest.line_height) * f64::from(text.font_px),
        0.0,
    );
    if ls != 0.0 || lh != 0.0 {
        let glyphs = f64::from(
            text.line_glyphs
                .get(usize::from(piece.line))
                .copied()
                .unwrap_or(1),
        );
        let lines = text.lines.len().max(1) as f64;
        let share = f64::from(text.align_share);
        let dx = ls * (f64::from(piece.col) - share * (glyphs - 1.0));
        let dy = lh * (f64::from(piece.line) - (lines - 1.0) / 2.0);
        m = mul(&m, &translate(dx, dy));
    }
    let Some(u) = f.units.get(usize::from(unit)).filter(|_| unit != NO_UNIT) else {
        return (mul(&at.place, &m), 1.0);
    };
    let r = if whole_units {
        text.unit_rects
            .get(usize::from(unit))
            .copied()
            .unwrap_or(rect)
    } else {
        rect
    };
    let (ox, oy) = (f64::from(r[0] + r[2]) / 2.0, f64::from(r[1] + r[3]) / 2.0);
    let (bw, bh) = (f64::from(nt.box_px.0), f64::from(nt.box_px.1));
    let (tx, ty) = (u.x * bw + u.x_px * k, u.y * bh + u.y_px * k);
    let (sx, sy) = (u.scale * u.scale_x, u.scale * u.scale_y);
    let (sin, cos) = finite(u.rotate, 0.0).to_radians().sin_cos();
    let turn: Affine = [cos * sx, sin * sx, -sin * sy, cos * sy, 0.0, 0.0];
    let unit = mul(
        &mul(&translate(ox + tx, oy + ty), &turn),
        &translate(-ox, -oy),
    );
    (mul(&at.place, &mul(&m, &unit)), u.opacity)
}

/// A size that animates (`s` against its rest `rest`), about the block's anchor (the identity at rest).
fn sized(text: &TextUnits, s: &NodeState, rest: &NodeState) -> Affine {
    let r = if rest.size > 0.0 {
        s.size / rest.size
    } else {
        1.0
    };
    if !r.is_finite() || (r - 1.0).abs() < 1e-9 {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let [ax, ay] = text.anchor.map(f64::from);
    [r, 0.0, 0.0, r, ax - r * ax, ay - r * ay]
}
