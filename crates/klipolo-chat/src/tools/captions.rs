//! The captions' look `setCaptions` sets over the style chosen (`set`, its keys `CAPTION_KEYS`): a font by name, a size
//! as a factor of the style's own (`captions::styles::CAPTION_SIZES`), a position (a place word or a height share), the
//! words' colour and the colour a said word or keyword lights up in (the style's accent), and uppercase. `none` gives
//! back the style's own. Whether a font has that name is the compiler's to judge.

use serde_json::Value;

use klipolo_model::captions::style::hex_ok;
use klipolo_model::captions::styles::CAPTION_SIZES;

use super::args::Args;
use super::error::{Problem, ToolError};
use super::sets::{Pair, pairs};

/// Where the captions sit: as the Captions panel's place words, or a share of the frame's height (their centre).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaptionPlace {
    Top,
    Middle,
    /// The lower third.
    Bottom,
    At(f64),
}

/// The heights a position may take: the model's own (`model.captions.place`).
pub const PLACE_YS: (f64, f64) = (0.05, 0.95);

/// What `set` changes of the captions' look; None keeps each, `Some(None)` gives back the style's own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptionChange {
    pub font: Option<Option<String>>,
    /// A factor of the style's size; 1 is its own.
    pub size: Option<f64>,
    pub position: Option<CaptionPlace>,
    /// `#RRGGBB`, upper case.
    pub color: Option<Option<String>>,
    /// The style's accent: what a said word or keyword lights up in.
    pub highlight: Option<Option<String>>,
    pub upper: Option<Option<bool>>,
}

/// The keys `set` takes.
pub const CAPTION_KEYS: [&str; 6] = ["font", "size", "position", "color", "highlight", "upper"];

/// Whether `p`'s value is the word `none` (any case).
fn none(p: &Pair) -> bool {
    p.value
        .as_str()
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("none"))
}

/// A `#RRGGBB` colour (upper case), or None for `none`.
fn color(p: &Pair, a: &Args) -> Result<Option<String>, ToolError> {
    if none(p) {
        return Ok(None);
    }
    let text = p.value.as_str().map(str::trim).unwrap_or_default();
    if hex_ok(text) {
        return Ok(Some(text.to_ascii_uppercase()));
    }
    Err(a.error(
        &format!("{}.value", p.at),
        Problem::NotForm {
            expected: "a hex colour like #FFD02E, or none",
            got: p.value.to_string(),
        },
    ))
}

fn place(p: &Pair, a: &Args) -> Result<CaptionPlace, ToolError> {
    if p.value.is_number() {
        return p.number(a, PLACE_YS).map(CaptionPlace::At);
    }
    let words = ["top", "middle", "bottom"];
    let word = p.word(a, &words).map_err(|_| {
        a.error(
            &format!("{}.value", p.at),
            Problem::NotForm {
                expected: "top, middle, bottom, or a height from 0.05 to 0.95",
                got: p.value.to_string(),
            },
        )
    })?;
    Ok(match word {
        "top" => CaptionPlace::Top,
        "middle" => CaptionPlace::Middle,
        _ => CaptionPlace::Bottom,
    })
}

/// `setCaptions`' `set` (left out or null: no change), its pairs read into a `CaptionChange`.
pub(super) fn read_look(a: &mut Args) -> Result<CaptionChange, ToolError> {
    if a.map.get("set").is_none_or(Value::is_null) {
        a.map.remove("set");
        return Ok(CaptionChange::default());
    }
    let mut change = CaptionChange::default();
    for p in &pairs(a, "set", &CAPTION_KEYS)? {
        match p.key {
            "font" => change.font = Some((!none(p)).then(|| p.text(a)).transpose()?),
            "size" => change.size = Some(p.number(a, (*CAPTION_SIZES.start(), *CAPTION_SIZES.end()))?),
            "position" => change.position = Some(place(p, a)?),
            "color" => change.color = Some(color(p, a)?),
            "highlight" => change.highlight = Some(color(p, a)?),
            _ => change.upper = Some((!none(p)).then(|| p.flag(a)).transpose()?),
        }
    }
    Ok(change)
}
