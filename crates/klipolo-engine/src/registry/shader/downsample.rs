//! A filter's half-size rule (moved from `shader.rs`): its definition's `downsample`, resolved against
//! its packed settings.

use klipolo_model::contrib::{ContribDef, DownsampleSpec};
use klipolo_model::params::{ParamKind, ParamUnit};

use super::{ParamLayout, ShaderKind, Slot};
use crate::plan::ParamBlock;

/// The least `atLeastPx` a filter that isn't built in may declare.
pub const MIN_DOWNSAMPLE_PX: f32 = 4.0;

/// A contribution's `DownsampleSpec` resolved against its packed settings: the f32 at
/// `param_offset`, times the pass's px, at least `at_least_px`, lets the pass run at half size. The rest of the
/// layer's stack then also runs at half size; the placed quad scales the result back up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Downsample {
    pub param_offset: usize,
    pub at_least_px: f32,
}

impl Downsample {
    /// The definition's rule, checked: a filter's number setting. A built-in may name any number setting; a filter
    /// that isn't built in only a number setting in px, from at least [`MIN_DOWNSAMPLE_PX`] output pixels, so
    /// what runs at half size is a blur-like result at least that wide. Anything else is refused, saying why.
    pub(super) fn resolve(
        def: &ContribDef,
        kind: ShaderKind,
        layout: &ParamLayout,
    ) -> Result<Option<Self>, String> {
        let Some(DownsampleSpec { param, at_least_px }) = &def.downsample else {
            return Ok(None);
        };
        if kind != ShaderKind::Filter {
            return Err(format!(
                "{}: only a filter can run at reduced size.",
                def.id
            ));
        }
        let builtin = def.id.is_builtin();
        let wide_enough = at_least_px.is_finite() && *at_least_px >= MIN_DOWNSAMPLE_PX;
        if !builtin && !wide_enough {
            return Err(format!(
                "{}: `downsample` starts at 4 px or more (`atLeastPx` is {at_least_px}).",
                def.id
            ));
        }
        layout
            .fields
            .iter()
            .find(|&&(i, slot, _)| {
                def.params[i].key == *param
                    && slot == Slot::F32
                    && match def.params[i].kind {
                        ParamKind::Float { unit, .. } => builtin || unit == Some(ParamUnit::Px),
                        _ => false,
                    }
            })
            .map(|&(_, _, offset)| {
                Some(Self {
                    param_offset: offset,
                    at_least_px: *at_least_px,
                })
            })
            .ok_or_else(|| {
                if builtin {
                    format!(
                        "{}: \"{param}\" isn't a number setting, so it can't choose the size.",
                        def.id
                    )
                } else {
                    format!(
                        "{}: \"{param}\" isn't a number setting in px, so it can't choose the size.",
                        def.id
                    )
                }
            })
    }

    /// Whether `params` (packed for this shader), at `px` output pixels per 1080p pixel, let the pass run at half
    /// size.
    pub fn applies(&self, params: &ParamBlock, px: f32) -> bool {
        let bytes = params.bytes();
        let Some(raw) = bytes.get(self.param_offset..self.param_offset + 4) else {
            return false;
        };
        f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) * px >= self.at_least_px
    }
}
