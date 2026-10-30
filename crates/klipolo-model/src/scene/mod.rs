//! The `scene@1` motion graphics format. A motion graphic is data that Klipolo's own
//! code evaluates, never a script: nodes named as CSS and SVG name them, per-node keys and a GSAP-shaped timeline of
//! tweens, and params the person edits like any effect's settings.
//!
//! - **Reading.** Two entry points, both total (every input gives a scene or problems, never a panic): the render
//!   worker reads what an agent submits with [`Scene::from_submission`], which accepts `svg` nodes (the worker turns
//!   them into `path` nodes before it stores the scene); everything else reads a stored scene with
//!   [`Scene::from_stored`], only after the worker's verdict for that exact contribution, and it refuses `svg` nodes.
//! - **Checking.** [`check_scene`] checks a scene against its own params and the catalog, within [`SceneLimits`], and
//!   says what drawing it costs ([`SceneFacts`]). Every problem names a JSON path and a stable code, in English for the
//!   agent ([`SceneProblem`]).
//! - **Bindings.** A string that is `$` and a name (`"$title"`) is bound to the param with that key; inside a
//!   `repeat`, `"$i"` is the instance's index and `"$row.label"`, `"$row.v0"`…`"$row.v3"` the table row's. `$$` at the
//!   start writes a literal `$`; a `$` not followed by a name (`"$5"`) is text.
//! - **Units.** `box`, `x`, `y`, `origin` and `gap` are fractions of the layer's box (`"50%"` is 0.5), or 1080p
//!   pixels when written `"22px"` or keyed with `"unit": "px"`. Sizes, stroke widths, radii, blur, shadows and letter
//!   spacing are 1080p pixels.
//! - **Animating.** A node's `animate` keys and the timeline's tweens name the same properties ([`keys`]); a nested
//!   one is dotted (`"trim.end"`, `"shadow.blur"`). Both become the same keys when the scene is compiled.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::contrib::ContribId;
use crate::params::ParamSpec;

mod check;
pub mod eval;
pub mod format;
pub mod instance;
pub mod keys;
mod load;
pub mod modifiers;
pub mod path;
mod problems;
mod props;
pub mod segments;
mod text;
pub mod timeline;
mod tween;
pub mod units;
mod values;
mod walk;

pub use check::{check_scene, param_binding};
pub use props::Props;
pub use text::*;
pub use tween::*;
pub use values::*;

/// The `scene` value this build reads and writes.
pub const SCENE_VERSION: u32 = 1;
/// How long a tween runs when it doesn't say (`dur`), as in GSAP; a `set` takes no time.
pub const DEFAULT_TWEEN_DUR: f64 = 0.5;
/// How long a particle lives when its emitter doesn't say (`life`), in seconds.
pub const DEFAULT_PARTICLE_LIFE: f64 = 1.0;

/// The caps every scene is held to where it comes in.
pub struct SceneLimits;

impl SceneLimits {
    /// The JSON's size.
    pub const MAX_BYTES: usize = 256 * 1024;
    /// How deep the raw JSON nests, scanned before it is parsed.
    pub const MAX_JSON_DEPTH: usize = 32;
    /// How deep nodes nest: top-level nodes are at depth 1.
    pub const MAX_DEPTH: usize = 8;
    /// Nodes, counting children and emitters' particle shapes.
    pub const MAX_NODES: usize = 200;
    /// Nodes drawn once every `repeat` is expanded.
    pub const MAX_INSTANCES: usize = 1000;
    /// Particles alive at once, over every emitter.
    pub const MAX_PARTICLES: usize = 500;
    /// Commands in one path's `d` (an arc counts as the curves it becomes).
    pub const MAX_PATH_COMMANDS: usize = 4096;
    /// Groups drawn offscreen at the same moment.
    pub const MAX_GROUPS_OFFSCREEN: usize = 4;
    /// Keys per animated property.
    pub const MAX_KEYS: usize = 64;
    /// Tracks a frame reads at most, each instance, unit and chained read counted (the check holds scenes to it).
    pub const MAX_TRACKS: usize = 65_536;
    pub const MAX_TWEENS: usize = 256;
    /// Selectors in one tween's list.
    pub const MAX_SELECTORS: usize = 32;
    /// Characters a text node can show, and all of them in a scene.
    pub const MAX_TEXT_NODE: usize = 500;
    pub const MAX_TEXT_SCENE: usize = 2000;
    /// Split text units blurring at the same moment.
    pub const MAX_BLURRING_UNITS: usize = 12;
    /// Different gradients a scene draws (the renderer's table; identical ones count once).
    pub const MAX_GRADIENTS: usize = 32;
    /// A scene's length in seconds, from 0.1 to 600.
    pub const MIN_DURATION: f64 = 0.1;
    pub const MAX_DURATION: f64 = 600.0;
    /// The params a scene declares.
    pub const MAX_PARAMS: usize = 16;
    /// The problems one answer lists; the rest are counted in a last one.
    pub const MAX_PROBLEMS: usize = 64;
}

