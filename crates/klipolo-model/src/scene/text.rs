//! A text node's richer properties: spans, outlines, the counter, the typewriter, scrambles, holds, and how it splits,
//! aligns and fits.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Flag, Paint, Val};

/// Part of a text with its own look; a `background` makes a marker highlight or a pill.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Span {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<Flag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<SpanBackground>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpanBackground {
    pub color: String,
    /// 1080p pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
}

/// A line around each glyph, in 1080p pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Outline {
    pub width: Val,
    pub color: String,
}

/// The counter: the text shows a number going from `from` to `to`, starting `at` seconds for `dur` seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Count {
    pub from: Val,
    pub to: Val,
    /// Digits after the point, 0–6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    /// Groups thousands (1,234).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<String>,
}

/// The typewriter: the text appears by characters or words from `at` over `dur` seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Reveal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<RevealBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RevealBy {
    Chars,
    Words,
}

/// Random characters (from `chars`) settling into the text from `at` over `dur` seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Scramble {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
}

/// From `t` seconds the text is `text`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HoldKey {
    pub t: f64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Split {
    Chars,
    Words,
    Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TextCase {
    Upper,
    Lower,
}

/// `shrink`: down to 50 %, then wrap within `wrap`, then clip to the box; `clip`: clip to the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    Shrink,
    Clip,
}
