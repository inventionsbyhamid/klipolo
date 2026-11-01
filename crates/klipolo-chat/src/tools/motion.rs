//! The motion tool: keys on a layer's position, scale, rotation and opacity, and its enter, exit and loop presets with
//! their settings, length and ease (`setMotion`), for any motion a person describes. `animate`'s words are the quick
//! shortcuts; this is what they are made of. The ease words (`EASE_FORMS`) are `animateText`'s too.
//!
//! Values are the model's own (`motion::LIMITS`): x and y move the layer by fractions of the frame (1 is a frame's width
//! right or height down), scale 1 is its size, rotation is degrees clockwise, opacity 0 to 1. A value outside them is
//! refused with the range, never clamped. Whether a key's time falls inside the layer, and a typewriter is on words, is
//! the compiler's to judge.

use klipolo_model::motion::{EASE_Y, Ease, LIMITS};
use klipolo_model::time::Us;
use serde_json::{Value, json};

use super::animate::LayerRef;
use super::args::{Args, kind};
use super::error::{Problem, ToolError};
use super::schema::{Word, list, object, seconds, string, words};
use super::{Tool, ToolCall};

/// A property keys move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Property {
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
}

impl Word for Property {
    fn word(self) -> &'static str {
        match self {
            Property::X => "x",
            Property::Y => "y",
            Property::Scale => "scale",
            Property::Rotation => "rotation",
            Property::Opacity => "opacity",
        }
    }
}

/// The properties, in `MotionKeys::each`'s order (`LIMITS`' too).
pub const PROPERTIES: [Property; 5] = [
    Property::X,
    Property::Y,
    Property::Scale,
    Property::Rotation,
    Property::Opacity,
];

impl Property {
    /// The values the model allows it.
    pub fn limits(self) -> (f64, f64) {
        LIMITS[self as usize]
    }
}

/// One key: at `at_us` from the layer's start the property is `value`, moving into it by `ease` (None: linear).
#[derive(Clone, Debug, PartialEq)]
pub struct KeyIn {
    pub property: Property,
    pub at_us: Us,
    pub value: f64,
    pub ease: Option<Ease>,
}

/// Which preset of a layer's motion: how it comes in, goes out, or moves while it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Enter,
    Exit,
    Loop,
}

/// The enter and exit presets by the tool's word, with the model's word (`contrib::builtin::MOTION_PRESETS`).
pub const ENTER_EXIT_PRESETS: [(&str, &str); 8] = [
    ("fade", "fade"),
    ("slide", "slide"),
    ("pop", "pop"),
    ("zoom", "zoom"),
    ("drop", "drop"),
    ("bounce", "bounce"),
    ("blur", "blur-in"),
    ("typewriter", "typewriter"),
];
/// The loop presets (`contrib::builtin::MOTION_LOOPS`).
pub const LOOP_PRESETS: [&str; 5] = ["float", "pulse", "shake", "wiggle", "spin"];
/// Where `slide` comes from or goes to.
pub const SIDES: [&str; 4] = ["left", "right", "top", "bottom"];
/// A loop's strength: the presets' own range.
pub const STRENGTH: std::ops::RangeInclusive<f64> = 0.0..=2.0;
/// How long a preset may take, in seconds (it is cut to the layer's length).
pub const PRESET_SECS: std::ops::RangeInclusive<f64> = 0.05..=60.0;

/// One preset `setMotion` sets: the model's motion preset word (`slide`, `blur-in`), or None to take the slot's away.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetIn {
    pub slot: Slot,
    pub preset: Option<&'static str>,
    pub duration_us: Option<Us>,
    pub ease: Option<Ease>,
    /// `slide`'s side.
    pub from: Option<&'static str>,
    /// A loop's strength.
    pub strength: Option<f64>,
}

/// The ease words, as an error says them.
pub const EASE_FORMS: &str = "linear, in, out, inOut, spring, spring with a bounce from 0 to 0.9 (spring 0.4), or \
                              cubic with four numbers (cubic 0.3 1.6 0.6 1)";
/// The most a spring may bounce: a damping ratio of 0.1, well inside what the model draws.
pub const MAX_BOUNCE: f64 = 0.9;
/// A spring's bounce when none is said.
pub const SPRING_BOUNCE: f64 = 0.3;

/// A spring of `bounce` (0 settles without passing its value, 0.9 swings a long time): its damping ratio is
/// `1 - bounce`, with stiffness 100 and mass 1 (the curve is stretched to the key's span, so only the ratio matters).
pub fn spring(bounce: f64) -> Ease {
    Ease::Spring {
        stiffness: 100.0,
        damping: 20.0 * (1.0 - bounce),
        mass: 1.0,
    }
}

