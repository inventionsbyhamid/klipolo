//! The cursor layer's settings: how a take recorded its cursor ([`RecordingCursor`]), how the
//! project and each clip draw it ([`CursorStyle`]), what an Auto zoom plans from ([`FocusKind`]) and how close a
//! following crop zooms ([`Follow`]). Types and their checks only: drawing and planning are the engine's.

use std::ops::RangeInclusive;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{invalid, ModelError};
use crate::messages::code;
use crate::time::Us;
use crate::timeline::Clip;

mod rgba;

pub use rgba::Rgba;

/// How a take's screen file shows the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum CursorCapture {
    /// No cursor in the frames: Klipolo draws it from the cursor file.
    Separate,
    /// The system drew it into the frames.
    Burned,
    /// Separate until `at_us` on the recording clock, then burned in by the recorder's fallback.
    BurnedFrom { at_us: Us },
}

impl CursorCapture {
    /// Klipolo draws the cursor at recording time `rec_us` (the frames don't show it there).
    pub fn separate_at(self, rec_us: Us) -> bool {
        match self {
            Self::Separate => true,
            Self::Burned => false,
            Self::BurnedFrom { at_us } => rec_us < at_us,
        }
    }
}

/// How a take's clicks were recorded. Ordered strongest first: `weakest` of a take is what it can promise (the `max`
/// of the modes it saw).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClickSource {
    /// Exact button events at their own hardware times.
    Events,
    /// The no-permission fallback, sampled at 240 Hz (±4.2 ms).
    Sampled,
    /// No clicks (Wayland).
    None,
}

/// A screen take's cursor: its file, how the frames show it, and how its clicks were recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecordingCursor {
    pub path: String,
    pub capture: CursorCapture,
    pub clicks: ClickSource,
}

impl RecordingCursor {
    /// A take whose frames show the cursor, with sampled clicks: what a project saved before this record names by its
    /// path alone, and test fixtures. Import takes the recorder's manifest's own values.
    pub fn burned_in(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            capture: CursorCapture::Burned,
            clicks: ClickSource::Sampled,
        }
    }
}

/// What draws for the cursor: the take's own shapes, or one of Klipolo's analytic looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CursorLook {
    Recorded,
    Dot,
    Arrow,
}

/// How much the drawn path is smoothed (a critically damped filter).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Smoothing {
    Off,
    Light,
    Medium,
    Strong,
}

impl Smoothing {
    /// The filter's speed (1/s): Light 40, Medium 22, Strong 12; Off None (the raw path).
    pub fn omega(self) -> Option<f64> {
        match self {
            Self::Off => None,
            Self::Light => Some(40.0),
            Self::Medium => Some(22.0),
            Self::Strong => Some(12.0),
        }
    }
}

/// What a real click shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClickEffect {
    None,
    Ripple,
    Press,
    RippleAndPress,
}

/// What an Auto zoom plans from.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FocusKind {
    Clicks,
    Pauses,
    Typing,
}

/// How the cursor is drawn: the project's, or one clip's own. Missing fields take the defaults.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct CursorStyle {
    /// The take's shapes on the raw path at size 1 with no effects; the other fields are kept for when it's off.
    pub as_recorded: bool,
    pub hidden: bool,
    pub look: CursorLook,
    /// Times the recorded size, within [`CursorStyle::SIZE`].
    pub size: f64,
    pub smoothing: Smoothing,
    /// Fade out after this long without moving, within [`CursorStyle::HIDE_US`]; None never hides.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hide_when_still_us: Option<Us>,
    pub click: ClickEffect,
    /// `#RRGGBBAA` in the JSON.
    #[ts(type = "string")]
    pub click_color: Rgba,
    /// 0 to 1 of a 180° shutter's smear.
    pub motion_blur: f64,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self {
            as_recorded: false,
            hidden: false,
            look: CursorLook::Recorded,
            size: 1.25,
            smoothing: Smoothing::Medium,
            hide_when_still_us: None,
            click: ClickEffect::Ripple,
            click_color: Rgba([0xFF, 0xFF, 0xFF, 0x80]),
            motion_blur: 0.5,
        }
    }
}

impl CursorStyle {
    pub const SIZE: RangeInclusive<f64> = 0.5..=4.0;
    pub const HIDE_US: RangeInclusive<Us> = 500_000..=10_000_000;
    const BLUR: RangeInclusive<f64> = 0.0..=1.0;

    /// What draws: while `as_recorded`, Recorded at size 1, Off, never hidden when still, no click effect, no blur
    /// (`hidden` still wins); otherwise itself.
    pub fn effective(&self) -> CursorStyle {
        if !self.as_recorded {
            return *self;
        }
        CursorStyle {
            look: CursorLook::Recorded,
            size: 1.0,
            smoothing: Smoothing::Off,
            hide_when_still_us: None,
            click: ClickEffect::None,
            motion_blur: 0.0,
            ..*self
        }
    }

    /// `model.cursor.size`, `model.cursor.hide`, `model.cursor.blur` (the colour is checked as it's read).
    pub fn check(&self) -> Result<(), ModelError> {
        if !Self::SIZE.contains(&self.size) {
            return Err(invalid(code!("model.cursor.size")));
        }
        if self
            .hide_when_still_us
            .is_some_and(|us| !Self::HIDE_US.contains(&us))
        {
            return Err(invalid(code!("model.cursor.hide")));
        }
        if !Self::BLUR.contains(&self.motion_blur) {
            return Err(invalid(code!("model.cursor.blur")));
        }
        Ok(())
    }
}

/// A following Fill crop (`Framing::follow`): how much closer than the plain crop it zooms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Follow {
    pub closer: f64,
}

impl Follow {
    pub const CLOSER: RangeInclusive<f64> = 1.0..=2.0;
}

/// A clip's cursor settings: its own style, and each format's follow within [`Follow::CLOSER`]
/// (`model.cursor.closer`).
pub(crate) fn check_clip(c: &Clip) -> Result<(), ModelError> {
    if let Some(style) = &c.cursor {
        style.check()?;
    }
    let close = |f: &Follow| Follow::CLOSER.contains(&f.closer);
    if !c
        .framing
        .values()
        .filter_map(|f| f.follow.as_ref())
        .all(close)
    {
        return Err(invalid(code!("model.cursor.closer")));
    }
    Ok(())
}
