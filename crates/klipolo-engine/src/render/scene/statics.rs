//! What each node of a scene instance draws, resolved once when the instance is built (never per frame): its look
//! (rect, ellipse, star, polygon, line, image, path, polyline, text, emoji and backdrop), its box,
//! its fill's and stroke's gradients in the instance's table (`gradients.rs`, its spans' too), its dashes, whether its
//! stroke is drawn as a path (a trimmed or dashed shape's) and its motion path's fit; its kind, blend, mask,
//! generator fill, where its subtree ends, and whether it is an emitter's particle shape.

use std::sync::Arc;

use klipolo_model::motion::Blend;
use klipolo_model::scene::instance::{Field, SceneInstance};
use klipolo_model::scene::timeline::Prop;
use klipolo_model::scene::{LineCap, NodeType, Paint};

use super::draws::Dash;
use super::gradients::{first_stop, gradient, intern, least_alpha, span_gradients};
use super::groups::MaskOf;
use super::motion::{MotionFit, motion_fit};
use super::shapes::GpuGradient;

/// What a node draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Look {
    /// Nothing of its own: a group, repeat or emitter passes its place down; texts, paths and particles are drawn by
    /// later tasks.
    None,
    Rect,
    Ellipse,
    Star {
        points: f64,
    },
    Polygon {
        sides: f64,
    },
    /// Cap: 0 butt, 1 round, 2 square.
    Line {
        cap: f32,
    },
    /// The layer's image at this index (`SceneLayer::images`).
    Image(u16),
    /// Drawn by the paths pipeline.
    Path,
    Polyline,
    /// Drawn by the text pipeline: a text node, and an image node showing an emoji.
    Text,
    Emoji,
    /// The frame under the layer, inside its rounded box.
    Backdrop,
}

/// One node's resolved static part.
#[derive(Clone, Copy, Debug)]
pub(super) struct NodeStatic {
    pub look: Look,
    /// Its box in its parent's: x, y, w and h, each a fraction of the parent's box and 1080p pixels.
    pub bounds: [[f64; 2]; 4],
    /// Indices into the instance's gradient table (−1: none).
    pub fill_gradient: i32,
    pub stroke_gradient: i32,
    /// The stroke gradient's own turn (radians) and slide (a fill's animate as the node's `fill_angle`/`fill_offset`).
    pub stroke_turn: f32,
    pub stroke_slide: f32,
    /// A gradient the table had no room for (an unchecked scene past 32) draws its first stop's colour: straight RGBA.
    pub fill_solid: Option<[f32; 4]>,
    pub stroke_solid: Option<[f32; 4]>,
    /// The fill gradient's most transparent stop's alpha (1 without one): how much its shadow shows through it.
    pub fill_stop_alpha: f32,
    /// The stroke gradient's most transparent stop's alpha (1 without one): under 1 its stroke isn't opaque.
    pub stroke_stop_alpha: f32,
    /// A rect's, ellipse's, star's, polygon's or line's stroke drawn as a path, for its trim or dashes.
    pub outline: bool,
    pub dash: Option<Dash>,
    pub motion: Option<MotionFit>,
    /// Its type, its blend with what is under it, and its mask (a group's).
    pub kind: NodeType,
    pub blend: Blend,
    pub mask: Option<MaskOf>,
    /// The generator it is filled with: its index in the scene's `generator_fills` (−1 none).
    pub fill: i16,
    /// Its fill and stroke are drawn apart (a path's, or a trimmed or dashed shape's): at fractional opacity the fill
    /// would show through the stroke unless it is drawn offscreen.
    pub split_stroke: bool,
    /// An emitter's particle shape: drawn at each particle, never where it sits.
    pub particle: bool,
    /// The instance node after its subtree (children, then particle shapes).
    pub end: u32,
    /// Where its spans' gradient indices start in the instance's span list (`statics`' third part).
    pub span_from: u32,
}

impl NodeStatic {
    const NONE: Self = Self {
        look: Look::None,
        bounds: WHOLE,
        fill_gradient: -1,
        stroke_gradient: -1,
        stroke_turn: 0.0,
        stroke_slide: 0.0,
        fill_solid: None,
        stroke_solid: None,
        fill_stop_alpha: 1.0,
        stroke_stop_alpha: 1.0,
        outline: false,
        dash: None,
        motion: None,
        kind: NodeType::Group,
        blend: Blend::Normal,
        mask: None,
        fill: -1,
        split_stroke: false,
        particle: false,
        end: 0,
        span_from: 0,
    };
}

/// An instance's static part: each node's, the gradient table they index, and every text node's spans' gradients.
pub(super) type Statics = (Vec<NodeStatic>, Vec<GpuGradient>, Vec<i32>);

