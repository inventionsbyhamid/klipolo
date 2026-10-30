//! The keys of the objects inside a scene (its top, segments, tweens, keys, strokes, paints, fonts, text animators,
//! modifiers and params), by where they are.

pub const TOP: &[&str] = &[
    "scene", "duration", "segments", "params", "nodes", "timeline",
];
pub const SEGMENTS: &[&str] = &["in", "out"];
pub const TWEEN: &[&str] = &[
    "to",
    "from",
    "fromTo",
    "set",
    "start",
    "at",
    "dur",
    "ease",
    "stagger",
    "repeat",
    "yoyo",
    "repeatDelay",
    "label",
];
pub const STAGGER: &[&str] = &["each", "amount", "from", "seed"];
pub const KEY: &[&str] = &["t", "v", "ease", "unit"];
pub const TRIM: &[&str] = &["start", "end", "offset"];
pub const SHADOW: &[&str] = &["x", "y", "blur", "spread", "color"];
pub const MASK: &[&str] = &["node", "mode", "invert"];
pub const MOTION_PATH: &[&str] = &[
    "path",
    "progress",
    "autoRotate",
    "viewBox",
    "preserveAspectRatio",
];
pub const GRADIENT: &[&str] = &["linear", "radial", "stops", "angle", "offset"];
pub const GENERATOR: &[&str] = &["generator", "params"];
pub const FONT: &[&str] = &["family", "source", "weight", "italic"];
pub const FONT_SOURCE: &[&str] = &["type", "version"];
pub const SPAN: &[&str] = &["text", "fill", "size", "weight", "italic", "background"];
pub const SPAN_BACKGROUND: &[&str] = &["color", "padding", "radius"];
pub const OUTLINE: &[&str] = &["width", "color"];
pub const COUNT: &[&str] = &[
    "from",
    "to",
    "decimals",
    "separator",
    "prefix",
    "suffix",
    "at",
    "dur",
    "ease",
];
pub const REVEAL: &[&str] = &["by", "at", "dur", "ease"];
pub const SCRAMBLE: &[&str] = &["chars", "seed", "at", "dur"];
pub const HOLD: &[&str] = &["t", "text"];
pub const CONE: &[&str] = &["angle", "spread"];
pub const VISIBLE_WHEN: &[&str] = &["key", "in"];

/// A modifier's keys by its `type`.
pub fn modifier(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "wiggle" => Some(&["type", "freq", "amp", "seed"]),
        "oscillate" => Some(&["type", "freq", "amp", "phase", "decay"]),
        "marquee" => Some(&["type", "speed"]),
        _ => None,
    }
}

/// A param's keys by its `type`.
pub fn param(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "float" => &[
            "key",
            "label",
            "visibleWhen",
            "type",
            "min",
            "max",
            "default",
            "step",
            "unit",
        ],
        "int" => &[
            "key",
            "label",
            "visibleWhen",
            "type",
            "min",
            "max",
            "default",
        ],
        "bool" | "point" | "angle" | "font" => &["key", "label", "visibleWhen", "type", "default"],
        "color" => &["key", "label", "visibleWhen", "type", "default", "alpha"],
        "choice" => &["key", "label", "visibleWhen", "type", "options", "default"],
        "text" => &["key", "label", "visibleWhen", "type", "default", "maxLen"],
        "asset" => &["key", "label", "visibleWhen", "type", "kind"],
        "table" => &[
            "key",
            "label",
            "visibleWhen",
            "type",
            "columns",
            "maxRows",
            "default",
        ],
        _ => return None,
    })
}
