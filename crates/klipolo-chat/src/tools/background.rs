//! `setBackground`: what shows behind the video where it doesn't fill the frame (a wide video fitted in a vertical
//! frame, or round the inset), and that inset. One entry of `TOOLS`; the words it takes for a background are here.
//! Every value may be left null to keep the project's own, so "add some padding" keeps the blur; at least one is set.

use std::ops::RangeInclusive;

use klipolo_model::project::BackgroundKind;
use serde_json::{Value, json};

use super::args::{Args, kind};
use super::error::{Problem, ToolError};
use super::schema::{Word, nullable_number, nullable_words};
use super::{Tool, ToolCall};

/// The insets the background takes round the video, in percent of the frame's shorter side on every edge (the model's
/// padding is the share: 0 to 0.4).
pub const PADDINGS: RangeInclusive<f64> = 0.0..=40.0;

/// A background chat may set. A picture behind the video stays the background card's: chat names no picture here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundChoice {
    /// Black.
    None,
    /// The video itself, blurred and dimmed behind it.
    Blur,
    /// One colour.
    Color,
    /// Two colours across the frame.
    Gradient,
}

impl BackgroundChoice {
    /// The model's kind it sets.
    pub fn kind(self) -> BackgroundKind {
        match self {
            BackgroundChoice::None => BackgroundKind::None,
            BackgroundChoice::Blur => BackgroundKind::Blur,
            BackgroundChoice::Color => BackgroundKind::Solid,
            BackgroundChoice::Gradient => BackgroundKind::Gradient,
        }
    }

    /// The tool's word for it.
    pub fn key(self) -> &'static str {
        match self {
            BackgroundChoice::None => "none",
            BackgroundChoice::Blur => "blur",
            BackgroundChoice::Color => "color",
            BackgroundChoice::Gradient => "gradient",
        }
    }
}

impl Word for BackgroundChoice {
    fn word(self) -> &'static str {
        self.key()
    }
}

/// The backgrounds `setBackground` offers, in the schema's order.
pub const BACKGROUNDS: [BackgroundChoice; 4] = [
    BackgroundChoice::Blur,
    BackgroundChoice::Color,
    BackgroundChoice::Gradient,
    BackgroundChoice::None,
];

/// A hex colour, or null.
fn nullable_colour(description: &str) -> Value {
    json!({"type": ["string", "null"], "description": description})
}

/// A colour as `#RRGGBB`, or None when it is null or left out. Whether it is a colour the project takes is the
/// model's rule, judged when the edit is tried.
fn colour(a: &mut Args, key: &str) -> Result<Option<String>, ToolError> {
    match a.take_nullable(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.trim().to_owned())),
        Some(other) => Err(a.error(
            key,
            Problem::WrongType {
                expected: "a hex colour like #1A2B3C or null",
                got: kind(&other),
            },
        )),
    }
}

pub(super) const SET_BACKGROUND: Tool = Tool {
    name: "setBackground",
    description: "What shows behind the video where it doesn't fill the frame, and an inset round it. Clips that \
                  fill the frame are set to fit so it shows: no setClip needed. blur is the usual look for vertical \
                  shorts. Null keeps a value.",
    parameters: || {
        vec![
            (
                "kind",
                nullable_words(
                    &BACKGROUNDS,
                    "blur: the video itself, blurred. none: black.",
                ),
            ),
            (
                "color",
                nullable_colour("Hex like #1A2B3C; a gradient's first colour."),
            ),
            (
                "colorEnd",
                nullable_colour("Hex: a gradient's second colour."),
            ),
            (
                "padding",
                nullable_number(&format!(
                    "Inset round the video, {} to {} percent of the frame's shorter side.",
                    PADDINGS.start(),
                    PADDINGS.end()
                )),
            ),
        ]
    },
    read: |a| {
        let kind = a.nullable_word("kind", &BACKGROUNDS)?;
        let color = colour(a, "color")?;
        let color_end = colour(a, "colorEnd")?;
        let padding = (a.nullable_number("padding", &PADDINGS)?).map(|percent| percent / 100.0);
        if kind.is_none() && color.is_none() && color_end.is_none() && padding.is_none() {
            return Err(ToolError {
                tool: a.tool.to_owned(),
                field: None,
                problem: Problem::NoChange {
                    fields: "kind, color, colorEnd or padding",
                },
            });
        }
        Ok(ToolCall::SetBackground {
            kind,
            color,
            color_end,
            padding,
        })
    },
};

/// The context's line for `background` (`Background: blur, padding 10%`), in the tool's words; None for the default
/// (black to the edges), so a project that never set one reads as before.
pub(crate) fn context_line(background: &klipolo_model::project::Background) -> Option<String> {
    let b = background;
    let kind = match b.kind {
        BackgroundKind::None => "none".to_owned(),
        BackgroundKind::Blur => "blur".to_owned(),
        BackgroundKind::Solid => format!("color {}", b.color),
        BackgroundKind::Gradient => format!(
            "gradient {} to {}",
            b.color,
            b.color_end.as_deref().unwrap_or(&b.color)
        ),
        BackgroundKind::Image => "a picture".to_owned(),
    };
    match (b.kind, b.padding > 0.0) {
        (BackgroundKind::None, false) => None,
        (_, false) => Some(format!("Background: {kind}")),
        (_, true) => Some(format!(
            "Background: {kind}, padding {:.0}%",
            b.padding * 100.0
        )),
    }
}
