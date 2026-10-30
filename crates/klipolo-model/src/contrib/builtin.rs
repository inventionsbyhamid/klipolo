//! Everything that ships inside Klipolo: the built-in looks, motion presets, audio effects, effects, generators and
//! transitions, and their catalogue, with the shader presets read from their data folders (`crate::presets`).

use super::{Catalog, ContribDef, ContribId, ContribKind, DownsampleSpec};
use crate::params::{ParamKind, ParamSpec, ParamUnit, PointN, VisibleWhen};

/// (word, name): the words are format 2's and v1's stored values.
pub const LOOKS: [(&str, &str); 7] = [
    ("warm", "Warm"),
    ("cool", "Cool"),
    ("mono", "Mono"),
    ("vintage", "Vintage"),
    ("vivid", "Vivid"),
    ("cinema", "Cinema"),
    ("romcom", "Rom-com"),
];
pub const AUDIO_EFFECTS: [(&str, &str); 3] = [
    ("voice", "Voice"),
    ("warm", "Warm"),
    ("telephone", "Telephone"),
];
pub const EFFECT_BLUR: &str = "klipolo.builtin/effect.blur";
/// Shape mask: keeps a layer inside a circle, a rounded square, a square or its own rounded rectangle, with a
/// soft edge and an optional border inside it (round facecams, rounded screenshots).
pub const EFFECT_MASK: &str = "klipolo.builtin/effect.mask";
pub const EFFECT_PIXELATE: &str = "klipolo.builtin/effect.pixelate";
pub const EFFECT_VIGNETTE: &str = "klipolo.builtin/effect.vignette";
pub const GENERATOR_GRADIENT: &str = "klipolo.builtin/generator.gradient";
pub const TRANSITION_DISSOLVE: &str = "klipolo.builtin/transition.dissolve";
pub const TRANSITION_DIP: &str = "klipolo.builtin/transition.dip";
pub const TRANSITION_PUSH: &str = "klipolo.builtin/transition.push";
pub const TRANSITION_WIPE: &str = "klipolo.builtin/transition.wipe";
/// Directions for push and wipe: where the outgoing picture goes.
pub const DIRECTIONS: [&str; 4] = ["left", "right", "up", "down"];
/// The shape mask's shapes, in the order its shader numbers them (`mask.wgsl` tests the index; a test pins it).
/// `circle`, `rounded` (corners of `cornerRadius`) and `square` are as big as the layer's shorter side, so a 16:9
/// facecam becomes a true circle, and `center` moves them across the room they have inside the layer (0 to 1 on
/// each axis is edge to edge); `frame` is the whole layer with corners of `cornerRadius`.
pub const MASK_SHAPES: [&str; 4] = ["circle", "rounded", "square", "frame"];

pub fn look(word: &str) -> ContribId {
    ContribId::builtin(&format!("look.{word}"))
}

pub fn audio_effect(word: &str) -> ContribId {
    ContribId::builtin(&format!("audio.{word}"))
}

/// Enter and exit presets; `typewriter` is for text layers only, and `slide` takes `from`. `blur-in` is
/// written in kebab case: contribution ids have no capitals (`ContribId::is_well_formed`).
pub const MOTION_PRESETS: [&str; 8] = [
    "fade",
    "slide",
    "pop",
    "zoom",
    "drop",
    "bounce",
    "blur-in",
    "typewriter",
];
/// Loop presets; each takes `strength`.
pub const MOTION_LOOPS: [&str; 5] = ["float", "pulse", "shake", "wiggle", "spin"];

pub fn motion(word: &str) -> ContribId {
    ContribId::builtin(&format!("motion.{word}"))
}

fn spec(key: &str, label: &str, kind: ParamKind) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: label.into(),
        visible_when: None,
        kind,
    }
}

/// `spec`, applying only while the choice `key` is one of `values`.
fn when(spec: ParamSpec, key: &str, values: &[&str]) -> ParamSpec {
    ParamSpec {
        visible_when: Some(VisibleWhen {
            key: key.into(),
            values: values.iter().map(|v| (*v).to_string()).collect(),
        }),
        ..spec
    }
}

fn float(
    key: &str,
    label: &str,
    (min, max, default): (f64, f64, f64),
    unit: ParamUnit,
) -> ParamSpec {
    spec(
        key,
        label,
        ParamKind::Float {
            min,
            max,
            default,
            step: None,
            unit: Some(unit),
        },
    )
}

fn choice(key: &str, label: &str, options: &[&str], default: &str) -> ParamSpec {
    spec(
        key,
        label,
        ParamKind::Choice {
            options: options.iter().map(|o| (*o).to_string()).collect(),
            default: default.into(),
        },
    )
}

fn color(key: &str, label: &str, default: &str) -> ParamSpec {
    spec(
        key,
        label,
        ParamKind::Color {
            default: default.into(),
            alpha: true,
        },
    )
}

/// The built-ins that hide what they cover (`ContribDef::protective`).
fn protective(id: &ContribId) -> bool {
    matches!(id.as_str(), EFFECT_BLUR | EFFECT_PIXELATE | EFFECT_MASK)
}

