//! Effect stacks on clips and layers. Validation of what the stacks hold runs after the batch (`validate`).

use crate::error::{invalid, not_found, Result};
use crate::messages::code;
use crate::ops::{EffectPatch, EffectTarget};
use crate::project::Project;
use crate::timeline::EffectInstance;

use super::places::Places;
use super::{fresh_id, LOCKED};

/// The stack `target` names, found through the batch's index. Layers on locked tracks refuse edits.
fn stack<'a>(
    p: &'a mut Project,
    target: &EffectTarget,
    places: &mut Places,
) -> Result<&'a mut Vec<EffectInstance>> {
    match target {
        EffectTarget::Clip { id } => {
            let i = places
                .clip(&p.timeline.main, id)
                .ok_or_else(|| not_found("clip", id))?;
            Ok(&mut p.timeline.main[i].effects)
        }
        EffectTarget::Item { id } => {
            let (t, i) = places
                .item(&p.timeline.tracks, id)
                .ok_or_else(|| not_found("layer", id))?;
            let track = &mut p.timeline.tracks[t];
            if track.locked {
                return Err(invalid(LOCKED));
            }
            Ok(&mut track.items[i].effects)
        }
    }
}

fn position(stack: &[EffectInstance], id: &str) -> Result<usize> {
    stack
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| not_found("effect", id))
}

pub(super) fn add_effect(
    p: &mut Project,
    target: &EffectTarget,
    effect: &EffectInstance,
    index: Option<usize>,
    places: &mut Places,
) -> Result<()> {
    let stack = stack(p, target, places)?;
    let mut effect = effect.clone();
    if effect.id.is_empty() {
        effect.id = fresh_id("fx".into(), |id| stack.iter().any(|e| e.id == id));
    } else if stack.iter().any(|e| e.id == effect.id) {
        return Err(invalid(code!("model.effect.alreadyThere")));
    }
    let at = index.unwrap_or(stack.len()).min(stack.len());
    stack.insert(at, effect);
    Ok(())
}

pub(super) fn update_effect(
    p: &mut Project,
    target: &EffectTarget,
    id: &str,
    patch: &EffectPatch,
    places: &mut Places,
) -> Result<()> {
    let stack = stack(p, target, places)?;
    let i = position(stack, id)?;
    let effect = &mut stack[i];
    if let Some(enabled) = patch.enabled {
        effect.enabled = enabled;
    }
    for key in patch.reset.iter().flatten() {
        effect.params.remove(key);
    }
    for (key, value) in patch.params.iter().flatten() {
        effect.params.insert(key.clone(), value.clone());
    }
    Ok(())
}

pub(super) fn remove_effect(
    p: &mut Project,
    target: &EffectTarget,
    id: &str,
    places: &mut Places,
) -> Result<()> {
    let stack = stack(p, target, places)?;
    let i = position(stack, id)?;
    stack.remove(i);
    Ok(())
}

pub(super) fn move_effect(
    p: &mut Project,
    target: &EffectTarget,
    id: &str,
    index: usize,
    places: &mut Places,
) -> Result<()> {
    let stack = stack(p, target, places)?;
    let i = position(stack, id)?;
    let effect = stack.remove(i);
    let at = index.min(stack.len());
    stack.insert(at, effect);
    Ok(())
}
