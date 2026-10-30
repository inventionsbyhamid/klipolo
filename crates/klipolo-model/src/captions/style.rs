//! One caption style as data: the fixed vocabulary every style file speaks, read from
//! `captions/styles/<key>.json` (built-ins) or a pack. The engine draws any style generically; nothing here names
//! a style. `check` (in `style_check.rs`) refuses a file that breaks a rule, naming the field.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use super::motion::{Entrance, Exit, Move, Per, ShakeDef};
pub use super::paint::{contrast, hex_ok, parse_hex, Paint, VariantDef};
use crate::contrib::ContribId;
use crate::project::FormatKind;

/// What a style file holds. Option fields may be left out; `leadMs` defaults to 50 and `autofitMin` to 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct CaptionStyleDef {
    /// `STYLE_SCHEMA`: a newer one is refused with its reason, never half-read.
    pub schema: u32,
    pub id: ContribId,
    pub name: String,
    pub font: StyleFont,
    /// Per-script stand-ins for the face (Loud's Cyrillic in Rubik Black Italic).
    pub fallbacks: Vec<ScriptFont>,
    pub case: TextCase,
    pub punctuation: Punctuation,
    /// Pixels at a 1080-pixel short side (`PerFormat::size`: square defaults to portrait × 0.9).
    pub size: PerFormat<f64>,
    /// How far a page may shrink to fit its lines.
    #[serde(default = "one")]
    pub autofit_min: f64,
    pub letter_spacing_em: f64,
    pub line_height: f64,
    /// The face's average advance per character in this case: the model's width estimate for paging.
    pub char_em: f64,
    pub fill: Paint,
    /// `#RRGGBB`: what `accent` paints, unless the person picks another (`CaptionSettings::accent`).
    pub accent: String,
    pub swatches: Vec<String>,
    #[serde(default)]
    pub outline: Option<OutlineDef>,
    #[serde(default)]
    pub shadow: Option<ShadowDef>,
    #[serde(default)]
    pub boxes: Option<BoxDef>,
    pub pages: PageRules,
    pub position: PerFormat<Position>,
    pub active: ActiveDef,
    #[serde(default)]
    pub keyword: Option<KeywordDef>,
    pub enter: Entrance,
    pub tilt_deg: f64,
    #[serde(default)]
    pub shake: Option<ShakeDef>,
    pub exit: Exit,
    /// Active-word effects and word reveals start this long before the word.
    #[serde(default = "lead")]
    pub lead_ms: u32,
    pub variants: Vec<VariantDef>,
}

fn one() -> f64 {
    1.0
}

fn lead() -> u32 {
    50
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionStyleFont")]
pub struct StyleFont {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionScriptFont")]
pub struct ScriptFont {
    /// ISO 15924 (`Cyrl`).
    pub script: String,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptionTextCase")]
pub enum TextCase {
    AsSaid,
    Upper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptionPunctuation")]
pub enum Punctuation {
    Keep,
    /// Only `?` and `!` are shown.
    QuestionAndExclamation,
    DropFinalPeriod,
}

/// A value per format; a square takes its own, else the portrait's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionPerFormat")]
pub struct PerFormat<T> {
    pub portrait: T,
    pub landscape: T,
    // A path default, so `T` needn't be `Default` itself.
    #[serde(default = "Option::default", skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub square: Option<T>,
}

impl<T: Clone> PerFormat<T> {
    pub fn get(&self, f: FormatKind) -> T {
        match f {
            FormatKind::Portrait => self.portrait.clone(),
            FormatKind::Landscape => self.landscape.clone(),
            FormatKind::Square => self.square.clone().unwrap_or_else(|| self.portrait.clone()),
        }
    }

    /// Every value given, the square's when set.
    pub fn values(&self) -> impl Iterator<Item = &T> {
        [&self.portrait, &self.landscape]
            .into_iter()
            .chain(self.square.as_ref())
    }
}

impl PerFormat<f64> {
    /// A size for `f`: the square's own, else the portrait's × 0.9 (research: square and 4:5 use 0.9 of portrait).
    pub fn size(&self, f: FormatKind) -> f64 {
        match (f, self.square) {
            (FormatKind::Square, None) => self.portrait * 0.9,
            _ => self.get(f),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionOutlineDef")]
pub struct OutlineDef {
    pub color: Paint,
    pub width_em: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionShadowDef")]
pub struct ShadowDef {
    pub color: Paint,
    pub alpha: f64,
    pub dx_em: f64,
    pub dy_em: f64,
    pub blur_em: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionBoxDef")]
pub struct BoxDef {
    pub on: BoxOn,
    pub color: Paint,
    pub alpha: f64,
    /// The words on the box; `contrast` when left out (black on a light box, white on a dark one).
    #[serde(default)]
    pub text: Option<Paint>,
    pub pad_x_em: f64,
    pub pad_y_em: f64,
    pub radius_em: f64,
    /// How long the active-word box takes to move to the next word, and to grow in on a line's first.
    pub glide_ms: u32,
    pub grow_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptionBoxOn")]
pub enum BoxOn {
    Line,
    ActiveWord,
}

/// Paging.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionPageRules")]
pub struct PageRules {
    pub reveal: Reveal,
    #[serde(default)]
    pub max_words: Option<u32>,
    pub max_lines: u32,
    /// A share of the frame's width.
    pub max_line_width: PerFormat<f64>,
    #[serde(default)]
    pub max_chars: Option<PerFormat<u32>>,
    pub break_silence_ms: u32,
    pub min_page_ms: u32,
    pub max_page_ms: u32,
    pub linger_ms: u32,
    #[serde(default)]
    pub reading_cps: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptionReveal")]
pub enum Reveal {
    Word,
    Page,
}

/// The block's centre as a share of the frame's height; `seam`: at the camera/screen seam of a portrait split.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionPosition")]
pub struct Position {
    pub y: f64,
    #[serde(default)]
    pub seam: Option<bool>,
}

/// The word being said.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionActiveDef")]
pub struct ActiveDef {
    #[serde(default)]
    pub color: Option<ActiveColor>,
    #[serde(default)]
    pub scale: Option<f64>,
    #[serde(default)]
    pub glow: Option<GlowDef>,
    /// The opacity of the words still to come.
    #[serde(default)]
    pub dim_upcoming: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionActiveColor")]
pub struct ActiveColor {
    pub color: Paint,
    pub fade_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionGlowDef")]
pub struct GlowDef {
    /// The word's own colour when left out.
    #[serde(default)]
    pub color: Option<Paint>,
    pub blur_em: f64,
    pub alpha: f64,
}

/// Emphasis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionKeywordDef")]
pub struct KeywordDef {
    pub words: KeywordWords,
    pub color: Paint,
    /// `#RRGGBB` for money and gain words, and for loss words.
    pub gain: String,
    pub loss: String,
    pub scale: f64,
    #[serde(default)]
    pub glow: Option<GlowDef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptionKeywordWords")]
pub enum KeywordWords {
    Power,
    Gaming,
}