/// The ease `text` says (`EASE_FORMS`), any case, spaces or commas between numbers; None when it says none of them.
pub fn read_ease(text: &str) -> Option<Ease> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let mut parts = lower
        .split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
        .filter(|p| !p.is_empty());
    let head = parts.next()?;
    let numbers: Option<Vec<f64>> = parts.map(|p| p.parse::<f64>().ok()).collect();
    let numbers = numbers.filter(|n| n.iter().all(|v| v.is_finite()))?;
    match (head, numbers.as_slice()) {
        ("linear", []) => Some(Ease::Linear),
        ("in", []) => Some(Ease::Power2In),
        ("out", []) => Some(Ease::Power2Out),
        ("inout", []) => Some(Ease::Power2InOut),
        ("spring", []) => Some(spring(SPRING_BOUNCE)),
        ("spring", [bounce]) if (0.0..=MAX_BOUNCE).contains(bounce) => Some(spring(*bounce)),
        ("cubic" | "cubic-bezier", [x1, y1, x2, y2]) => {
            let x = 0.0..=1.0;
            let y = EASE_Y.0..=EASE_Y.1;
            (x.contains(x1) && x.contains(x2) && y.contains(y1) && y.contains(y2)).then_some(
                Ease::CubicBezier {
                    x1: *x1,
                    y1: *y1,
                    x2: *x2,
                    y2: *y2,
                },
            )
        }
        _ => None,
    }
}

impl Args {
    /// An ease (`EASE_FORMS`), or None when it is null or left out.
    pub(super) fn nullable_ease(&mut self, key: &str) -> Result<Option<Ease>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(None);
        };
        let Value::String(text) = &value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "text or null",
                    got: kind(&value),
                },
            ));
        };
        read_ease(text).map(Some).ok_or_else(|| {
            self.error(
                key,
                Problem::NotForm {
                    expected: EASE_FORMS,
                    got: value.to_string(),
                },
            )
        })
    }

    /// A number within `bounds`, the field's own range.
    pub(super) fn number_within(
        &mut self,
        key: &str,
        (min, max): (f64, f64),
    ) -> Result<f64, ToolError> {
        let value = self.take(key)?;
        let n = self.number(key, &value, "a number")?;
        if (min..=max).contains(&n) {
            Ok(n)
        } else {
            Err(self.error(key, Problem::NotWithin { min, max, got: n }))
        }
    }

    /// The layer a tool names: `last`, or a ref (`t1`, `i1`, `v1`; not yet resolved).
    pub(super) fn layer_ref(&mut self, key: &str) -> Result<LayerRef, ToolError> {
        let layer = self.text(key)?;
        Ok(if layer.eq_ignore_ascii_case("last") {
            LayerRef::Last
        } else {
            LayerRef::One(layer)
        })
    }
}

fn key_in(a: &mut Args) -> Result<KeyIn, ToolError> {
    let property = a.word("property", &PROPERTIES)?;
    let at_us = a.time("atSec")?;
    let value = a.number_within("value", property.limits())?;
    let ease = a.nullable_ease("ease")?;
    Ok(KeyIn {
        property,
        at_us,
        value,
        ease,
    })
}

/// How `setMotion` writes a preset, as an error says it, for each slot.
const ENTER_EXIT_FORM: &str = "a preset (fade, slide, pop, zoom, drop, bounce, blur, typewriter), slide with a side \
                               (slide left), then a length and an ease if asked (pop 0.4s spring 0.5), or none";
const LOOP_FORM: &str = "a loop (float, pulse, shake, wiggle, spin), a strength from 0 to 2 if asked, then a cycle and \
                         an ease if asked (float 1.5 2s), or none";

/// The preset `text` writes for `slot`: its word, `slide`'s side or a loop's strength, a length in seconds (`0.5s`),
/// and an ease (`EASE_FORMS`), each after the one before; None for the slot's to go (`none`). The problem, when it
/// doesn't read, is `NotForm` with the slot's form.
fn read_preset(slot: Slot, text: &str) -> Result<PresetIn, &'static str> {
    let form = if slot == Slot::Loop {
        LOOP_FORM
    } else {
        ENTER_EXIT_FORM
    };
    let lower = text.trim().to_ascii_lowercase();
    let mut words = lower.split_whitespace().peekable();
    let head = words.next().ok_or(form)?;
    let mut preset = PresetIn {
        slot,
        preset: None,
        duration_us: None,
        ease: None,
        from: None,
        strength: None,
    };
    if head == "none" {
        return if words.next().is_none() {
            Ok(preset)
        } else {
            Err(form)
        };
    }
    preset.preset = Some(match slot {
        Slot::Loop => LOOP_PRESETS
            .iter()
            .copied()
            .find(|w| *w == head)
            .ok_or(form)?,
        _ => (ENTER_EXIT_PRESETS.iter())
            .find(|(word, _)| *word == head)
            .map(|(_, preset)| *preset)
            .ok_or(form)?,
    });
    if head == "slide"
        && let Some(side) = words.peek().and_then(|w| SIDES.iter().find(|s| *s == w))
    {
        preset.from = Some(side);
        words.next();
    }
    if slot == Slot::Loop
        && let Some(n) = words.peek().and_then(|w| w.parse::<f64>().ok())
    {
        if !STRENGTH.contains(&n) {
            return Err(form);
        }
        preset.strength = Some(n);
        words.next();
    }
    if let Some(secs) = (words.peek())
        .and_then(|w| w.strip_suffix('s'))
        .and_then(|n| n.parse::<f64>().ok())
    {
        if !PRESET_SECS.contains(&secs) {
            return Err(form);
        }
        preset.duration_us = Some(klipolo_model::time::secs_to_us(secs));
        words.next();
    }
    let rest: Vec<&str> = words.collect();
    if !rest.is_empty() {
        preset.ease = Some(read_ease(&rest.join(" ")).ok_or(form)?);
    }
    Ok(preset)
}

