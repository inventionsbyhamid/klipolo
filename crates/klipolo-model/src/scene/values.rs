//! The shapes of a node's richer properties: paints, fonts, strokes, masks, text animators and particles.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Val;
use crate::contrib::ContribId;
use crate::font::FontRef;
use crate::params::ParamValue;

/// A colour (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgb()`, `rgba()`) or a `"$key"` colour setting; a gradient; or a
/// generator drawn inside the shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Paint {
    Color(String),
    Gradient(Gradient),
    Generator(GeneratorFill),
}

/// Exactly one of `linear` and `radial`, and 2–16 `stops` in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Gradient {
    /// From `[x0, y0]` to `[x1, y1]`: `[x0, y0, x1, y1]`, fractions of the node's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear: Option<[f64; 4]>,
    /// Centre and radius: `[cx, cy, r]`, fractions of the node's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radial: Option<[f64; 3]>,
    /// `[[offset 0–1, colour], …]`.
    pub stops: Vec<(f64, String)>,
    /// Degrees the gradient turns about the box's centre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<Val>,
    /// Slides every stop along the gradient (a shimmer animates it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Val>,
}

/// A generator contribution drawn inside the shape, with its settings (values or `"$key"` bindings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeneratorFill {
    pub generator: ContribId,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamValue>,
}

/// A text node's font: a `FontRef`'s JSON (`{"family": "Montserrat", "source": {"type": "google", "version": "v31"}}`)
/// with an optional `weight` and `italic` beside it, which win over the node's own; or a bare family name, the
/// shorthand, kept as written and resolved by the fonts' `resolve_family` when drawn; or `"$key"`, a font setting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SceneFont {
    Ref {
        #[serde(flatten)]
        font: FontRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weight: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        italic: Option<bool>,
    },
    Name(String),
}

/// Fractions of the stroke's length: it draws from `start` to `end`, both moved along by `offset`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Trim {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Val>,
}

/// One radius for every corner, or `[top left, top right, bottom right, bottom left]`, in 1080p pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Radius {
    All(Val),
    Corners([Val; 4]),
}

/// A drop shadow: offset, blur and spread in 1080p pixels, and a colour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Shadow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A group shows only where another node (`"#id"`) is opaque (`alpha`) or bright (`luma`), or the reverse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MaskRef {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<MaskMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MaskMode {
    Alpha,
    Luma,
}

/// Moves the node along a path as `progress` goes from 0 to 1, its origin on the path: SVG `d` fitted into the
/// parent's box as a `path` node's is (its `viewBox`, else the path's own bounds, `xMidYMid meet` unless `none`), or
/// `"#id"` of a `path` node, which then lends its `d`, `viewBox` and box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MotionPath {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Val>,
    /// Turns the node to face along the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_rotate: Option<bool>,
    /// `[x, y, width, height]` of the path's coordinates that fill the parent's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_box: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_aspect_ratio: Option<AspectRatio>,
}

/// How a path's `viewBox` fills its box, as SVG's `preserveAspectRatio`: whole and centred (`"xMidYMid meet"`, the
/// default), or stretched to it (`"none"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AspectRatio {
    #[default]
    #[serde(rename = "xMidYMid meet")]
    Meet,
    #[serde(rename = "none")]
    Stretch,
}

/// Which parts of a path are inside, as SVG's `fill-rule`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FillRule {
    #[default]
    #[serde(rename = "nonzero")]
    NonZero,
    #[serde(rename = "evenodd")]
    EvenOdd,
}

/// Movement added to a property, deterministic and seeded: `{"type": "wiggle", "freq": 2, "amp": 5}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Modifier {
    /// Smooth noise, `freq` times a second, up to `amp`.
    Wiggle {
        freq: f64,
        amp: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u32>,
    },
    /// A sine wave, fading by `decay` a second.
    Oscillate {
        freq: f64,
        amp: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decay: Option<f64>,
    },
    /// Keeps moving at `speed` a second.
    Marquee { speed: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

/// Where particles head: `angle` degrees (0 is right, −90 up), spread over `spread` degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cone {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<f64>,
}

/// One number, or a `[min, max]` each particle picks from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum NumRange {
    One(f64),
    Between([f64; 2]),
}

impl NumRange {
    /// The largest value it gives.
    pub fn max(self) -> f64 {
        match self {
            Self::One(v) => v,
            Self::Between([a, b]) => a.max(b),
        }
    }

    pub fn values(self) -> [f64; 2] {
        match self {
            Self::One(v) => [v, v],
            Self::Between(v) => v,
        }
    }
}

/// On or off, or `"$key"`, an on/off setting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Flag {
    Bool(bool),
    Bind(String),
}

/// A polyline's points: `"$key"`, a table setting (one point a row), or `[[x, y], …]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Points {
    Bind(String),
    List(Vec<[f64; 2]>),
}

/// `[x, y]` (fractions, `"50%"` or `"22px"`), or CSS words: `"50% 0%"`, `"center"`, `"left top"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Origin {
    Pair([Val; 2]),
    Css(String),
}

/// A string as it is shown: `$$…` is a literal `$…` (a binding is `$` and a name, and `$5` is text as written). The
/// checker reads text and font names through it, and so do the evaluator and the renderer.
pub fn literal(s: &str) -> &str {
    match s.strip_prefix('$') {
        Some(rest) if rest.starts_with('$') => rest,
        _ => s,
    }
}
