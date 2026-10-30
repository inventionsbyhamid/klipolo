//! Which keys each node type takes, which properties animate and what their values are, and a close name for a key
//! that isn't one. serde's `flatten` can't deny unknown keys, so the loader walks the raw JSON against these tables.

use super::NodeType;

mod hints;
pub use hints::{expects, suggest, why_not};

/// What every node takes.
pub const COMMON: &[&str] = &[
    "type",
    "id",
    "class",
    "box",
    "animate",
    "x",
    "y",
    "origin",
    "scale",
    "scaleX",
    "scaleY",
    "rotate",
    "opacity",
    "visible",
    "blur",
    "blend",
    "clip",
    "motionPath",
    "modifiers",
];

/// What a node of this type takes beside [`COMMON`].
pub fn own(kind: NodeType) -> &'static [&'static str] {
    match kind {
        NodeType::Rect => &[
            "fill",
            "shadow",
            "radius",
            "stroke",
            "strokeWidth",
            "cap",
            "join",
            "dash",
            "dashOffset",
            "trim",
        ],
        NodeType::Ellipse => &[
            "fill",
            "shadow",
            "stroke",
            "strokeWidth",
            "cap",
            "join",
            "dash",
            "dashOffset",
            "trim",
        ],
        NodeType::Path => &[
            "fill",
            "shadow",
            "d",
            "viewBox",
            "preserveAspectRatio",
            "strokeScale",
            "fillRule",
            "stroke",
            "strokeWidth",
            "cap",
            "join",
            "dash",
            "dashOffset",
            "trim",
        ],
        NodeType::Star => &[
            "fill",
            "shadow",
            "sides",
            "innerRadius",
            "stroke",
            "strokeWidth",
            "cap",
            "join",
            "dash",
            "dashOffset",
            "trim",
        ],
        NodeType::Polygon => &[
            "fill",
            "shadow",
            "sides",
            "stroke",
            "strokeWidth",
            "cap",
            "join",
            "dash",
            "dashOffset",
            "trim",
        ],
        NodeType::Line => &[
            "shadow",
            "stroke",
            "strokeWidth",
            "cap",
            "dash",
            "dashOffset",
            "trim",
        ],
        NodeType::Polyline => &[
            "shadow",
            "points",
            "smooth",
            "stroke",
            "strokeWidth",
            "cap",
            "join",
            "dash",
            "dashOffset",
            "trim",
        ],
        NodeType::Text => &[
            "fill",
            "shadow",
            "text",
            "spans",
            "font",
            "size",
            "weight",
            "italic",
            "align",
            "wrap",
            "lineHeight",
            "letterSpacing",
            "case",
            "outline",
            "split",
            "stagger",
            "count",
            "reveal",
            "scramble",
            "holds",
            "fit",
        ],
        NodeType::Image => &["asset", "emoji", "file", "radius", "shadow"],
        NodeType::Svg => &["src"],
        NodeType::Group => &["children", "rotateX", "rotateY", "perspective", "mask"],
        NodeType::Repeat => &["children", "times", "rows", "gap"],
        NodeType::Emitter => &[
            "burst",
            "rate",
            "at",
            "cone",
            "speed",
            "gravity",
            "drag",
            "spin",
            "life",
            "sizeOverLife",
            "opacityOverLife",
            "child",
            "seed",
        ],
        NodeType::Backdrop => &["tint", "radius"],
    }
}

/// Whether a node of this type takes `key`.
pub fn accepts(kind: NodeType, key: &str) -> bool {
    COMMON.contains(&key) || own(kind).contains(&key)
}

/// What an animated property's values are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// A fraction of the box, `"50%"`, `"22px"`, or a number binding.
    Position,
    /// A number or a number binding; `"50%"` reads as 0.5.
    Number,
    Color,
    Text,
    /// `inset(…)` or `circle(…)`.
    Clip,
    /// `"50% 0%"` and the like.
    Origin,
    /// 0 or 1, or an on/off binding.
    Flag,
}

/// Every property that animates, the key a node must take for it, and its values. Nested ones are dotted.
pub const ANIMATABLE: &[(&str, &str, ValueKind)] = &[
    ("x", "x", ValueKind::Position),
    ("y", "y", ValueKind::Position),
    ("origin", "origin", ValueKind::Origin),
    ("scale", "scale", ValueKind::Number),
    ("scaleX", "scaleX", ValueKind::Number),
    ("scaleY", "scaleY", ValueKind::Number),
    ("rotate", "rotate", ValueKind::Number),
    ("rotateX", "rotateX", ValueKind::Number),
    ("rotateY", "rotateY", ValueKind::Number),
    ("perspective", "perspective", ValueKind::Number),
    ("opacity", "opacity", ValueKind::Number),
    ("visible", "visible", ValueKind::Flag),
    ("blur", "blur", ValueKind::Number),
    ("clip", "clip", ValueKind::Clip),
    ("fill", "fill", ValueKind::Color),
    ("fill.offset", "fill", ValueKind::Number),
    ("fill.angle", "fill", ValueKind::Number),
    ("stroke", "stroke", ValueKind::Color),
    ("strokeWidth", "strokeWidth", ValueKind::Number),
    ("dashOffset", "dashOffset", ValueKind::Number),
    ("trim.start", "trim", ValueKind::Number),
    ("trim.end", "trim", ValueKind::Number),
    ("trim.offset", "trim", ValueKind::Number),
    ("radius", "radius", ValueKind::Number),
    ("innerRadius", "innerRadius", ValueKind::Number),
    ("shadow.x", "shadow", ValueKind::Number),
    ("shadow.y", "shadow", ValueKind::Number),
    ("shadow.blur", "shadow", ValueKind::Number),
    ("shadow.spread", "shadow", ValueKind::Number),
    ("shadow.color", "shadow", ValueKind::Color),
    ("motionPath.progress", "motionPath", ValueKind::Number),
    ("size", "size", ValueKind::Number),
    ("letterSpacing", "letterSpacing", ValueKind::Number),
    ("lineHeight", "lineHeight", ValueKind::Number),
    ("text", "text", ValueKind::Text),
    ("tint", "tint", ValueKind::Color),
];

/// The values of the property `name` and the key a node must take for it, if it animates at all.
pub fn animatable(name: &str) -> Option<(&'static str, ValueKind)> {
    ANIMATABLE
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, base, kind)| (*base, *kind))
}

/// The values of `name` on a node of this type, if it animates there.
pub fn animatable_on(kind: NodeType, name: &str) -> Option<ValueKind> {
    let (base, values) = animatable(name)?;
    accepts(kind, base).then_some(values)
}

/// The names that animate on a node of this type.
pub fn animatable_names(kind: NodeType) -> impl Iterator<Item = &'static str> {
    ANIMATABLE
        .iter()
        .filter(move |(_, base, _)| accepts(kind, base))
        .map(|(name, _, _)| *name)
}

/// The keys of the objects inside a scene, by where they are.
pub mod nested;
