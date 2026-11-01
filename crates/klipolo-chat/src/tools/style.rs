//! The words' style, as `setLayer`'s keys (`STYLE_KEYS`): the text card's fields, by the card's words: a font from the
//! app's fonts, a size, the colour, a highlight behind the words, an outline and its width, the shadow, the alignment,
//! bold and italic, and the card's one-click looks (`LOOKS`, as `apps/desktop/src/lib/textLook.ts` has them). A look
//! comes first and the other keys go on top of it. Colours are `#RRGGBB` or `#RRGGBBAA`; whether a font has that name,
//! and the layer holds words, is the compiler's to judge.

use klipolo_model::timeline::TextAlign;

use super::args::Args;
use super::error::{Problem, ToolError};
use super::schema::Word;
use super::sets::Pair;

/// An outline's width, as a share of the text size: the card's three steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineStep {
    Thin,
    Medium,
    Thick,
}

impl OutlineStep {
    /// Its width at a text `size` (`textLook.ts`'s `OUTLINE_EM`).
    pub fn width(self, size: f64) -> f64 {
        let em = match self {
            OutlineStep::Thin => 0.04,
            OutlineStep::Medium => 0.07,
            OutlineStep::Thick => 0.1,
        };
        (size * em * 100.0).round() / 100.0
    }
}

impl Word for OutlineStep {
    fn word(self) -> &'static str {
        match self {
            OutlineStep::Thin => "thin",
            OutlineStep::Medium => "medium",
            OutlineStep::Thick => "thick",
        }
    }
}

const STEPS: [OutlineStep; 3] = [OutlineStep::Thin, OutlineStep::Medium, OutlineStep::Thick];

impl Word for TextAlign {
    fn word(self) -> &'static str {
        match self {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        }
    }
}

const ALIGNS: [TextAlign; 3] = [TextAlign::Left, TextAlign::Center, TextAlign::Right];

/// One of the card's looks: its colour, the box behind the words (None: no box), its outline's colour and step, and
/// whether it has a shadow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub key: &'static str,
    pub color: &'static str,
    pub highlight: Option<&'static str>,
    pub outline: Option<(&'static str, OutlineStep)>,
    pub shadow: bool,
}

impl Word for Look {
    fn word(self) -> &'static str {
        self.key
    }
}

const WHITE: &str = "#FFFFFF";
const BLACK: &str = "#000000";
const YELLOW: &str = "#FFD02E";
/// The box behind the words: the design's navy at 86 %.
const BOX_COLOR: &str = "#121A33DB";

/// The card's looks, in its order.
pub const LOOKS: [Look; 5] = [
    Look {
        key: "clean",
        color: WHITE,
        highlight: None,
        outline: None,
        shadow: true,
    },
    Look {
        key: "boldOutline",
        color: WHITE,
        highlight: None,
        outline: Some((BLACK, OutlineStep::Thick)),
        shadow: false,
    },
    Look {
        key: "highlight",
        color: BLACK,
        highlight: Some(YELLOW),
        outline: None,
        shadow: false,
    },
    Look {
        key: "box",
        color: WHITE,
        highlight: Some(BOX_COLOR),
        outline: None,
        shadow: false,
    },
    Look {
        key: "pop",
        color: YELLOW,
        highlight: None,
        outline: Some((BLACK, OutlineStep::Medium)),
        shadow: false,
    },
];

/// A colour, or `none` to take it away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paint {
    Color(String),
    None,
}

/// The text sizes the model takes, in pixels at a 1080-pixel short side.
pub const SIZES: std::ops::RangeInclusive<f64> = 4.0..=400.0;

/// What `styleText` changes; None keeps each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleChange {
    pub look: Option<Look>,
    pub font: Option<String>,
    pub size: Option<f64>,
    pub color: Option<String>,
    pub highlight: Option<Paint>,
    pub outline: Option<Paint>,
    pub outline_width: Option<OutlineStep>,
    pub shadow: Option<bool>,
    pub align: Option<TextAlign>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
}

/// Whether `text` is `#RRGGBB` or `#RRGGBBAA`.
pub fn is_hex(text: &str) -> bool {
    let hex = text.strip_prefix('#').unwrap_or_default();
    matches!(hex.len(), 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The keys `styleText` takes.
pub const STYLE_KEYS: [&str; 11] = [
    "look",
    "font",
    "size",
    "color",
    "highlight",
    "outline",
    "outlineWidth",
    "shadow",
    "align",
    "bold",
    "italic",
];

/// A colour `#RRGGBB` or `#RRGGBBAA` (upper case), or `none` where `none` may take it away.
fn paint(p: &Pair, a: &Args, none: bool) -> Result<Paint, ToolError> {
    let text = p.value.as_str().map(str::trim).unwrap_or_default();
    if none && text.eq_ignore_ascii_case("none") {
        return Ok(Paint::None);
    }
    if is_hex(text) {
        return Ok(Paint::Color(text.to_ascii_uppercase()));
    }
    Err(a.error(
        &format!("{}.value", p.at),
        Problem::NotForm {
            expected: if none {
                "a hex colour like #FFD02E, or none"
            } else {
                "a hex colour like #FFD02E"
            },
            got: p.value.to_string(),
        },
    ))
}

fn word_of<T: Word>(p: &Pair, a: &Args, all: &[T]) -> Result<T, ToolError> {
    let words: Vec<&'static str> = all.iter().map(|w| w.word()).collect();
    let said = p.word(a, &words)?;
    Ok(all
        .iter()
        .copied()
        .find(|w| w.word() == said)
        .unwrap_or(all[0]))
}

/// One `setLayer` pair of the words' style (a key of `STYLE_KEYS`) into `change`.
pub(super) fn read_style(p: &Pair, a: &Args, change: &mut StyleChange) -> Result<(), ToolError> {
    match p.key {
        "look" => change.look = Some(word_of(p, a, &LOOKS)?),
        "font" => change.font = Some(p.text(a)?),
        "size" => change.size = Some(p.number(a, (*SIZES.start(), *SIZES.end()))?),
        "color" => {
            if let Paint::Color(c) = paint(p, a, false)? {
                change.color = Some(c);
            }
        }
        "highlight" => change.highlight = Some(paint(p, a, true)?),
        "outline" => change.outline = Some(paint(p, a, true)?),
        "outlineWidth" => change.outline_width = Some(word_of(p, a, &STEPS)?),
        "shadow" => change.shadow = Some(p.flag(a)?),
        "align" => change.align = Some(word_of(p, a, &ALIGNS)?),
        "bold" => change.bold = Some(p.flag(a)?),
        _ => change.italic = Some(p.flag(a)?),
    }
    Ok(())
}
