//! A shape mask on the main footage masks its cursor too: the built-in `effect.mask`'s settings, read
//! from the footage's packed pass for this frame, so the GPU pass clips the cursor and its ripples to the same shape.
//! The footage's other effects (blur, pixelate, a vignette…) never reach the cursor: it is drawn over the processed
//! footage, sharp at the output size.

use klipolo_model::contrib::builtin;

use crate::plan::{CursorMask, EffectPass};

/// The first shape mask among `effects` (the main footage's enabled passes, in order), or None. O(passes + settings),
/// no allocation.
pub(super) fn of(effects: &[EffectPass]) -> Option<CursorMask> {
    let pass = effects
        .iter()
        .find(|p| p.shader.id.as_str() == builtin::EFFECT_MASK)?;
    let word = |key: &str, k: usize| {
        let sh = &pass.shader;
        let &(_, _, at) = sh
            .layout
            .fields
            .iter()
            .find(|f| sh.params[f.0].key == key)?;
        let b = pass.params.bytes().get(at + 4 * k..at + 4 * k + 4)?;
        Some([b[0], b[1], b[2], b[3]])
    };
    let float = |key: &str, k: usize| word(key, k).map(f32::from_le_bytes);
    Some(CursorMask {
        shape: word("shape", 0).map_or(0, u32::from_le_bytes),
        center: (float("center", 0)?, float("center", 1)?),
        corner: float("cornerRadius", 0)?,
        feather: float("feather", 0)?,
    })
}
