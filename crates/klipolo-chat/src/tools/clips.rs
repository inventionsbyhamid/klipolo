//! The clip tools: a clip's speed, volume, mute and Fit or Fill (`setClip`, the clip card's fields), and the transition
//! at a cut (`setTransition`, Add transition and the transition card). Each is one entry of `TOOLS`; the words they
//! take are here.
//!
//! They take exactly what the cards offer: a speed from `clip_settings::SPEEDS`, a volume from 0 to 2 in hundredths, a
//! transition from the built-in four, from 0.1 to 5 s long (`transitions::MIN_TRANSITION_US`, `MAX_TRANSITION_US`).
//! Anything else is refused, never clamped. Whether a ref names a clip, a clip has sound, or a cut has room for a length
//! is the compiler's to judge against the project.

use std::ops::RangeInclusive;

use klipolo_model::clip_settings::{SPEEDS, VOLUME_MAX, VOLUME_MIN, VOLUME_STEP, is_volume};
use klipolo_model::contrib::builtin;
use klipolo_model::time::US_PER_SEC;
use klipolo_model::timeline::FramingMode;
use klipolo_model::transitions::{MAX_TRANSITION_US, MIN_TRANSITION_US};

use super::args::Args;
use super::error::{Problem, ToolError};
use super::schema::{Word, nullable_boolean, nullable_number, nullable_words, string, words};
use super::{FORMATS, Tool, ToolCall};

/// What a clip tool names: one clip by its ref from the context (`c2`, not yet resolved), or every clip (`all`). For
/// `setTransition`, the clip before the cut, or every cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipRef {
    One(String),
    All,
}

impl ClipRef {
    /// The ref as the model wrote it: `c2`, or `all`.
    pub fn word(&self) -> &str {
        match self {
            ClipRef::One(r) => r,
            ClipRef::All => "all",
        }
    }
}

/// A built-in transition `setTransition` offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionKind {
    /// A crossfade.
    Dissolve,
    /// Through black (its colour's default) and back.
    Dip,
    Push,
    Wipe,
}

impl TransitionKind {
    /// The tool's word for it.
    pub fn key(self) -> &'static str {
        match self {
            TransitionKind::Dissolve => "dissolve",
            TransitionKind::Dip => "dip",
            TransitionKind::Push => "push",
            TransitionKind::Wipe => "wipe",
        }
    }

    /// The built-in transition it is.
    pub fn effect(self) -> &'static str {
        match self {
            TransitionKind::Dissolve => builtin::TRANSITION_DISSOLVE,
            TransitionKind::Dip => builtin::TRANSITION_DIP,
            TransitionKind::Push => builtin::TRANSITION_PUSH,
            TransitionKind::Wipe => builtin::TRANSITION_WIPE,
        }
    }
}

/// `setTransition`'s `kind`: a transition, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Is(TransitionKind),
    None,
}

impl Word for Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Is(kind) => kind.key(),
            Kind::None => "none",
        }
    }
}

impl Word for FramingMode {
    fn word(self) -> &'static str {
        match self {
            FramingMode::Fit => "fit",
            FramingMode::Fill => "fill",
        }
    }
}

const KINDS: [Kind; 5] = [
    Kind::Is(TransitionKind::Dissolve),
    Kind::Is(TransitionKind::Dip),
    Kind::Is(TransitionKind::Push),
    Kind::Is(TransitionKind::Wipe),
    Kind::None,
];
const FRAMINGS: [FramingMode; 2] = [FramingMode::Fit, FramingMode::Fill];
/// `SPEEDS` as the model writes them, for the refusal that lists them (a test holds them to `SPEEDS`).
pub const SPEED_WORDS: [&str; 6] = ["0.5", "0.75", "1", "1.25", "1.5", "2"];
/// The lengths a transition takes, in seconds.
pub const TRANSITION_SECS: RangeInclusive<f64> =
    MIN_TRANSITION_US as f64 / US_PER_SEC as f64..=MAX_TRANSITION_US as f64 / US_PER_SEC as f64;

impl Args {
    /// A clip ref, or `all` (any case).
    fn clip_ref(&mut self, key: &str) -> Result<ClipRef, ToolError> {
        let text = self.text(key)?;
        Ok(if text.eq_ignore_ascii_case("all") {
            ClipRef::All
        } else {
            ClipRef::One(text)
        })
    }

