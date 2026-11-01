//! What a measurement's points cover (the envelope's inside, the costliest point within it, the numbers measured inside
//! their range) and the layer's share of the memory it used.

use std::time::Duration;

use super::Point;
use crate::make::budget::Own;
use crate::make::draft::DraftLayer;
use crate::make::envelope::{Envelope, MAX_CORNERS, MAX_PROBES, Params, index_of};
use crate::make::measure::MemoryUse;
use crate::make::protocol::SizeName;
use crate::make::sizes::preview_size;
use crate::registry::ShaderKind;

/// The numbers measured inside their range: those costly at both ends, and those that bound a loop.
pub(super) fn interior_keys(draft: &DraftLayer, both: &[String]) -> Vec<String> {
    let mut keys = both.to_vec();
    if let Some(facts) = draft.facts() {
        for l in &facts.loops {
            if let Some(key) = &l.bound_param
                && !keys.contains(key)
            {
                keys.push(key.clone());
            }
        }
    }
    keys
}

pub(super) fn numeric_gt(
    value: &klipolo_model::params::ParamValue,
    base: Option<&klipolo_model::params::ParamValue>,
) -> bool {
    use klipolo_model::params::ParamValue::Number;
    matches!((value, base), (Number(a), Some(Number(b))) if a > b)
}

/// Whether `params` lie inside `env`: every number within its range, every bool and choice allowed.
pub(super) fn inside(env: &Envelope, params: &Params, draft: &DraftLayer) -> bool {
    params.iter().all(|(key, value)| {
        let in_range = match (env.ranges.get(key), value) {
            (Some(&(lo, hi)), klipolo_model::params::ParamValue::Number(x)) => {
                *x >= lo - 1e-9 && *x <= hi + 1e-9
            }
            _ => true,
        };
        let allowed = match (env.allowed.get(key), draft.def.param(key)) {
            (Some(set), Some(s)) => index_of(s, Some(value)).is_none_or(|i| set.contains(&i)),
            _ => true,
        };
        in_range && allowed
    })
}

/// The costliest point within class inside the envelope (the bools and choices are then measured on it).
pub(super) fn worst_inside<'p>(
    all: &'p [Point],
    env: &Envelope,
    draft: &DraftLayer,
) -> Option<&'p Point> {
    costliest(
        all.iter()
            .filter(|p| p.within && inside(env, &p.params, draft)),
    )
}

/// What a measurement left unmeasured, by cause: setting combinations the time cut, those past
/// the corner or probe cap, and export sizes the time cut (estimated from the preview).
#[derive(Default)]
pub(super) struct Cut {
    pub time: usize,
    pub cap: usize,
    pub exports: Vec<(SizeName, (u32, u32))>,
}

/// The agent's notes on `cut`, each naming its real cause.
pub(super) fn cut_notes(cut: &Cut, budget: Duration) -> Vec<String> {
    let secs = budget.as_secs_f64();
    let mut notes = Vec::new();
    if cut.time > 0 {
        notes.push(format!(
            "{} setting combinations weren't measured before the checker's {secs:.1} s ran out and count as Over. \
             Fewer costly settings, or a cheaper body, measure faster.",
            cut.time
        ));
    }
    if cut.cap > 0 {
        notes.push(format!(
            "{} setting combinations weren't measured: the checker measures at most {MAX_CORNERS} corners and \
             {MAX_PROBES} probe combinations a request. They count as Over, so the envelope leaves their settings \
             out. Fewer costly settings, or fewer options, measure in full.",
            cut.cap
        ));
    }
    let p1080_cut = cut.exports.iter().any(|(n, _)| *n == SizeName::P1080);
    for (name, (w, h)) in &cut.exports {
        notes.push(match name {
            // 2160p is then 4 × the 1080p figure.
            SizeName::P2160 => format!(
                "The {w}×{h} time is 4 × the 1080p figure{}: the checker's {secs:.1} s ran out before it was measured.",
                if p1080_cut { ", itself estimated from the preview" } else { "" }
            ),
            _ => format!(
                "The {w}×{h} time is estimated from the preview: the checker's {secs:.1} s ran out before it was \
                 measured."
            ),
        });
    }
    notes
}

/// The costliest of `points`: by its own class, then by the figure that
/// decided it (a Heavy point's half-size p95, a Light or Normal one's preview p95). One the time or a cap cut ranks
/// above every one measured.
pub(super) fn costliest<'p>(points: impl Iterator<Item = &'p Point>) -> Option<&'p Point> {
    points.max_by(|a, b| {
        a.class
            .cmp(&b.class)
            .then(a.decided.ms().total_cmp(&b.decided.ms()))
    })
}

/// The layer's own share of `peak` and the rest, by each kind's rule (`budget::Own`): a generator owns its
/// target (a quarter at half size), a filter its stack's ping-pong pair at the test image's size (70 % of the canvas)
/// grown by its padding at its largest, a transition its two sides and its result; anything beyond is extra.
pub(super) fn memory(draft: &DraftLayer, peak: u64) -> MemoryUse {
    let (w, h) = preview_size(draft.format);
    let own = match draft.kind {
        ShaderKind::Generator if draft.half_always() => {
            Own::generator((w.div_ceil(2), h.div_ceil(2)), 0.0)
        }
        ShaderKind::Generator => Own::generator((w, h), 0.0),
        ShaderKind::Filter => {
            let pad = draft
                .def
                .padding
                .as_ref()
                .map_or(0.0, |p| f64::from(p.max_px))
                * f64::from(h)
                / 1080.0;
            let side = |n: u32| (f64::from(n) * 0.7).ceil() as u32;
            Own::filter((side(w), side(h)), pad)
        }
        ShaderKind::Transition => Own::transition((w, h)),
    };
    own.split(peak, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make::budget::CostClass;
    use crate::make::measure::{Decided, Pcts};

    fn point(class: CostClass, decided: Decided) -> Point {
        Point {
            params: Params::new(),
            preview: Pcts::flat(decided.ms().min(30.0)),
            single_p95: decided.ms().min(30.0),
            within: class != CostClass::Over,
            class,
            decided,
            peak: 0,
            batches: None,
            half_batches: None,
            held: false,
        }
    }

    /// Points rank by their own class, then by the figure that decided it (a Heavy point's half-size
    /// p95, above any point within Normal); one the time cut ranks above every Over one measured.
    #[test]
    fn points_are_ranked_by_what_decides_their_class() {
        use CostClass::{Heavy, Normal, Over};
        let points = [
            point(Normal, Decided::Preview(1.9)),
            point(Heavy, Decided::Half(5.5)),
            point(Heavy, Decided::Half(4.0)),
        ];
        assert_eq!(
            costliest(points.iter()).unwrap().decided,
            Decided::Half(5.5)
        );
        let normal_only = [
            point(Normal, Decided::Preview(1.9)),
            point(Normal, Decided::Preview(1.2)),
        ];
        assert_eq!(
            costliest(normal_only.iter()).unwrap().decided,
            Decided::Preview(1.9)
        );
        let over = [
            point(Over, Decided::Preview(30.0)),
            point(Over, Decided::Unmeasured),
            point(Heavy, Decided::Half(5.9)),
        ];
        assert_eq!(costliest(over.iter()).unwrap().decided, Decided::Unmeasured);
    }
}
