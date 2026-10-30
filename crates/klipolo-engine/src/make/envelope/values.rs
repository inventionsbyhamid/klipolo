//! Setting values as the measurement and the clamp read them: defaults, numbers, bool and choice indices, the probe
//! sets, and the clamp into an envelope.

use std::collections::BTreeMap;

use klipolo_model::contrib::ContribDef;
use klipolo_model::params::{ParamKind, ParamSpec, ParamValue, PointN};

use super::{Envelope, Params, with_index};
use crate::plan::drawn::effective;

/// `params` clamped into `env`: a number past its range at the range's end, a bool or choice outside its
/// allowed values at its default (or, when that is outside too, the first allowed). Each value is read as the renderer
/// draws it (`plan::drawn::effective`, N-m5): one left out at its default, one it can't read at its default, a number
/// past the definition's own bounds at the bound. A value the layer holds that draws otherwise is written back as it
/// draws, so every reader of the settings (a padded filter's margin too) sees what is drawn; a
/// setting the layer leaves out stays out unless its default lies outside the envelope.
pub fn clamp(def: &ContribDef, env: &Envelope, params: &mut Params) {
    for s in &def.params {
        if let Some(held) = params.get(&s.key)
            && let Some(drawn) = effective(s, Some(held))
            && *held != drawn
        {
            params.insert(s.key.clone(), drawn);
        }
        if let Some(&(lo, hi)) = env.ranges.get(&s.key)
            && let Some(ParamValue::Number(x)) = effective(s, params.get(&s.key))
            && (x < lo || x > hi)
        {
            params.insert(s.key.clone(), number(s, x.max(lo).min(hi)));
        }
        if let Some(allowed) = env.allowed.get(&s.key) {
            let now = index_of(s, effective(s, params.get(&s.key)).as_ref());
            if now.is_some_and(|i| !allowed.contains(&i)) {
                let default = default_value(s);
                match default.as_ref().and_then(|d| index_of(s, Some(d))) {
                    Some(i) if allowed.contains(&i) => {
                        params.insert(s.key.clone(), default.expect("indexed"));
                    }
                    _ => {
                        if let Some(&first) = allowed.iter().next() {
                            *params = with_index(def, params, &s.key, first);
                        }
                    }
                }
            }
        }
    }
}

/// Each setting at its default.
pub fn defaults(def: &ContribDef) -> Params {
    def.params
        .iter()
        .filter_map(|s| Some((s.key.clone(), default_value(s)?)))
        .collect()
}

/// A setting's default value, for the kinds a shader takes.
pub fn default_value(spec: &ParamSpec) -> Option<ParamValue> {
    Some(match &spec.kind {
        ParamKind::Float { default, .. } | ParamKind::Angle { default } => {
            ParamValue::Number(*default)
        }
        ParamKind::Int { default, .. } => ParamValue::Number(*default as f64),
        ParamKind::Bool { default } => ParamValue::Bool(*default),
        ParamKind::Choice { default, .. } | ParamKind::Color { default, .. } => {
            ParamValue::Text(default.clone())
        }
        ParamKind::Point { default } => ParamValue::Point(*default),
        _ => return None,
    })
}

/// The settings as numbers (a bool 0 or 1, a choice its option's index): what a measurement log records.
pub fn numeric(def: &ContribDef, params: &Params) -> BTreeMap<String, f64> {
    def.params
        .iter()
        .filter_map(|s| {
            let v = params.get(&s.key)?;
            let x = as_number(v).or_else(|| index_of(s, Some(v)).map(f64::from))?;
            Some((s.key.clone(), x))
        })
        .collect()
}

/// A bool's or choice's index (`false` 0, `true` 1, an option's position).
pub fn index_of(spec: &ParamSpec, value: Option<&ParamValue>) -> Option<u32> {
    match (&spec.kind, value?) {
        (ParamKind::Bool { .. }, ParamValue::Bool(b)) => Some(u32::from(*b)),
        (ParamKind::Choice { options, .. }, ParamValue::Text(t)) => {
            options.iter().position(|o| o == t).map(|i| i as u32)
        }
        _ => None,
    }
}

/// The probe set of a colour or point: points, the frame's 3×3 grid and the default;
/// colours, the default, black, white, pure red, green and blue, and alpha 0 when the colour has alpha.
pub fn probe_values(spec: &ParamSpec) -> Vec<ParamValue> {
    match &spec.kind {
        ParamKind::Point { default } => std::iter::once(*default)
            .chain(
                [0.0, 0.5, 1.0]
                    .into_iter()
                    .flat_map(|y| [0.0, 0.5, 1.0].into_iter().map(move |x| PointN { x, y })),
            )
            .map(ParamValue::Point)
            .collect(),
        ParamKind::Color { default, alpha } => {
            let suffix = if *alpha { "FF" } else { "" };
            let mut all: Vec<String> = std::iter::once(default.clone())
                .chain(
                    ["#000000", "#FFFFFF", "#FF0000", "#00FF00", "#0000FF"]
                        .map(|c| format!("{c}{suffix}")),
                )
                .collect();
            if *alpha {
                all.push(format!("{}00", default.get(..7).unwrap_or("#000000")));
            }
            all.dedup();
            all.into_iter().map(ParamValue::Text).collect()
        }
        _ => Vec::new(),
    }
}

/// A number setting's declared bounds.
pub(super) fn bounds(spec: &ParamSpec) -> Option<(f64, f64)> {
    match &spec.kind {
        ParamKind::Float { min, max, .. } => Some((*min, *max)),
        ParamKind::Int { min, max, .. } => Some((*min as f64, *max as f64)),
        ParamKind::Angle { .. } => Some((-360.0, 360.0)),
        _ => None,
    }
}

pub(super) fn number(spec: &ParamSpec, x: f64) -> ParamValue {
    ParamValue::Number(match spec.kind {
        ParamKind::Int { .. } => x.round(),
        _ => x,
    })
}

pub(super) fn as_number(value: &ParamValue) -> Option<f64> {
    match value {
        ParamValue::Number(n) => Some(*n),
        _ => None,
    }
}
