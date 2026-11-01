//! What a measurement covers: the settings it is measured at, and the
//! envelope it bounds. A costly number narrows to a range; a costly bool or choice keeps the values that measured
//! within class (`allowed`), never a narrowed range; a colour or point that decides work (`End::Probe`) is measured
//! at its probe set and never bounded: the worst probe counts toward the class.
//! Everything else is unbounded. The engine draws a checked contribution clamped to its envelope (`clamp`): a number
//! past its range at the range's end, a bool or choice outside its values at its default.
//!
//! A number costly at both ends, or one that bounds a loop, is also measured inside its range, at 25, 50 and 75 % of
//! the range the envelope covers: the measured p95 over every point decides the class, with no static floor.
//!
//! How many settings are measured is bounded: at most `MAX_CORNERS` corner measurements (the defaults, the worst number
//! corners, each narrowing step, each bool or choice value) and `MAX_PROBES` probe combinations per request. The probed
//! settings move together by probe index (`probe_combos`): combination k sets every point to the
//! grid's k-th cell, every colour to its k-th probe colour and every number measured inside its range to its k-th
//! interior value, so ten combinations cover every value of every setting and the cap never drops one. A combination
//! the caps leave unmeasured counts as Over, so the envelope only ever covers what was measured.

use std::collections::{BTreeMap, BTreeSet};

use klipolo_model::contrib::ContribDef;
use klipolo_model::params::{ParamKind, ParamSpec, ParamValue};
use serde::{Deserialize, Serialize};

use crate::registry::guard::{CostlyParam, End};

mod values;
use values::{as_number, bounds, number};
pub use values::{clamp, default_value, defaults, index_of, numeric, probe_values};

/// One setting value per key.
pub type Params = BTreeMap<String, ParamValue>;

/// Corner measurements a request makes at most: the defaults, the number corners, the narrowing steps and the bool and
/// choice values (at most 12 corners).
pub const MAX_CORNERS: usize = 12;
/// A choice's options measured at most; later ones are never allowed.
pub const MAX_OPTIONS: usize = 8;
/// Probe combinations a request measures at most (the probe × corner combinations are capped). Each
/// is measured once, at the worst number corner inside the envelope with every bool and choice at its default. A point
/// has 9 probes besides its default and a colour 5 or 6, and a request's own value adds one, so the combinations
/// (`probe_combos`) are at most 10.
pub const MAX_PROBES: usize = 16;
/// Where inside its range a number costly at both ends, or bounding a loop, is also measured.
pub const INTERIOR: [f64; 3] = [0.25, 0.5, 0.75];
/// The narrowing steps: 50 %, then 25 % of the way from the default to the costly end.
pub const NARROWING: [f64; 2] = [0.5, 0.25];

/// What a measurement covers: a range per costly number setting, the values measured within class per costly
/// bool (`0`/`1`) or choice (option indices). A setting it doesn't name is unbounded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    #[serde(default)]
    pub ranges: BTreeMap<String, (f64, f64)>,
    #[serde(default)]
    pub allowed: BTreeMap<String, BTreeSet<u32>>,
}

/// The settings a definition is measured at, planned from its costly list.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The defaults (the request's values over them).
    pub base: Params,
    /// The worst number corners: every `Min`/`Max` number at its costly end, each `Both` number at each end (the first
    /// two each on their own, any more together), the rest at `base`. Empty without costly numbers.
    pub numbers: Vec<Params>,
    /// Each costly bool's and choice's values other than `base`'s, as (key, index).
    pub values: Vec<(String, u32)>,
    /// Each probed colour's and point's probe set, its first value `base`'s (`probe_values`).
    pub probed: Vec<(String, Vec<ParamValue>)>,
    /// The numbers costly at both ends: they are also measured inside their range (`INTERIOR`).
    pub interior: Vec<String>,
}