/// How `setMotion` writes its presets, as an error says it.
const PRESETS_FORM: &str = "presets each as a slot and a preset, separated by semicolons (enter slide left 0.5s \
                            spring 0.3; exit fade; loop float 1.5), each slot enter, exit or loop";

/// `setMotion`'s presets as written: `enter …; exit …; loop …`, each slot at most once, each read by `read_preset`
/// (a comma before a slot word separates too). The form a part breaks, when one does.
pub fn read_presets(text: &str) -> Result<Vec<PresetIn>, &'static str> {
    let lower = text.to_ascii_lowercase();
    let mut parts: Vec<String> = Vec::new();
    for part in lower.split(';') {
        let mut current = String::new();
        for (i, piece) in part.split(',').enumerate() {
            let starts_slot = ["enter", "exit", "loop"]
                .iter()
                .any(|s| piece.trim_start().starts_with(s));
            if i > 0 && !starts_slot {
                current.push(',');
            } else if i > 0 {
                parts.push(std::mem::take(&mut current));
            }
            current.push_str(piece);
        }
        parts.push(current);
    }
    let mut out: Vec<PresetIn> = Vec::new();
    for part in parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let (slot, rest) = part.split_once(char::is_whitespace).unwrap_or((part, ""));
        let slot = match slot {
            "enter" => Slot::Enter,
            "exit" => Slot::Exit,
            "loop" => Slot::Loop,
            _ => return Err(PRESETS_FORM),
        };
        if out.iter().any(|p| p.slot == slot) {
            return Err(PRESETS_FORM);
        }
        out.push(read_preset(slot, rest)?);
    }
    if out.is_empty() {
        return Err(PRESETS_FORM);
    }
    Ok(out)
}

impl Args {
    /// `setMotion`'s presets at `key` (`read_presets`), or none when it is null or left out.
    fn nullable_presets(&mut self, key: &str) -> Result<Vec<PresetIn>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(Vec::new());
        };
        let Value::String(text) = &value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "text or null",
                    got: kind(&value),
                },
            ));
        };
        read_presets(text).map_err(|expected| {
            self.error(
                key,
                Problem::NotForm {
                    expected,
                    got: value.to_string(),
                },
            )
        })
    }
}

/// A nullable list of objects, each read by `read`: None when it is null or left out.
fn nullable_each<T>(
    a: &mut Args,
    key: &str,
    read: fn(&mut Args) -> Result<T, ToolError>,
) -> Result<Option<Vec<T>>, ToolError> {
    if a.map.get(key).is_none_or(Value::is_null) {
        a.map.remove(key);
        return Ok(None);
    }
    a.each(key, read).map(Some)
}

fn nullable_list(items: Value, description: &str) -> Value {
    let mut schema = list(items, description);
    schema["type"] = json!(["array", "null"]);
    schema
}

/// The `layer` field the layer tools share.
pub(super) fn layer_field() -> (&'static str, Value) {
    ("layer", string("last, or a ref."))
}

pub(super) const SET_MOTION: Tool = Tool {
    name: "setMotion",
    description: "Keyframes, or enter, exit and loop presets, for a layer; a recording's clip (c1) keys its camera \
                  bubble (x, y, scale). Null keeps.",
    parameters: || {
        vec![
            layer_field(),
            (
                "keys",
                nullable_list(
                    object(vec![
                        (
                            "property",
                            words(&PROPERTIES, "x, y: frame shares; rotation: degrees."),
                        ),
                        ("atSec", seconds("From the layer's start.")),
                        (
                            "value",
                            json!({"type": "number", "description": "Its value."}),
                        ),
                        (
                            "ease",
                            super::schema::nullable_string(
                                "linear, in, out, inOut, spring, spring 0.4 (bounce to 0.9), cubic x1 y1 x2 y2.",
                            ),
                        ),
                    ]),
                    "A property listed gets just these keys; [] removes all.",
                ),
            ),
            (
                "presets",
                super::schema::nullable_string(
                    "Such as \"enter slide left 0.5s spring; exit fade; loop float 1.5 2s\".",
                ),
            ),
        ]
    },
    read: |a| {
        let layer = a.layer_ref("layer")?;
        let keys = nullable_each(a, "keys", key_in)?;
        let presets = a.nullable_presets("presets")?;
        if keys.is_none() && presets.is_empty() {
            return Err(ToolError {
                tool: a.tool.to_owned(),
                field: None,
                problem: Problem::NoChange {
                    fields: "keys or presets",
                },
            });
        }
        Ok(ToolCall::SetMotion {
            layer,
            keys,
            presets,
        })
    },
};
