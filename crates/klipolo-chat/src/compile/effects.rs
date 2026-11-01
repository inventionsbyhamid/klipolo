//! `setEffects` on the additions' working copy: effects added to, changed in, moved within or taken off a clip's
//! footage or a layer's stack, as the effects card does, each one checked against its own schema in the catalogue
//! (`Catalog::builtin`). A clip's look (`look.warm`, the look card's) is set the same way, by its name.
//! - An effect is named by the word after `effect.` (or `look.`) in its id: one the stack has is changed in place (its
//!   first of that name), any other is added, at `at` or last. A setting is read as its schema's kind says (a number,
//!   a colour, one of a choice's options, on or off, a point as `x,y`) and refused with what it takes.
//! - Taking an effect off is a removal (the model's `removals`): the compiler proposes it, as every removal is.

use std::collections::BTreeMap;

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::{Catalog, ContribDef, ContribKind};
use klipolo_model::ids::Id;
use klipolo_model::ops::{ClipPatch, EffectPatch, EffectTarget, Op};
use klipolo_model::params::{ParamKind, ParamValue, PointN};
use klipolo_model::timeline::{EffectInstance, Look};

use super::animate::noun;
use super::{Edit, Refused, label, to_model};
use crate::tools::{EffectChange, SettingValue};

/// What `setEffects` names: a clip's footage or a layer, by id, and how the model named it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub target: EffectTarget,
    pub said: String,
}

/// The catalogue's effects (and, on a clip, its looks) by the word after their kind: `blur`, `drop-shadow`, `warm`.
fn named<'c>(catalog: &'c Catalog, name: &str, clip: bool) -> Option<&'c ContribDef> {
    let name = name.trim().to_ascii_lowercase();
    let local = name.strip_prefix("klipolo.builtin/").unwrap_or(&name);
    let bare = (local.split_once('.')).map_or(local, |(_, word)| word);
    catalog.defs().find(|d| {
        let kinds = match d.kind {
            ContribKind::Effect => true,
            ContribKind::Look => clip,
            _ => false,
        };
        kinds
            && d.id.is_builtin()
            && (d.id.as_str().rsplit_once('/')).is_some_and(|(_, l)| {
                l == local || l.split_once('.').is_some_and(|(_, w)| w == bare)
            })
    })
}

/// The effects `setEffects` offers, by word, for a refusal that lists them.
fn offered(catalog: &Catalog, clip: bool) -> String {
    let mut words: Vec<&str> = (catalog.defs())
        .filter(|d| d.id.is_builtin())
        .filter(|d| d.kind == ContribKind::Effect || (clip && d.kind == ContribKind::Look))
        .filter_map(|d| d.id.as_str().rsplit_once('.').map(|(_, w)| w))
        .collect();
    words.sort_unstable();
    words.dedup();
    words.join(", ")
}

/// `value` as setting `key` of `def` takes it, checked against its schema; the reason for the model when it doesn't fit.
fn setting(
    work: &Project,
    def: &ContribDef,
    (key, value): (&str, &SettingValue),
) -> Result<ParamValue, String> {
    let name = (def.id.as_str().rsplit_once('.')).map_or(def.id.as_str(), |(_, w)| w);
    let Some(spec) = def.param(key) else {
        let keys: Vec<&str> = def.params.iter().map(|p| p.key.as_str()).collect();
        let takes = if keys.is_empty() {
            "none".to_owned()
        } else {
            keys.join(", ")
        };
        return Err(format!(
            "setEffects: {name} has no setting {key:?}; its settings are {takes}."
        ));
    };
    let wrong = |what: &str| format!("setEffects: {name}'s {key} takes {what}.");
    let read = match (&spec.kind, value) {
        (ParamKind::Float { min, max, .. }, SettingValue::Number(n)) => {
            if !(*min..=*max).contains(n) {
                return Err(wrong(&format!("a number from {min} to {max}")));
            }
            ParamValue::Number(*n)
        }
        (ParamKind::Angle { .. }, SettingValue::Number(n)) => ParamValue::Number(*n),
        (ParamKind::Int { min, max, .. }, SettingValue::Number(n)) => {
            if n.fract() != 0.0 || !((*min as f64)..=(*max as f64)).contains(n) {
                return Err(wrong(&format!("a whole number from {min} to {max}")));
            }
            ParamValue::Number(*n)
        }
        (ParamKind::Bool { .. }, SettingValue::Flag(b)) => ParamValue::Bool(*b),
        (ParamKind::Color { .. }, SettingValue::Text(t)) => {
            ParamValue::Text(t.to_ascii_uppercase())
        }
        (ParamKind::Choice { options, .. }, SettingValue::Text(t)) => {
            let Some(o) = options.iter().find(|o| o.eq_ignore_ascii_case(t)) else {
                return Err(wrong(&format!("one of {}", options.join(", "))));
            };
            ParamValue::Text(o.clone())
        }
        (ParamKind::Text { .. }, SettingValue::Text(t)) => ParamValue::Text(t.clone()),
        (ParamKind::Point { .. }, SettingValue::Text(t)) => {
            let xy: Vec<f64> = (t.split([',', ' ']))
                .filter(|p| !p.is_empty())
                .filter_map(|p| p.trim().parse().ok())
                .collect();
            match xy.as_slice() {
                [x, y] => ParamValue::Point(PointN { x: *x, y: *y }),
                _ => return Err(wrong("a point as two numbers, such as \"0.5,0.5\"")),
            }
        }
        (kind, _) => {
            let what = match kind {
                ParamKind::Float { .. } | ParamKind::Angle { .. } | ParamKind::Int { .. } => {
                    "a number"
                }
                ParamKind::Bool { .. } => "true or false",
                ParamKind::Color { .. } => "a hex colour like #FFD02E",
                ParamKind::Point { .. } => "a point such as \"0.5,0.5\"",
                _ => "a value Klipolo can't set from chat",
            };
            return Err(wrong(what));
        }
    };
    spec.check(&read, work)
        .map_err(|_| wrong("a value inside its range"))?;
    Ok(read)
}