/// The settings that cost: the guard's (or a scene's) list, and a shader's padding setting at `Max` (it grows the
/// targets) when the list doesn't name it.
pub fn costly_list(def: &ContribDef, costly: &[CostlyParam]) -> Vec<CostlyParam> {
    let mut list = costly.to_vec();
    if let Some(key) = def.padding.as_ref().and_then(|p| p.param.as_ref())
        && !list.iter().any(|c| &c.key == key)
    {
        list.push(CostlyParam {
            key: key.clone(),
            costly_end: End::Max,
        });
    }
    list
}

/// The corners of `def` for `costly` (a costly list, so scenes use it too), the defaults first, at most
/// `MAX_CORNERS`: the worst number corners, then each bool and choice value on the first of them (or the defaults).
pub fn corners(def: &ContribDef, costly: &[CostlyParam]) -> Vec<Params> {
    let p = plan(def, costly, &Params::new());
    let on = p.numbers.first().unwrap_or(&p.base).clone();
    let mut all = vec![p.base.clone()];
    all.extend(p.numbers.iter().cloned());
    all.extend(
        p.values
            .iter()
            .map(|(key, i)| with_index(def, &on, key, *i)),
    );
    all.dedup();
    all.truncate(MAX_CORNERS);
    all
}

/// The plan for `def` at `values` over its defaults.
pub fn plan(def: &ContribDef, costly: &[CostlyParam], values: &Params) -> Plan {
    let mut base = defaults(def);
    base.extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
    let spec = |key: &str| def.param(key);
    let (mut ends, mut both) = (Vec::new(), Vec::new());
    let (mut values_out, mut probed) = (Vec::new(), Vec::new());
    for c in costly {
        let Some(s) = spec(&c.key) else { continue };
        match (&s.kind, c.costly_end) {
            (_, End::Probe) => {
                // The request's value first; the default stays a probe when they differ.
                let mut set = probe_values(s);
                if let Some(now) = base.get(&s.key)
                    && set.first() != Some(now)
                {
                    set.retain(|v| v != now);
                    set.insert(0, now.clone());
                }
                probed.push((s.key.clone(), set));
            }
            (ParamKind::Bool { .. } | ParamKind::Choice { .. }, _) => {
                let now = index_of(s, base.get(&s.key));
                let count = match &s.kind {
                    ParamKind::Choice { options, .. } => options.len().min(MAX_OPTIONS) as u32,
                    _ => 2,
                };
                values_out.extend(
                    (0..count)
                        .filter(|i| Some(*i) != now)
                        .map(|i| (s.key.clone(), i)),
                );
            }
            (_, End::Both) if bounds(s).is_some() => both.push(s),
            (_, end) if bounds(s).is_some() => ends.push((s, end)),
            _ => {}
        }
    }
    let interior = both.iter().map(|s| s.key.clone()).collect();
    let mut worst = base.clone();
    for (s, end) in &ends {
        let (lo, hi) = bounds(s).expect("filtered");
        worst.insert(
            s.key.clone(),
            number(s, if *end == End::Min { lo } else { hi }),
        );
    }
    let mut numbers = Vec::new();
    let at = |corner: &Params, group: &[&ParamSpec], high: bool| {
        let mut c = corner.clone();
        for s in group {
            let (lo, hi) = bounds(s).expect("filtered");
            c.insert(s.key.clone(), number(s, if high { hi } else { lo }));
        }
        c
    };
    let groups: Vec<Vec<&ParamSpec>> = match both.len() {
        0..=2 => both.iter().map(|s| vec![*s]).collect(),
        _ => vec![vec![both[0]], vec![both[1]], both[2..].to_vec()],
    };
    if groups.is_empty() && !ends.is_empty() {
        numbers.push(worst.clone());
    }
    for group in &groups {
        numbers.push(at(&worst, group, false));
        numbers.push(at(&worst, group, true));
    }
    numbers.retain(|c| c != &base);
    numbers.dedup();
    Plan {
        base,
        numbers,
        values: values_out,
        probed,
        interior,
    }
}

