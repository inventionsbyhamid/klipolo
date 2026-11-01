//! A node's properties: one flat set of optional fields, each named as CSS and SVG name it. Which of them a
//! node type takes is [`super::keys`]'s table; the loader refuses any other key with a close name.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::text::*;
use super::values::*;
use super::{Node, StaggerArg, Val};
use crate::motion::Blend;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Props {
    // Transforms and visibility (every node).
    /// Moves the node: a fraction of its parent's box, `"50%"`, or `"22px"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Val>,
    /// What it scales and turns about, in its own box: `"50% 0%"`, `"center"`, `[0.5, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_x: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_y: Option<Val>,
    /// Degrees, clockwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<Val>,
    /// Groups: a 3D tilt in degrees, seen through `perspective` (1080p pixels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate_x: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate_y: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perspective: Option<Val>,
    /// 0–1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<Flag>,

    // Paint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Paint>,
    /// 1080p pixels; a `path`'s are in its own units and scale with it, as in SVG, unless `strokeScale` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<LineCap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<LineJoin>,
    /// Dash and gap lengths in 1080p pixels (a `path`'s in its stroke's units), 1 to 4 of them (an odd list repeats,
    /// as in SVG). Dashes are cut square; the line's caps are its ends'.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash_offset: Option<Val>,
    /// Draws only part of the stroke (draw-on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<Trim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<Radius>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
    /// `inset(t r b l round r)` or `circle(r at x y)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    /// Groups: another node's alpha or luma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<MaskRef>,
    /// 1080p pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<Blend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_path: Option<MotionPath>,
    /// Per property name: a wiggle, an oscillation or a marquee added to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<BTreeMap<String, Modifier>>,

    // Shapes.
    /// A polyline's points: a `"$table"` setting or `[[x, y], …]` fractions of the box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Points>,
    /// A star's inner points, as a fraction of its outer radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_radius: Option<Val>,
    /// A polygon's sides or a star's points, 3–64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sides: Option<Val>,
    /// SVG path data, in the path's own units: its `viewBox` (else its own bounds) fills the node's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    /// A path's `[x, y, width, height]` that fills its box; its gradients are fractions of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_box: Option<[f64; 4]>,
    /// `"xMidYMid meet"` (the default: whole and centred) or `"none"` (stretched to the box).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_aspect_ratio: Option<AspectRatio>,
    /// False: a path's stroke and dashes are 1080p pixels, whatever the path's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_scale: Option<bool>,
    /// `"nonzero"` (the default) or `"evenodd"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_rule: Option<FillRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth: Option<bool>,

    // Text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Parts with their own look, instead of `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spans: Option<Vec<Span>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<SceneFont>,
    /// 1080p pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Val>,
    /// 100–900; `font`'s own weight wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<Flag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    /// The width lines wrap at, a fraction of the box or `"640px"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<Val>,
    /// A multiple of the size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<TextCase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Outline>,
    /// Animates characters, words or lines one by one (with `stagger`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Split>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<StaggerArg>,
    /// Counts from one number to another (the value animator).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<Count>,
    /// Types the text on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal: Option<Reveal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scramble: Option<Scramble>,
    /// Text that changes at given times ("SUBSCRIBE" → "SUBSCRIBED").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holds: Option<Vec<HoldKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,

    // Images.
    /// An image setting: `"$logo"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Drawn by the system's emoji face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// Refused until packs exist: a library folder can't hold files. Left out of the schema, so agents aren't
    /// offered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub file: Option<String>,

    // Repeats.
    /// A fixed number of instances, 0–1,000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<Val>,
    /// One instance per row of a table setting: `"$data"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<String>,
    /// How far each instance moves from the one before: `[dx, dy]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<[Val; 2]>,

    // SVG (submissions only).
    /// The name of an SVG in the request's `svgs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,

    // Emitters.
    /// Particles at once, at `at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<Val>,
    /// Particles a second, from `at` on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<Val>,
    /// When it starts, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone: Option<Cone>,
    /// 1080p pixels a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<NumRange>,
    /// 1080p pixels a second, a second (down is positive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity: Option<f64>,
    /// How fast particles slow down, per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drag: Option<f64>,
    /// Degrees a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spin: Option<NumRange>,
    /// Seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub life: Option<NumRange>,
    /// Scale factors spread evenly over each particle's life, at most 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_over_life: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity_over_life: Option<Vec<f64>>,
    /// The shape every particle draws.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<Box<Node>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,

    // Backdrops.
    /// A colour laid over what is under the layer (with `blur` blurring it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
}
