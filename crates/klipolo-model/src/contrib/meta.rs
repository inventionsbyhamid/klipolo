//! What contributions say about themselves: a description and tags, the margin an effect
//! draws past its layer, the size and rate it renders at, the blend a new layer of it starts with, and where it came
//! from. All are left out of the JSON when unset, so built-ins and older definitions read as before.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{ContribDef, ContribId, ContribKind};
use crate::params::{ParamKind, ParamValue};

/// Plugin ids starting with this belong to a person's effect library and its copies embedded in projects:
/// `lib.<random id per install>`. Packs may not use it.
pub const LIBRARY_PREFIX: &str = "lib.";
/// The widest margin any contribution draws past its layer, in 1080p pixels, whatever its definition says.
pub const MAX_PADDING_PX: f32 = 256.0;
/// What a generator layer's `source` may name: a shader generator or a scene (`scenes@1`).
pub const GENERATOR_SOURCES: [ContribKind; 2] = [ContribKind::Generator, ContribKind::Scene];

impl ContribId {
    /// A version in a person's library, or a project's embedded copy of one.
    pub fn is_library(&self) -> bool {
        self.plugin().starts_with(LIBRARY_PREFIX)
    }

    /// A scene (`scenes@1`): its local part starts with `scene.` (`klipolo.builtin/scene.<name>`,
    /// `lib.<id>/scene.<slug>.v<N>`), so scenes embedded from another computer count too.
    pub fn is_scene(&self) -> bool {
        self.0
            .split_once('/')
            .is_some_and(|(_, local)| local.starts_with("scene."))
    }
}

impl ContribDef {
    /// No settings, no downsample, not protective, and none of the optional fields.
    pub fn new(id: impl Into<ContribId>, kind: ContribKind, name: &str) -> Self {
        Self {
            id: id.into(),
            kind,
            name: name.into(),
            params: Vec::new(),
            downsample: None,
            protective: false,
            description: String::new(),
            tags: Vec::new(),
            padding: None,
            resolution: None,
            fps: None,
            blend: None,
            provenance: None,
        }
    }
}

/// How far a filter draws past its layer's box (a glow, a shadow): `px` plus `per_unit` pixels per unit
/// of the number setting `param`, at most `max_px` and [`MAX_PADDING_PX`]. The host grows the layer's effect target by
/// it on every side, clipped to the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PaddingSpec {
    /// 1080p pixels on every side.
    pub px: f32,
    /// A number setting that adds `per_unit` pixels per unit of its value (a glow's radius).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub param: Option<String>,
    #[serde(default)]
    pub per_unit: f32,
    pub max_px: f32,
}

impl PaddingSpec {
    /// The margin for these values, never negative, at most `max_px` and `MAX_PADDING_PX`. Its setting is read as the
    /// shader draws it: a finite number, else the default, within the setting's own `min` and `max`
    /// (no margin from a setting that isn't a number, which the loaders refuse).
    pub fn px_for(&self, values: &BTreeMap<String, ParamValue>, def: &ContribDef) -> f32 {
        let from_param = self.param.as_deref().map_or(0.0, |key| {
            let Some(ParamKind::Float {
                min, max, default, ..
            }) = def.param(key).map(|s| &s.kind)
            else {
                return 0.0;
            };
            let value = match values.get(key) {
                Some(ParamValue::Number(n)) if n.is_finite() => *n,
                _ => *default,
            };
            // Not `clamp`, which panics on crossed bounds: as `plan::drawn` reads it.
            value.max(*min).min(*max) as f32 * self.per_unit
        });
        // Not `clamp`, which panics on a negative cap and passes a NaN through: a definition that skipped its check
        // still gives a margin in range (a NaN anywhere gives 0).
        let cap = if self.max_px >= 0.0 {
            self.max_px.min(MAX_PADDING_PX)
        } else {
            0.0
        };
        let px = self.px + from_param;
        if px > 0.0 {
            px.min(cap)
        } else {
            0.0
        }
    }
}

/// The size a generator renders at before it is scaled to its layer. `Auto` is measured at both sizes in the render
/// worker, which picks half when it looks the same (PSNR ≥ 40 dB); built-in presets never ship it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Resolution {
    Full,
    Half,
    Auto,
}

/// Where an agent-made contribution came from: the app that made it and when, the version it was based on, and its
/// licence (`original` or an SPDX id). No prompt text. The person's content: shown, never interpreted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Provenance {
    pub made_by: String,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub based_on: Option<ContribId>,
    pub license: String,
}