    /// One of `SPEEDS`, or None when it is null or left out.
    fn nullable_speed(&mut self, key: &str) -> Result<Option<f64>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(None);
        };
        let speed = self.number(key, &value, "a number or null")?;
        if SPEEDS.contains(&speed) {
            return Ok(Some(speed));
        }
        Err(self.error(
            key,
            Problem::NotOneOf {
                allowed: SPEED_WORDS.to_vec(),
                nullable: true,
                got: value.to_string(),
            },
        ))
    }

    /// A volume the card's slider can set, or None when it is null or left out.
    fn nullable_volume(&mut self, key: &str) -> Result<Option<f64>, ToolError> {
        let Some(volume) = self.nullable_number(key, &(VOLUME_MIN..=VOLUME_MAX))? else {
            return Ok(None);
        };
        if !is_volume(volume) {
            return Err(self.error(
                key,
                Problem::OffStep {
                    step: VOLUME_STEP,
                    got: volume,
                },
            ));
        }
        Ok(Some(volume))
    }
}

pub(super) const SET_CLIP: Tool = Tool {
    name: "setClip",
    description: "Set a clip's speed, volume, mute, or Fit or Fill. Null keeps a value.",
    parameters: || {
        vec![
            ("clip", string("A clip's ref, such as c2, or all.")),
            (
                "speed",
                nullable_number("0.5, 0.75, 1, 1.25, 1.5 or 2 (2 is twice as fast)."),
            ),
            (
                "volume",
                nullable_number("0 to 2 in steps of 0.01; 1 is as recorded."),
            ),
            (
                "muted",
                nullable_boolean("True mutes it, false unmutes it."),
            ),
            (
                "framing",
                nullable_words(
                    &FRAMINGS,
                    "fit shows the whole picture; fill fills the frame, cropping.",
                ),
            ),
            (
                "framingFormat",
                nullable_words(
                    &FORMATS,
                    "With framing: the format it is for. Null for every format.",
                ),
            ),
        ]
    },
    read: |a| {
        let clip = a.clip_ref("clip")?;
        let speed = a.nullable_speed("speed")?;
        let volume = a.nullable_volume("volume")?;
        let muted = a.nullable_flag("muted")?;
        let framing = a.nullable_word("framing", &FRAMINGS)?;
        // Read with framing only: with none, no format's framing changes.
        let format = a.nullable_word("framingFormat", &FORMATS)?;
        if speed.is_none() && volume.is_none() && muted.is_none() && framing.is_none() {
            return Err(ToolError {
                tool: a.tool.to_owned(),
                field: None,
                problem: Problem::NoChange {
                    fields: "speed, volume, muted or framing",
                },
            });
        }
        Ok(ToolCall::SetClip {
            clip,
            speed,
            volume,
            muted,
            framing: framing.map(|mode| (mode, format)),
        })
    },
};

pub(super) const SET_TRANSITION: Tool = Tool {
    name: "setTransition",
    description: "Add, change or remove the transition at a cut.",
    parameters: || {
        vec![
            (
                "after",
                string("The clip before the cut, such as c2 for the cut from c2 to c3, or all."),
            ),
            (
                "kind",
                words(
                    &KINDS,
                    "dissolve is a crossfade, dip goes through black, push and wipe slide across, none removes it.",
                ),
            ),
            (
                "durationSec",
                nullable_number("0.1 to 5 s. Null for 0.5 s, or the length it has."),
            ),
        ]
    },
    read: |a| {
        let after = a.clip_ref("after")?;
        let kind = a.word("kind", &KINDS)?;
        let duration_us = a.nullable_length("durationSec", &TRANSITION_SECS)?;
        let kind = match kind {
            Kind::Is(kind) => Some(kind),
            Kind::None if duration_us.is_some() => {
                return Err(a.error(
                    "durationSec",
                    Problem::Unused {
                        why: "goes with a transition: make it null when kind is none",
                    },
                ));
            }
            Kind::None => None,
        };
        Ok(ToolCall::SetTransition {
            after,
            kind,
            duration_us,
        })
    },
};