/// One motion graphic. JSON: `{"scene": 1, "duration": 4, "params": […], "nodes": […], "timeline": […]}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    /// Always 1 (`scene@1`); a larger number was made for a newer Klipolo.
    pub scene: u32,
    /// Seconds, 0.1–600.
    pub duration: f64,
    /// The entrance and exit: with them the hold between stretches to the layer's length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Segments>,
    /// At most 16: text, colour, number, choice, on/off, font, image and table settings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamSpec>,
    /// Drawn in order, later on top; at most 200 counting children.
    pub nodes: Vec<Node>,
    /// GSAP-shaped tweens, at most 256.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline: Vec<Tween>,
}

/// Scene times in `[0, in]` play from the layer's start and those in `[duration − out, duration]` up to its end.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Segments {
    #[serde(rename = "in")]
    pub enter: f64,
    pub out: f64,
}

/// One drawn thing (or a group of them), inside its `box`: `[x, y, w, h]`, fractions of its parent's box (the
/// layer's at the top); the whole box when left out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Node {
    #[serde(rename = "type")]
    pub kind: NodeType,
    /// Unique in the scene; tweens select it as `#id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Space-separated names; tweens select them as `.name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[Val; 4]>,
    #[serde(flatten)]
    pub props: Props,
    /// Keys per property name: `{"opacity": [{"t": 0, "v": 0}, {"t": 0.4, "v": 1, "ease": "power2.out"}]}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub animate: BTreeMap<String, Vec<SceneKey>>,
    /// A `group`'s or `repeat`'s nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum NodeType {
    /// A rectangle, `radius` rounding its corners.
    Rect,
    Ellipse,
    /// `sides` points, `innerRadius` deep.
    Star,
    /// A regular polygon of `sides` sides.
    Polygon,
    /// From the box's top left to its bottom right.
    Line,
    /// SVG path data (`d`).
    Path,
    /// `points`, from a table setting or written out, optionally `smooth`.
    Polyline,
    Text,
    /// An image setting (`asset`) or an emoji.
    Image,
    /// Submissions only: an SVG the worker turns into `path` nodes before storing the scene.
    Svg,
    Group,
    /// Its children `times` times, or once per row of a table setting (`rows`).
    Repeat,
    /// Particles: a seeded burst or rate of its `child`.
    Emitter,
    /// Blurs or tints what is under the layer, inside its box.
    Backdrop,
}

impl NodeType {
    pub const ALL: [NodeType; 14] = [
        Self::Rect,
        Self::Ellipse,
        Self::Star,
        Self::Polygon,
        Self::Line,
        Self::Path,
        Self::Polyline,
        Self::Text,
        Self::Image,
        Self::Svg,
        Self::Group,
        Self::Repeat,
        Self::Emitter,
        Self::Backdrop,
    ];

    /// The JSON name: `rect`, `polyline`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rect => "rect",
            Self::Ellipse => "ellipse",
            Self::Star => "star",
            Self::Polygon => "polygon",
            Self::Line => "line",
            Self::Path => "path",
            Self::Polyline => "polyline",
            Self::Text => "text",
            Self::Image => "image",
            Self::Svg => "svg",
            Self::Group => "group",
            Self::Repeat => "repeat",
            Self::Emitter => "emitter",
            Self::Backdrop => "backdrop",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// A number, or a string: a colour, a `"$binding"`, `"50%"`, `"22px"`, a clip, an origin or a text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Val {
    Num(f64),
    Str(String),
}

/// One key of an animated property: at `t` seconds the value is `v`, eased in from the key before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SceneKey {
    pub t: f64,
    pub v: Val,
    /// GSAP's or CSS's name (`power3.out`, `cubic-bezier(…)`, `steps(4)`, `spring(…)`, `hold`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<String>,
    /// For `x` and `y`: `px` reads `v` as 1080p pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    Px,
    Fraction,
}

/// Something wrong with a scene, for the agent: where (a JSON pointer, `/nodes/1/fill`), a stable code
/// (`scene.bindingKind`) and an English sentence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SceneProblem {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

/// What a checked scene costs to draw, for its budget class and the measurement.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneFacts {
    /// Nodes as written, counting children and particle shapes.
    pub nodes: usize,
    /// Nodes drawn once every `repeat` is expanded, at the largest its settings allow.
    pub max_instances: usize,
    /// Particles alive at once, at most.
    pub max_particles: usize,
    /// Groups drawn offscreen at the same moment, at most.
    pub groups_offscreen: usize,
    /// Path commands over every path and motion path.
    pub path_commands: usize,
    /// Characters the scene's text can show, at the longest its settings allow.
    pub text_chars: usize,
    /// Whether anything moves: keys, tweens, modifiers, particles or text animators.
    pub animates: bool,
    /// The generators drawn inside shapes, each once.
    pub generator_fills: Vec<ContribId>,
    /// The most animated values a frame reads (`MAX_TRACKS` at most): an upper bound on each instance's.
    pub track_reads: usize,
}

/// The scene's JSON Schema, generated from these types (served by `authoringGuide`).
pub fn json_schema() -> schemars::Schema {
    schemars::schema_for!(Scene)
}