/// The probe combinations on `base`: combination k sets every probed setting to its k-th probe value and
/// every interior number (`(key, range)`) to its k-th `INTERIOR` value in that range, keeping a setting with fewer at
/// its value in `base`. Those equal to `base`, and repeats, are left out. At most `MAX_PROBES`: more would fail closed
/// (the caller counts the rest as Over).
pub fn probe_combos(
    def: &ContribDef,
    base: &Params,
    probed: &[(String, Vec<ParamValue>)],
    interior: &[(String, (f64, f64))],
) -> Vec<Params> {
    let longest = probed.iter().map(|(_, set)| set.len()).max().unwrap_or(0);
    let count = longest.max(if interior.is_empty() {
        0
    } else {
        INTERIOR.len() + 1
    });
    let mut combos: Vec<Params> = Vec::new();
    for k in 1..count {
        let mut c = base.clone();
        for (key, set) in probed {
            if let Some(v) = set.get(k) {
                c.insert(key.clone(), v.clone());
            }
        }
        for (key, (lo, hi)) in interior {
            if let (Some(f), Some(s)) = (INTERIOR.get(k - 1), def.param(key)) {
                let x = lo + f * (hi - lo);
                let x = match s.kind {
                    ParamKind::Int { .. } => x.round().clamp(lo.ceil(), hi.floor()),
                    _ => x,
                };
                c.insert(key.clone(), number(s, x));
            }
        }
        if c != *base && !combos.contains(&c) {
            combos.push(c);
        }
    }
    combos
}

/// `corner` moved `fraction` of the way from `base` toward itself, for every number it changed: an int rounded toward
/// `base`, so what is measured is what the range covers.
pub fn toward(def: &ContribDef, base: &Params, corner: &Params, fraction: f64) -> Params {
    let mut out = corner.clone();
    for (key, value) in corner {
        let (Some(s), Some(to), Some(from)) = (
            def.param(key),
            as_number(value),
            base.get(key).and_then(as_number),
        ) else {
            continue;
        };
        if to == from || bounds(s).is_none() {
            continue;
        }
        let mut x = from + fraction * (to - from);
        if matches!(s.kind, ParamKind::Int { .. }) {
            x = if to > from { x.floor() } else { x.ceil() };
        }
        out.insert(key.clone(), number(s, x));
    }
    out
}

/// `base` with the bool or choice `key` at its `index`-th value.
pub fn with_index(def: &ContribDef, base: &Params, key: &str, index: u32) -> Params {
    let mut out = base.clone();
    match def.param(key).map(|s| &s.kind) {
        Some(ParamKind::Bool { .. }) => {
            out.insert(key.into(), ParamValue::Bool(index == 1));
        }
        Some(ParamKind::Choice { options, .. }) => {
            if let Some(o) = options.get(index as usize) {
                out.insert(key.into(), ParamValue::Text(o.clone()));
            }
        }
        _ => {}
    }
    out
}

/// The range a costly number covers: each costly direction that a corner over class reached (`narrowed`) at
/// `fraction` of the way from the default, every other at its declared end. None for anything but a number.
pub fn range(
    spec: &ParamSpec,
    base: &Params,
    narrowed: (bool, bool),
    fraction: f64,
) -> Option<(f64, f64)> {
    let (lo, hi) = bounds(spec)?;
    let d = base
        .get(&spec.key)
        .and_then(as_number)
        .unwrap_or(lo)
        .clamp(lo, hi);
    let toward = |end: f64, up: bool| {
        let x = d + fraction * (end - d);
        match (&spec.kind, up) {
            (ParamKind::Int { .. }, true) => x.floor(),
            (ParamKind::Int { .. }, false) => x.ceil(),
            _ => x,
        }
    };
    Some((
        if narrowed.0 { toward(lo, false) } else { lo },
        if narrowed.1 { toward(hi, true) } else { hi },
    ))
}

#[cfg(test)]
mod tests;
