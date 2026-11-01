//! The words' animation tool (`animateText`): words on screen come in letter by letter, word by word or line by line,
//! each from its own values (a rise, a size, a fade, a turn, a blur) over its own time, one after another or from the
//! middle out, and leave the same way if asked. The values come as `{key, value}` pairs (`TEXT_KEYS`), which the prompt
//! lists. The compiler makes it a `scene@1` motion graphic (`klipolo_model::media::text_motion`), checked like any
//! scene and held to what Klipolo draws in time.

use klipolo_model::media::text_motion::{
    From, LIMITS, Order, STAGGER_SECS, TextMotion, UNIT_SECS, Unit,
};

use super::error::Problem;
use super::motion::{EASE_FORMS, layer_field, read_ease};
use super::schema::Word;
use super::sets::{pairs, schema};
use super::{Tool, ToolCall};

impl Word for Unit {
    fn word(self) -> &'static str {
        match self {
            Unit::Letter => "letter",
            Unit::Word => "word",
            Unit::Line => "line",
        }
    }
}

const UNITS: [Unit; 3] = [Unit::Letter, Unit::Word, Unit::Line];
const ORDERS: [(&str, Order); 4] = [
    ("start", Order::Start),
    ("end", Order::End),
    ("center", Order::Center),
    ("random", Order::Random),
];

/// The keys `animateText` takes: where each unit starts from, then its time, the time between units, the ease, which
/// unit goes first and whether they leave the same way.
pub const TEXT_KEYS: [&str; 11] = [
    "by", "y", "scale", "opacity", "rotation", "blur", "sec", "stagger", "ease", "order", "exit",
];

pub(super) const ANIMATE_TEXT: Tool = Tool {
    name: "animateText",
    description: "Words come in by letter, word or line, each from the values set.",
    parameters: || vec![layer_field(), ("set", schema("Keys to set."))],
    read: |a| {
        let layer = a.layer_ref("layer")?;
        let set = pairs(a, "set", &TEXT_KEYS)?;
        let [ly, ls, lo, lr, lb] = LIMITS;
        let mut motion = TextMotion {
            unit: Unit::Word,
            from: From::default(),
            unit_secs: None,
            ease: None,
            stagger_secs: None,
            order: None,
            exit: false,
        };
        for p in &set {
            match p.key {
                "by" => {
                    let word = p.word(a, &UNITS.map(|u| u.word()))?;
                    motion.unit = UNITS
                        .into_iter()
                        .find(|u| u.word() == word)
                        .unwrap_or(Unit::Word);
                }
                "y" => motion.from.y = Some(p.number(a, ly)?),
                "scale" => motion.from.scale = Some(p.number(a, ls)?),
                "opacity" => motion.from.opacity = Some(p.number(a, lo)?),
                "rotation" => motion.from.rotation = Some(p.number(a, lr)?),
                "blur" => motion.from.blur = Some(p.number(a, lb)?),
                "sec" => {
                    motion.unit_secs = Some(p.number(a, (*UNIT_SECS.start(), *UNIT_SECS.end()))?)
                }
                "stagger" => {
                    motion.stagger_secs =
                        Some(p.number(a, (*STAGGER_SECS.start(), *STAGGER_SECS.end()))?)
                }
                "ease" => {
                    let text = p.text(a)?;
                    motion.ease = Some(read_ease(&text).ok_or_else(|| {
                        a.error(
                            &format!("{}.value", p.at),
                            Problem::NotForm {
                                expected: EASE_FORMS,
                                got: p.value.to_string(),
                            },
                        )
                    })?);
                }
                "order" => {
                    let word = p.word(a, &ORDERS.map(|(w, _)| w))?;
                    motion.order = ORDERS.iter().find(|(w, _)| *w == word).map(|(_, o)| *o);
                }
                _ => motion.exit = p.flag(a)?,
            }
        }
        if motion.from.is_rest() {
            return Err(a.error(
                "set",
                Problem::Unused {
                    why: "must set where the units start from: y, scale, opacity, rotation or blur",
                },
            ));
        }
        Ok(ToolCall::AnimateText { layer, motion })
    },
};
