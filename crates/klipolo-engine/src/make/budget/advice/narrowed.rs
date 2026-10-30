//! The advice's line for each number setting the measurement narrowed.

use klipolo_model::contrib::ContribDef;
use klipolo_model::params::ParamKind;

use crate::make::measure::Measured;

/// Each number setting the measurement narrowed (a corner past it was Over): what it is measured up to, and that past
/// it the layer draws clamped.
pub(super) fn narrowed(m: &Measured, def: &ContribDef) -> Vec<String> {
    let mut out = Vec::new();
    for (key, &(lo, hi)) in &m.envelope.ranges {
        let Some((min, max)) = def.param(key).and_then(|s| match s.kind {
            ParamKind::Float { min, max, .. } => Some((min, max)),
            ParamKind::Int { min, max, .. } => Some((min as f64, max as f64)),
            _ => None,
        }) else {
            continue;
        };
        let (low, high) = (lo > min + 1e-9, hi < max - 1e-9);
        let range = match (low, high) {
            (false, false) => continue,
            (false, true) => format!("up to {}", short(hi)),
            (true, false) => format!("down to {}", short(lo)),
            (true, true) => format!("from {} to {}", short(lo), short(hi)),
        };
        out.push(format!(
            "`{key}` is measured {range}; past that it would be Over, so it draws clamped there."
        ));
    }
    out
}

/// A setting's value as an agent would write it.
fn short(x: f64) -> String {
    let s = format!("{x:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}
