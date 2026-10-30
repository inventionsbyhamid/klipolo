//! The envelope clamp: a checked contribution is
//! drawn only within the settings this computer measured. Each costly number its envelope bounds is held inside the
//! measured range, and each costly bool or choice whose value wasn't measured within class is drawn at its default (or,
//! when that wasn't measured within class either, at the first value that was). Every value is read as the renderer
//! draws it (`plan::drawn`), so a value it can't read is its default and never "outside".
//!
//! It runs once a pass, before the padding's margin and `pack_params`, on a render copy: never on the project, whose
//! stored value stays what the person typed (the app re-measures from it). Nothing to clamp borrows the layer's own
//! settings and allocates nothing, which is every frame of a layer inside its envelope and of every built-in.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use klipolo_model::params::{ParamKind, ParamSpec, ParamValue};

use crate::make::envelope::Envelope;
use crate::plan::drawn::{Drawn, drawn};

/// How far past a measured end a number still counts as inside (as the verified store reads it).
const SLACK: f64 = 1e-9;

/// `values` as drawn within `envelope` (see the module's doc): borrowed when nothing moves, else a copy with the moved
/// settings written in. `specs` are the shader's settings; without an envelope (a built-in) nothing moves.
pub fn clamped<'a>(
    values: &'a BTreeMap<String, ParamValue>,
    specs: &[ParamSpec],
    envelope: Option<&Envelope>,
) -> Cow<'a, BTreeMap<String, ParamValue>> {
    let Some(env) = envelope else {
        return Cow::Borrowed(values);
    };
    let mut out: Option<BTreeMap<String, ParamValue>> = None;
    for spec in specs {
        let moved = match drawn(spec, values.get(&spec.key)) {
            Drawn::Number(x) => env
                .ranges
                .get(&spec.key)
                .filter(|(lo, hi)| x < lo - SLACK || x > hi + SLACK)
                .map(|&(lo, hi)| number(spec, x.max(lo).min(hi))),
            Drawn::Bool(on) => outside(env, spec, u32::from(on)),
            Drawn::Choice(index) => outside(env, spec, index),
            _ => None,
        };
        if let Some(value) = moved {
            out.get_or_insert_with(|| values.clone())
                .insert(spec.key.clone(), value);
        }
    }
    out.map_or(Cow::Borrowed(values), Cow::Owned)
}

/// What a bool or choice at `index` is drawn at when its envelope didn't measure it: its default if that was measured,
/// else the first value that was. None when it was measured, or the envelope doesn't bound it.
fn outside(env: &Envelope, spec: &ParamSpec, index: u32) -> Option<ParamValue> {
    let allowed: &BTreeSet<u32> = env.allowed.get(&spec.key)?;
    if allowed.contains(&index) {
        return None;
    }
    let default = match drawn(spec, None) {
        Drawn::Bool(on) => u32::from(on),
        Drawn::Choice(i) => i,
        _ => return None,
    };
    let to = if allowed.contains(&default) {
        default
    } else {
        *allowed.iter().next()?
    };
    match &spec.kind {
        ParamKind::Bool { .. } => Some(ParamValue::Bool(to == 1)),
        ParamKind::Choice { options, .. } => {
            options.get(to as usize).cloned().map(ParamValue::Text)
        }
        _ => None,
    }
}

/// `x` as a stored number of `spec`'s kind (an int rounded).
fn number(spec: &ParamSpec, x: f64) -> ParamValue {
    ParamValue::Number(match spec.kind {
        ParamKind::Int { .. } => x.round(),
        _ => x,
    })
}

#[cfg(test)]
mod tests;