/// `setEffects` on `target` of `work`: its ops in order (each effect's change in its stack, a look's change to its
/// clip), as one edit.
pub(super) fn set_effects(
    work: &Project,
    target: &Target,
    changes: &[EffectChange],
) -> Result<Edit, Refused> {
    let catalog = Catalog::builtin();
    let said = target.said.as_str();
    let (mut stack, noun_said, clip): (Vec<EffectInstance>, &str, bool) = match &target.target {
        EffectTarget::Clip { id } => {
            let Some(clip) = work.timeline.main.iter().find(|c| &c.id == id) else {
                return Err(to_model(format!("setEffects: {said} is gone.")));
            };
            (clip.effects.clone(), "clip", true)
        }
        EffectTarget::Item { id } => {
            let item = super::motion::drawn(work, (id, said), "setEffects")?;
            (item.effects.clone(), noun(item), false)
        }
    };
    let mut ops = Vec::new();
    let mut look: Option<Look> = None;
    for change in changes {
        let Some(def) = named(catalog, &change.name, clip) else {
            return Err(to_model(format!(
                "setEffects: {:?} is not an effect{}. Use one of {}.",
                change.name,
                if clip { " or a look" } else { "" },
                offered(catalog, clip)
            )));
        };
        let mut params = BTreeMap::new();
        for (key, value) in &change.settings {
            // A look's strength is the clip's, 0 to 1, as its card sets it.
            if def.kind == ContribKind::Look {
                match (key.as_str(), value) {
                    ("strength", SettingValue::Number(s)) if (0.0..=1.0).contains(s) => {
                        params.insert(key.clone(), ParamValue::Number(*s));
                        continue;
                    }
                    _ => {
                        return Err(to_model(format!(
                            "setEffects: a look takes only strength, a number from 0 to 1, not {key} {}.",
                            value.said()
                        )));
                    }
                }
            }
            let read = setting(work, def, (key, value)).map_err(to_model)?;
            params.insert(key.clone(), read);
        }
        if def.kind == ContribKind::Look {
            let EffectTarget::Clip { id } = &target.target else {
                continue;
            };
            let clip = work.timeline.main.iter().find(|c| &c.id == id);
            let mut now = look
                .clone()
                .or(clip.map(|c| c.look.clone()))
                .unwrap_or_default();
            if change.remove {
                now.preset = None;
            } else {
                now.preset = Some(def.id.clone());
                if let Some(ParamValue::Number(s)) = params.get("strength") {
                    now.strength = *s;
                }
            }
            look = Some(now);
            continue;
        }
        let effect_target = target.target.clone();
        let found = stack.iter().position(|e| e.effect == def.id);
        match (found, change.remove) {
            (None, true) => {
                return Err(to_model(format!(
                    "setEffects: {said} has no {} to remove.",
                    change.name
                )));
            }
            (Some(k), true) => {
                let gone = stack.remove(k);
                ops.push(Op::RemoveEffect {
                    target: effect_target,
                    id: gone.id,
                });
            }
            (Some(k), false) => {
                let id = stack[k].id.clone();
                if !params.is_empty() {
                    stack[k].params.extend(params.clone());
                    ops.push(Op::UpdateEffect {
                        target: effect_target.clone(),
                        id: id.clone(),
                        patch: EffectPatch {
                            enabled: Some(true),
                            params: Some(params),
                            reset: None,
                        },
                    });
                }
                if let Some(to) = change.at.filter(|to| *to != k) {
                    let moved = stack.remove(k);
                    stack.insert(to.min(stack.len()), moved);
                    ops.push(Op::MoveEffect {
                        target: effect_target,
                        id,
                        index: to,
                    });
                }
            }
            (None, false) => {
                let id = fresh_id(&stack, def);
                let effect = EffectInstance {
                    id: id.clone(),
                    effect: def.id.clone(),
                    enabled: true,
                    params,
                };
                let index = change.at.map(|to| to.min(stack.len()));
                match index {
                    Some(to) => stack.insert(to, effect.clone()),
                    None => stack.push(effect.clone()),
                }
                ops.push(Op::AddEffect {
                    target: effect_target,
                    effect,
                    index,
                });
            }
        }
    }
    if let (Some(look), EffectTarget::Clip { id }) = (look, &target.target) {
        ops.push(Op::UpdateClip {
            id: id.clone(),
            patch: ClipPatch {
                look: Some(look),
                ..ClipPatch::default()
            },
        });
    }
    Ok(Edit {
        ops,
        label: label("setEffects", vec![("target", ErrorParam::from(noun_said))]),
    })
}

/// An id no effect in `stack` has, from the effect's word: `blur`, `blur-2`.
fn fresh_id(stack: &[EffectInstance], def: &ContribDef) -> Id {
    let word = (def.id.as_str().rsplit_once('.')).map_or("fx", |(_, w)| w);
    let taken = |id: &str| stack.iter().any(|e| e.id == id);
    if !taken(word) {
        return word.to_owned();
    }
    (2..)
        .map(|n| format!("{word}-{n}"))
        .find(|id| !taken(id))
        .unwrap_or_default()
}