/// Every instance node's static part, the gradient table they index, and the spans' gradients.
pub(super) fn statics(inst: &SceneInstance, images: &[(Arc<str>, Arc<str>)]) -> Statics {
    let compiled = inst.compiled();
    let mut table: Vec<GpuGradient> = Vec::new();
    let mut spans: Vec<i32> = Vec::new();
    let mut out = Vec::with_capacity(inst.len());
    for i in 0..inst.len() {
        let Some(cn) = inst.cnode(i).and_then(|c| compiled.nodes.get(c)) else {
            out.push(NodeStatic {
                end: i as u32 + 1,
                ..NodeStatic::NONE
            });
            continue;
        };
        let node = &cn.node;
        let p = &node.props;
        let sides = |default: f64| {
            p.sides
                .as_ref()
                .and_then(|v| inst.number_of(i, Field::Sides, v))
                .unwrap_or(default)
        };
        let look = match node.kind {
            NodeType::Rect => Look::Rect,
            NodeType::Ellipse => Look::Ellipse,
            NodeType::Star => Look::Star { points: sides(5.0) },
            NodeType::Polygon => Look::Polygon { sides: sides(6.0) },
            NodeType::Line => Look::Line {
                cap: match p.cap {
                    Some(LineCap::Round) => 1.0,
                    Some(LineCap::Square) => 2.0,
                    _ => 0.0,
                },
            },
            NodeType::Image if p.emoji.is_some() && p.asset.is_none() => Look::Emoji,
            NodeType::Text => Look::Text,
            NodeType::Image => p
                .asset
                .as_deref()
                .and_then(|s| inst.asset_of(i, s))
                .and_then(|id| images.iter().position(|(a, _)| &**a == id))
                .map_or(Look::None, |k| Look::Image(k as u16)),
            NodeType::Path if p.d.is_some() => Look::Path,
            NodeType::Polyline if p.points.is_some() => Look::Polyline,
            NodeType::Backdrop => Look::Backdrop,
            _ => Look::None,
        };
        let trims = |prop: Prop| {
            matches!(
                prop,
                Prop::TrimStart | Prop::TrimEnd | Prop::TrimOffset | Prop::DashOffset
            )
        };
        let shape = matches!(
            look,
            Look::Rect
                | Look::Ellipse
                | Look::Star { .. }
                | Look::Polygon { .. }
                | Look::Line { .. }
        );
        let outline = shape && (p.trim.is_some() || p.dash.is_some() || inst.animates(i, trims));
        let mut bounds = WHOLE;
        if let Some(b) = &node.bounds {
            let fields = [Field::BoxX, Field::BoxY, Field::BoxW, Field::BoxH];
            for ((slot, v), field) in bounds.iter_mut().zip(b).zip(fields) {
                if let Some(l) = inst.length_of(i, field, v) {
                    *slot = l;
                }
            }
        }
        // A gradient's index in the table, or (no room) its first stop's colour.
        let mut index = |paint: &Option<Paint>| match paint {
            Some(Paint::Gradient(g)) if look != Look::None => match gradient(inst, i, g) {
                Some(gpu) => match intern(&mut table, gpu) {
                    -1 => (-1, Some(first_stop(&gpu))),
                    k => (k, None),
                },
                None => (-1, None),
            },
            _ => (-1, None),
        };
        let ((fill_gradient, fill_solid), (stroke_gradient, stroke_solid)) =
            (index(&p.fill), index(&p.stroke));
        let fill_stop_alpha = usize::try_from(fill_gradient)
            .ok()
            .and_then(|g| table.get(g))
            .map_or(1.0, least_alpha);
        let stroke_stop_alpha = usize::try_from(stroke_gradient)
            .ok()
            .and_then(|g| table.get(g))
            .map_or(1.0, least_alpha);
        let (mut stroke_turn, mut stroke_slide) = (0.0, 0.0);
        if let Some(Paint::Gradient(g)) = &p.stroke {
            let read = |v: &Option<klipolo_model::scene::Val>, field| {
                v.as_ref().and_then(|v| inst.number_of(i, field, v))
            };
            stroke_turn =
                read(&g.angle, Field::GradientAngle).map_or(0.0, |a| a.to_radians() as f32);
            stroke_slide = read(&g.offset, Field::GradientOffset).map_or(0.0, |o| o as f32);
        }
        let fill = match &p.fill {
            Some(Paint::Generator(g)) => compiled
                .facts
                .generator_fills
                .iter()
                .position(|id| *id == g.generator)
                .map_or(-1, |k| k as i16),
            _ => -1,
        };
        let span_from = if look == Look::Text {
            span_gradients(inst, i, p.spans.as_deref(), &mut table, &mut spans)
        } else {
            spans.len() as u32
        };
        out.push(NodeStatic {
            look,
            bounds,
            fill_gradient,
            stroke_gradient,
            stroke_turn,
            stroke_slide,
            fill_solid,
            stroke_solid,
            fill_stop_alpha,
            stroke_stop_alpha,
            outline,
            dash: p.dash.as_deref().and_then(Dash::of),
            motion: motion_fit(inst, i, p),
            kind: node.kind,
            blend: p.blend.unwrap_or_default(),
            mask: super::masks::mask_of(inst, i),
            fill,
            split_stroke: look == Look::Path || (outline && !matches!(look, Look::Line { .. })),
            particle: cn.particle,
            end: i as u32 + 1,
            span_from,
        });
    }
    // Each subtree ends after its last descendant: children follow their parent in paint order.
    for j in (0..out.len()).rev() {
        let end = out[j].end;
        if let Some(p) = inst.parent(j).and_then(|p| out.get_mut(p)) {
            p.end = p.end.max(end);
        }
    }
    (out, table, spans)
}

/// The whole parent's box.
const WHOLE: [[f64; 2]; 4] = [[0.0, 0.0], [0.0, 0.0], [1.0, 0.0], [1.0, 0.0]];
