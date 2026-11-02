//! Why a call couldn't be read (`ToolError`), as one plain sentence for the model.

use std::fmt;

use klipolo_model::time::{MAX_TIME_US, US_PER_SEC};

use super::MAX_PHRASE_WORDS;

/// Why a call couldn't be read: the tool it named, the argument at fault (`ranges[1].endSec`; None for the call as a
/// whole) and the problem. Its text is one plain sentence for the model, on one line.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolError {
    pub tool: String,
    pub field: Option<String>,
    pub problem: Problem,
}

/// What was wrong with a call.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// No tool has the name called.
    UnknownTool,
    /// The arguments aren't JSON.
    NotJson,
    /// A field the tool needs isn't there.
    Missing,
    /// A field the tool doesn't take.
    Unexpected,
    /// A value of the wrong JSON type: what it must be, and what it was (`a number of seconds`, `text`).
    WrongType {
        expected: &'static str,
        got: &'static str,
    },
    /// A word that isn't one of `allowed` (or null, when `nullable`); `got` is the value given, as JSON.
    NotOneOf {
        allowed: Vec<&'static str>,
        nullable: bool,
        got: String,
    },
    /// Empty text, or a list that needs an item.
    Empty,
    /// A number of seconds outside `min..=max`.
    OutOfRange { min: f64, max: f64, got: f64 },
    /// A number that isn't seconds (a volume) outside `min..=max`.
    NotWithin { min: f64, max: f64, got: f64 },
    /// An end at or before its start: the start's field and both values as given.
    NotAfter {
        start_field: String,
        start: f64,
        end: f64,
    },
    /// More than one short noun phrase: a list, or over `MAX_PHRASE_WORDS` words.
    NotOnePhrase,
    /// A number between two of the steps it moves in (a volume of 0.333 where it moves in hundredths).
    OffStep { step: f64, got: f64 },
    /// A value the call can't use with the others as given: `why` says what it goes with.
    Unused { why: &'static str },
    /// A call that changes nothing: none of `fields` is set.
    NoChange { fields: &'static str },
    /// Text that isn't in one of the forms `expected` lists; `got` is the value given, as JSON.
    NotForm { expected: &'static str, got: String },
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tool = &self.tool;
        let field = self.field.as_deref().unwrap_or("the arguments");
        match &self.problem {
            Problem::UnknownTool => write!(f, "There is no tool called {tool:?}."),
            Problem::NotJson => write!(f, "{tool}: the arguments are not valid JSON."),
            Problem::Missing => write!(f, "{tool}: {field} is missing."),
            // The model wrote this field's name: quoted, so a line break in it stays on the line.
            Problem::Unexpected => write!(f, "{tool}: {field:?} is not an argument of this tool."),
            Problem::WrongType { expected, got } => {
                write!(f, "{tool}: {field} must be {expected}, not {got}.")
            }
            Problem::NotOneOf {
                allowed,
                nullable,
                got,
            } => {
                let or_null = if *nullable { " or null" } else { "" };
                write!(
                    f,
                    "{tool}: {field} must be one of {}{or_null}, not {got}.",
                    allowed.join(", ")
                )
            }
            Problem::Empty => write!(f, "{tool}: {field} must not be empty."),
            Problem::OutOfRange { min, max, got } => write!(
                f,
                "{tool}: {field} must be between {} and {} seconds{}.",
                said_secs(*min),
                said_secs(*max),
                given(*got)
            ),
            Problem::NotWithin { min, max, got } => write!(
                f,
                "{tool}: {field} must be between {} and {}{}.",
                said_secs(*min),
                said_secs(*max),
                given(*got)
            ),
            Problem::NotAfter {
                start_field,
                start,
                end,
            } => write!(
                f,
                "{tool}: {field} ({}) must be after {start_field} ({}).",
                said_secs(*end),
                said_secs(*start)
            ),
            Problem::NotOnePhrase => write!(
                f,
                "{tool}: {field} must be one short noun phrase naming one thing, at most {MAX_PHRASE_WORDS} words; \
                 call {tool} once for each thing."
            ),
            Problem::OffStep { step, got } => write!(
                f,
                "{tool}: {field} must be in steps of {}{}.",
                said_secs(*step),
                given(*got)
            ),
            Problem::Unused { why } => write!(f, "{tool}: {field} {why}."),
            Problem::NoChange { fields } => write!(f, "{tool}: set at least one of {fields}."),
            Problem::NotForm { expected, got } => {
                write!(f, "{tool}: {field} must be {expected}, not {got}.")
            }
        }
    }
}

impl std::error::Error for ToolError {}

/// The longest time the model stores, in seconds: a number given past it, either way, is said as "a number past 86400"
/// rather than printed (`given`).
pub(super) const LONGEST_SECS: f64 = (MAX_TIME_US / US_PER_SEC) as f64;

/// Seconds as an error says them: at most three decimals ("12.123", "15", "-1").
fn said_secs(secs: f64) -> String {
    // `+ 0.0` turns a rounded -0 into 0.
    format!("{}", (secs * 1000.0).round() / 1000.0 + 0.0)
}

/// What was given, after the rule it breaks: ", not 15", or past the longest time either way "; got a number past
/// 86400", so a model's `1e300` never comes back as hundreds of digits.
fn given(got: f64) -> String {
    let longest = said_secs(LONGEST_SECS);
    if got > LONGEST_SECS {
        format!("; got a number past {longest}")
    } else if got < -LONGEST_SECS {
        format!("; got a number below -{longest}")
    } else {
        format!(", not {}", said_secs(got))
    }
}