pub(super) fn catalog() -> Catalog {
    let def = |id: ContribId, kind, name: &str, params| ContribDef {
        protective: protective(&id),
        params,
        ..ContribDef::new(id, kind, name)
    };
    let mut c = Catalog::default();
    for style in crate::captions::styles::catalog_defs() {
        c = c.with(style);
    }
    for (word, name) in LOOKS {
        c = c.with(def(look(word), ContribKind::Look, name, Vec::new()));
    }
    let from = || {
        choice(
            "from",
            "From",
            &["top", "bottom", "left", "right"],
            "bottom",
        )
    };
    // A factor, not a length or a share: no unit (1 is the preset's own size).
    let strength = || {
        spec(
            "strength",
            "Strength",
            ParamKind::Float {
                min: 0.0,
                max: 2.0,
                default: 1.0,
                step: Some(0.1),
                unit: None,
            },
        )
    };
    for (word, name, params) in [
        ("fade", "Fade", vec![]),
        ("slide", "Slide", vec![from()]),
        ("pop", "Pop", vec![]),
        ("zoom", "Zoom", vec![]),
        ("drop", "Drop", vec![]),
        ("bounce", "Bounce", vec![]),
        ("blur-in", "Blur", vec![]),
        ("typewriter", "Typewriter", vec![]),
        ("float", "Float", vec![strength()]),
        ("pulse", "Pulse", vec![strength()]),
        ("shake", "Shake", vec![strength()]),
        ("wiggle", "Wiggle", vec![strength()]),
        ("spin", "Spin", vec![strength()]),
    ] {
        c = c.with(def(motion(word), ContribKind::Motion, name, params));
    }
    for (word, name) in AUDIO_EFFECTS {
        c = c.with(def(
            audio_effect(word),
            ContribKind::AudioEffect,
            name,
            Vec::new(),
        ));
    }
    // The shipped presets: their definitions are data, read as a library version's are.
    for (local, json) in crate::presets::SHADER_PRESETS
        .into_iter()
        .chain(crate::presets::SCENE_PRESETS)
    {
        let preset: ContribDef = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("presets/{local}/contrib.json: {e}"));
        c = c.with(preset);
    }
    c.with(ContribDef {
        downsample: Some(DownsampleSpec {
            param: "radius".into(),
            at_least_px: 8.0,
        }),
        ..def(
            EFFECT_BLUR.into(),
            ContribKind::Effect,
            "Blur",
            vec![float("radius", "Radius", (0.0, 64.0, 12.0), ParamUnit::Px)],
        )
    })
    .with(def(
        EFFECT_PIXELATE.into(),
        ContribKind::Effect,
        "Pixelate",
        vec![float(
            "size",
            "Cell size",
            (2.0, 128.0, 24.0),
            ParamUnit::Px,
        )],
    ))
    .with(def(
        EFFECT_VIGNETTE.into(),
        ContribKind::Effect,
        "Vignette",
        vec![
            float("amount", "Amount", (0.0, 1.0, 0.5), ParamUnit::Percent),
            float("softness", "Softness", (0.0, 1.0, 0.5), ParamUnit::Percent),
        ],
    ))
    .with(def(
        EFFECT_MASK.into(),
        ContribKind::Effect,
        "Shape mask",
        vec![
            choice("shape", "Shape", &MASK_SHAPES, "circle"),
            // `frame` fills the layer, so it has no room to move in.
            when(
                spec(
                    "center",
                    "Centre",
                    ParamKind::Point {
                        default: PointN { x: 0.5, y: 0.5 },
                    },
                ),
                "shape",
                &["circle", "rounded", "square"],
            ),
            // A share of the layer's shorter side, for `rounded` and `frame`; 50 % rounds fully.
            when(
                float(
                    "cornerRadius",
                    "Corner radius",
                    (0.0, 0.5, 0.2),
                    ParamUnit::Percent,
                ),
                "shape",
                &["rounded", "frame"],
            ),
            float("feather", "Feather", (0.0, 8.0, 1.0), ParamUnit::Px),
            float(
                "borderWidth",
                "Border width",
                (0.0, 24.0, 0.0),
                ParamUnit::Px,
            ),
            color("borderColor", "Border colour", "#FFFFFF"),
        ],
    ))
    .with(def(
        GENERATOR_GRADIENT.into(),
        ContribKind::Generator,
        "Gradient",
        vec![
            color("startColor", "Start colour", "#1E64DC"),
            color("endColor", "End colour", "#101114"),
            spec("angle", "Angle", ParamKind::Angle { default: 90.0 }),
        ],
    ))
    .with(def(
        TRANSITION_DISSOLVE.into(),
        ContribKind::Transition,
        "Dissolve",
        Vec::new(),
    ))
    .with(def(
        TRANSITION_DIP.into(),
        ContribKind::Transition,
        "Dip to colour",
        vec![spec(
            "color",
            "Colour",
            ParamKind::Color {
                default: "#000000".into(),
                alpha: false,
            },
        )],
    ))
    .with(def(
        TRANSITION_PUSH.into(),
        ContribKind::Transition,
        "Push",
        vec![choice("direction", "Direction", &DIRECTIONS, "left")],
    ))
    .with(def(
        TRANSITION_WIPE.into(),
        ContribKind::Transition,
        "Wipe",
        vec![
            choice("direction", "Direction", &DIRECTIONS, "left"),
            float("softness", "Softness", (0.0, 1.0, 0.1), ParamUnit::Percent),
        ],
    ))
}
