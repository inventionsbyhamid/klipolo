//! The overlay planner's smaller parts (moved from `overlay.rs`): unknown motion presets named as missing, the
//! `blur-in` preset's pass, the typewriter's cut, whether a scene's generator fills animate and whether it has a
//! backdrop.

use klipolo_model::contrib::ContribKind;
use klipolo_model::motion::Motion;
use klipolo_model::scene::timeline::CompiledScene;

use super::super::effects::{self, Clock};
use super::super::motion::Moved;
use super::super::strings::Table;
use super::super::{EffectPass, MissingContrib};
use crate::registry::Registry;

/// Whether a generator `scene` draws inside its shapes animates, so it is never still.
pub(super) fn fills_animate(scene: &CompiledScene, registry: &Registry) -> bool {
    scene
        .facts
        .generator_fills
        .iter()
        .any(|id| registry.shader(id).is_some_and(|sh| sh.animates))
}

/// Whether `scene` has a backdrop node (the compositor grabs the frame under its layer first).
pub(super) fn has_backdrop(scene: &CompiledScene) -> bool {
    scene
        .nodes
        .iter()
        .any(|n| n.node.kind == klipolo_model::scene::NodeType::Backdrop)
}

/// Names in `missing` each of `m`'s presets this build doesn't draw (they do nothing). Allocation-free when they
/// are all known.
pub(super) fn note_unknown(
    m: &Motion,
    registry: &Registry,
    s: &mut Table,
    missing: &mut Vec<MissingContrib>,
) {
    for u in [&m.enter, &m.exit, &m.looping].into_iter().flatten() {
        if registry.motion_preset(&u.preset).is_none() {
            effects::note_missing(missing, ContribKind::Motion, s.str(u.preset.as_str()));
        }
    }
}

/// While `blur-in` runs, the built-in blur is one more pass at the end of the layer's stack (the effect path, as with
/// any effect); nothing otherwise.
pub(super) fn blur_in(
    registry: &Registry,
    layer_id: &str,
    mv: &Moved,
    clock: Clock,
    fx: &mut Vec<EffectPass>,
) {
    if mv.blur_px > 0.0
        && let Some(pass) = effects::blur_pass(registry, layer_id, mv.blur_px, clock)
    {
        fx.push(pass);
    }
}

/// The first `reveal` (0–1) of `text` by characters, rounded up (the typewriter's cut); all of it at 1.
pub(super) fn typed(text: &str, reveal: f64) -> &str {
    if reveal >= 1.0 {
        return text;
    }
    let chars = text.chars().count();
    let keep = (chars as f64 * reveal).ceil() as usize;
    let end = text.char_indices().nth(keep).map_or(text.len(), |(i, _)| i);
    &text[..end]
}
