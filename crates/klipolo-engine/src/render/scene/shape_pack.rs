//! One node as shapes for the GPU: its SDF shape and its analytic
//! shadow before it.

use klipolo_model::scene::eval::NodeState;

use super::place::{World, centred, mul, translate};
use super::shapes::{ShapeInstance, kind};
use super::statics::{Look, NodeStatic};

/// Node `s` (static part `st`, place `here`) as a shape, its shadow first; an image's `picture` dimensions contain its
/// picture in its box.
pub(super) fn push_shape(
    shapes: &mut Vec<ShapeInstance>,
    here: &World,
    s: &NodeState,
    st: &NodeStatic,
    look: Look,
    k: f64,
    picture: Option<(u32, u32)>,
) {
    let m = centred(here);
    let mut half = [here.w / 2.0, here.h / 2.0];
    if let Some((pw, ph)) = picture {
        let (pw, ph) = (f64::from(pw.max(1)), f64::from(ph.max(1)));
        let fit = (here.w / pw).min(here.h / ph);
        half = [pw * fit / 2.0, ph * fit / 2.0];
    }
    // CSS order to the shader's: top right, bottom right, top left, bottom left.
    let r = s.radius.map(|v| (v * k).max(0.0));
    let radius = [r[1], r[2], r[0], r[3]];
    let stroke_w = if s.stroke[3] > 0.0 && !st.outline {
        (s.stroke_width * k).max(0.0)
    } else {
        0.0
    };
    let (shape_kind, points, inner, cap) = match look {
        Look::Rect => (kind::RECT, 0.0, 0.0, 0.0),
        Look::Ellipse => (kind::ELLIPSE, 0.0, 0.0, 0.0),
        Look::Star { points } => (kind::STAR, points, s.inner_radius, 0.0),
        Look::Polygon { sides } => (
            kind::STAR,
            sides,
            (std::f64::consts::PI / sides.max(3.0)).cos(),
            0.0,
        ),
        Look::Line { cap } => (kind::LINE, 0.0, 0.0, cap),
        Look::Image(_) => (kind::IMAGE, 0.0, 0.0, 0.0),
        Look::None | Look::Path | Look::Polyline | Look::Text | Look::Emoji | Look::Backdrop => {
            return;
        }
    };
    let tinted = |solid: Option<[f32; 4]>, state: [f32; 4]| {
        solid.map_or(state, |c| [0, 1, 2, 3].map(|j| c[j] * state[j]))
    };
    let [c1, c2] = here.clips;
    let base = ShapeInstance {
        xf: [m[0], m[1], m[2], m[3]].map(|v| v as f32),
        at_half: [m[4], m[5], half[0], half[1]].map(|v| v as f32),
        shape: [
            shape_kind,
            points as f32,
            inner as f32,
            (stroke_w / 2.0 + 1.5) as f32,
        ],
        radius: radius.map(|v| v as f32),
        fill: if picture.is_some() {
            [1.0; 4]
        } else {
            tinted(st.fill_solid, s.fill)
        },
        stroke: tinted(st.stroke_solid, s.stroke),
        paint: [
            st.fill_gradient as f32,
            st.stroke_gradient as f32,
            s.fill_angle.to_radians() as f32,
            s.fill_offset as f32,
        ],
        misc: [stroke_w as f32, here.opacity as f32, cap, st.stroke_turn],
        clip_m: c1.m,
        clip_t: c1.t,
        clip: c1.c,
        clip2_m: c2.m,
        clip2_t: c2.t,
        clip2: c2.c,
        extra: [st.stroke_slide, 0.0, 0.0, 0.0],
    };
    // The least alpha its fill can have where it covers: an image's picture is taken as opaque, a gradient's tint by its
    // most transparent stop.
    let flat_alpha = match look {
        Look::Image(_) => 1.0,
        _ if st.fill_gradient >= 0 => base.fill[3] * st.fill_stop_alpha,
        _ => base.fill[3],
    };
    if let Some(shadow) = shadow_of(&base, s, look, k, flat_alpha) {
        shapes.push(shadow);
    }
    shapes.push(base);
}

/// A rect's, an ellipse's or an image's drop shadow (offset and spread in node space, as CSS's turns with its box),
/// drawn before it and kept outside it (a translucent fill doesn't show its own shadow), exactly for its
/// shape's `flat_alpha`; other shapes' shadows are drawn by the group pass.
fn shadow_of(
    base: &ShapeInstance,
    s: &NodeState,
    look: Look,
    k: f64,
    flat_alpha: f32,
) -> Option<ShapeInstance> {
    let sh = s.shadow;
    let visible =
        sh.color[3] > 0.0 && (sh.blur > 0.0 || sh.spread != 0.0 || sh.x != 0.0 || sh.y != 0.0);
    if !visible || !matches!(look, Look::Rect | Look::Ellipse | Look::Image(_)) {
        return None;
    }
    let (hx, hy) = (f64::from(base.at_half[2]), f64::from(base.at_half[3]));
    let spread = sh.spread * k;
    let half = [(hx + spread).max(0.0), (hy + spread).max(0.0)];
    let ellipse = look == Look::Ellipse;
    let corner = if ellipse {
        half[0].min(half[1])
    } else {
        (f64::from(base.radius[0]) + spread).max(0.0)
    };
    let sigma = (sh.blur * k / 2.0).max(0.0);
    let (dx, dy) = (sh.x * k, sh.y * k);
    let node: [f64; 6] = [
        base.xf[0],
        base.xf[1],
        base.xf[2],
        base.xf[3],
        base.at_half[0],
        base.at_half[1],
    ]
    .map(f64::from);
    let m = mul(&node, &translate(dx, dy));
    Some(ShapeInstance {
        xf: [m[0], m[1], m[2], m[3]].map(|v| v as f32),
        at_half: [m[4], m[5], half[0], half[1]].map(|v| v as f32),
        shape: [
            kind::SHADOW,
            sigma as f32,
            corner as f32,
            (3.0 * sigma + 1.5) as f32,
        ],
        // The shape it stays outside of, in the shadow's space: its centre, half size, corners and kind.
        fill: [-dx, -dy, hx, hy].map(|v| v as f32),
        stroke: sh.color,
        paint: [-1.0, -1.0, 0.0, 0.0],
        misc: [
            0.0,
            base.misc[1],
            if ellipse { 1.0 } else { 0.0 },
            flat_alpha,
        ],
        ..*base
    })
}
