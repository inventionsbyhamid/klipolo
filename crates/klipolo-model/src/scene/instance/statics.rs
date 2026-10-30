//! An instance node's resting values: its static properties with its settings resolved, held to their ranges, and
//! its own clip.

use super::super::eval::{ClipShape, ClipState, NodeState};
use super::super::timeline::{lower_of, Lower, Prop};
use super::super::units::{parse_clip, parse_origin, Clip, Length};
use super::super::{Node, Origin, Paint, Radius};
use super::Ctx;
use crate::ease_names;
use crate::motion::Ease;

/// A length as `[fraction, px]`.
pub(crate) fn length(l: Length) -> [f64; 2] {
    match l {
        Length::Fraction(f) => [f, 0.0],
        Length::Px(p) => [0.0, p],
    }
}

/// A clip with its lengths as `[fraction, px]`.
pub(crate) fn clip_state(c: Clip) -> ClipState {
    match c {
        Clip::Inset {
            top,
            right,
            bottom,
            left,
            round,
        } => ClipState {
            shape: ClipShape::Inset,
            v: [
                length(top),
                length(right),
                length(bottom),
                length(left),
                length(round),
            ],
        },
        Clip::Circle { r, x, y } => ClipState {
            shape: ClipShape::Circle,
            v: [length(r), length(x), length(y), [0.0; 2], [0.0; 2]],
        },
    }
}

/// An ease named in a scene, or `default` when it names none Klipolo draws.
pub(crate) fn ease(name: Option<&str>, default: Ease) -> Ease {
    name.and_then(|n| ease_names::read(n).ok())
        .map(|(e, _)| e)
        .filter(|e| e.is_valid())
        .unwrap_or(default)
}

/// The node's resting values and its own clip.
pub(crate) fn base(node: &Node, ctx: &Ctx) -> (NodeState, ClipState) {
    let p = &node.props;
    let mut s = NodeState::REST;
    for (v, name) in [(&p.x, "x"), (&p.y, "y")] {
        if let (Some(v), Some(Lower::Pos(f, px))) = (v, lower_of(name)) {
            if let Some([a, b]) = ctx.len(v, false) {
                s.set(f, a);
                s.set(px, b);
            }
        }
    }
    if let Some(o) = &p.origin {
        let pair = match o {
            Origin::Pair([a, b]) => ctx.len(a, false).zip(ctx.len(b, false)),
            Origin::Css(css) => parse_origin(css).map(|(a, b)| (length(a), length(b))),
        };
        if let Some((a, b)) = pair {
            s.set(Prop::OriginX, a[0]);
            s.set(Prop::OriginXPx, a[1]);
            s.set(Prop::OriginY, b[0]);
            s.set(Prop::OriginYPx, b[1]);
        }
    }
    let numbers = [
        (&p.scale, Prop::Scale),
        (&p.scale_x, Prop::ScaleX),
        (&p.scale_y, Prop::ScaleY),
        (&p.rotate, Prop::Rotate),
        (&p.rotate_x, Prop::RotateX),
        (&p.rotate_y, Prop::RotateY),
        (&p.perspective, Prop::Perspective),
        (&p.opacity, Prop::Opacity),
        (&p.blur, Prop::Blur),
        (&p.stroke_width, Prop::StrokeWidth),
        (&p.dash_offset, Prop::DashOffset),
        (&p.inner_radius, Prop::InnerRadius),
        (&p.size, Prop::Size),
        (&p.letter_spacing, Prop::LetterSpacing),
        (&p.line_height, Prop::LineHeight),
    ];
    for (v, prop) in numbers {
        if let Some(n) = v.as_ref().and_then(|v| ctx.num(v)) {
            s.set(prop, n);
        }
    }
    if let Some(f) = p.visible.as_ref().and_then(|f| ctx.flag(f)) {
        s.visible = f;
    }
    paint(
        &p.fill,
        ctx,
        &mut s,
        [Prop::FillR, Prop::FillG, Prop::FillB, Prop::FillA],
        true,
    );
    paint(
        &p.stroke,
        ctx,
        &mut s,
        [Prop::StrokeR, Prop::StrokeG, Prop::StrokeB, Prop::StrokeA],
        false,
    );
    if let Some(trim) = &p.trim {
        for (v, prop) in [
            (&trim.start, Prop::TrimStart),
            (&trim.end, Prop::TrimEnd),
            (&trim.offset, Prop::TrimOffset),
        ] {
            if let Some(n) = v.as_ref().and_then(|v| ctx.num(v)) {
                s.set(prop, n);
            }
        }
    }
    match &p.radius {
        Some(Radius::All(v)) => {
            if let Some(n) = ctx.num(v) {
                s.set(Prop::Radius, n);
            }
        }
        Some(Radius::Corners(c)) => {
            for (slot, v) in c.iter().enumerate() {
                if let Some(n) = ctx.num(v) {
                    s.radius[slot] = Prop::Radius.clamp(n);
                }
            }
        }
        None => {}
    }
    if let Some(sh) = &p.shadow {
        for (v, prop) in [
            (&sh.x, Prop::ShadowX),
            (&sh.y, Prop::ShadowY),
            (&sh.blur, Prop::ShadowBlur),
            (&sh.spread, Prop::ShadowSpread),
        ] {
            if let Some(n) = v.as_ref().and_then(|v| ctx.num(v)) {
                s.set(prop, n);
            }
        }
        if let Some(c) = sh.color.as_deref().and_then(|c| ctx.color(c)) {
            s.shadow.color = c;
        }
    }
    if let Some(n) = p
        .motion_path
        .as_ref()
        .and_then(|m| m.progress.as_ref())
        .and_then(|v| ctx.num(v))
    {
        s.set(Prop::Progress, n);
    }
    if let Some(c) = p.tint.as_deref().and_then(|c| ctx.color(c)) {
        s.tint = c;
    }
    let clip = p
        .clip
        .as_deref()
        .and_then(parse_clip)
        .map_or(ClipState::default(), clip_state);
    s.clip = clip;
    (s, clip)
}

/// A fill or stroke's resting colour: the colour, white under a gradient or generator (theirs are the node's own), and
/// for a fill black when unset (as in SVG), for a stroke none.
fn paint(p: &Option<Paint>, ctx: &Ctx, s: &mut NodeState, props: [Prop; 4], fill: bool) {
    let rgba = match p {
        Some(Paint::Color(c)) => ctx.color(c),
        Some(Paint::Gradient(g)) => {
            if fill {
                if let Some(n) = g.offset.as_ref().and_then(|v| ctx.num(v)) {
                    s.set(Prop::FillOffset, n);
                }
                if let Some(n) = g.angle.as_ref().and_then(|v| ctx.num(v)) {
                    s.set(Prop::FillAngle, n);
                }
            }
            Some([1.0; 4])
        }
        Some(Paint::Generator(_)) => Some([1.0; 4]),
        None => None,
    };
    if let Some(c) = rgba {
        for (prop, v) in props.into_iter().zip(c) {
            s.set(prop, f64::from(v));
        }
    }
}
