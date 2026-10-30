//! The static cost cap: a shader whose bound is over 250,000 operations or 64 texture samples per pixel is
//! refused before any GPU work, with the numbers and advice for the loop that can bring it under.

use naga::{Function, Handle};

use super::analyze::Summary;
use super::work::{HUGE, Work};
use super::{
    Diagnostic, MAX_OPS_PER_PIXEL, MAX_SAMPLES_PER_PIXEL, Placed, StaticFacts, UNKNOWN_LOOP_BOUND,
};

/// `cost.static` when the facts are over the cap, with the numbers and advice for the loop that can bring them under:
/// the total is affine in each loop's trips (both arms of a branch count), so the largest constant bound that fits
/// is exact. Unknown bounds are advised first, then the loop with the largest share.
pub(super) fn over_cap(
    p: &Placed,
    sums: &[Option<Summary>],
    entry: Handle<Function>,
    facts: &StaticFacts,
) -> Option<Diagnostic> {
    let (ops, samples) = (facts.ops_per_pixel, facts.samples_per_pixel);
    if ops <= MAX_OPS_PER_PIXEL && samples <= MAX_SAMPLES_PER_PIXEL {
        return None;
    }
    let message = format!(
        "This shader can take up to {ops} operations and {samples} texture samples per pixel; the limits are \
         {MAX_OPS_PER_PIXEL} operations per pixel and {MAX_SAMPLES_PER_PIXEL} samples."
    );
    // How often each function runs per pixel, callers before callees.
    let mut order = Vec::new();
    let mut seen = vec![false; sums.len()];
    postorder(sums, entry, &mut seen, &mut order);
    order.reverse();
    let mut runs = vec![0.0f64; sums.len()];
    runs[entry.index()] = 1.0;
    for h in &order {
        let Some(s) = sums[h.index()].as_ref() else {
            continue;
        };
        for &(callee, mult) in &s.calls {
            runs[callee.index()] = (runs[callee.index()] + runs[h.index()] * mult).min(HUGE);
        }
    }
    let total = sums[entry.index()]
        .as_ref()
        .map_or(Work::default(), |s| s.bound);
    let ops_first = ops as f64 / MAX_OPS_PER_PIXEL as f64
        >= f64::from(samples) / f64::from(MAX_SAMPLES_PER_PIXEL);
    let fit = |cap: f64, total: f64, share: f64, per_trip: f64| {
        if per_trip <= 0.0 {
            f64::INFINITY
        } else {
            ((cap - (total - share)) / per_trip).floor()
        }
    };
    // (offset, trips, share in the metric that's further over, the largest bound that fits). A loop whose bound
    // doesn't move what's over (it reads no texture when samples are over) fits at its own bound or above.
    let mut candidates: Vec<(usize, Option<u32>, f64, f64)> = Vec::new();
    for h in &order {
        let Some(s) = sums[h.index()].as_ref() else {
            continue;
        };
        for rec in &s.loops {
            let Some(offset) = rec.offset else { continue };
            let per_trip = rec.per_iter * (runs[h.index()] * rec.mult).min(HUGE);
            let trips = f64::from(rec.trips.unwrap_or(UNKNOWN_LOOP_BOUND));
            let share = per_trip * trips;
            let most = fit(MAX_OPS_PER_PIXEL as f64, total.ops, share.ops, per_trip.ops).min(fit(
                f64::from(MAX_SAMPLES_PER_PIXEL),
                total.samples,
                share.samples,
                per_trip.samples,
            ));
            if most < trips {
                candidates.push((
                    offset,
                    rec.trips,
                    if ops_first { share.ops } else { share.samples },
                    most,
                ));
            }
        }
    }
    let line = |offset: usize| p.body[..offset].matches('\n').count() + 1;
    let fixable = candidates.iter().filter(|c| c.3 >= 1.0).max_by(|a, b| {
        (a.1.is_none(), a.2)
            .partial_cmp(&(b.1.is_none(), b.2))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let largest = candidates
        .iter()
        .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
    let d = Diagnostic::error("cost.static", message);
    Some(match (fixable, largest) {
        (Some(&(offset, None, _, most)), _) => d.at(p.body, offset).hint(format!(
            "The loop at line {} has no constant bound, so it counts as {UNKNOWN_LOOP_BOUND} steps: give it a constant \
             bound of at most {most}, as in `for (var i = 0; i < {most}; i++)`.",
            line(offset)
        )),
        (Some(&(offset, Some(t), _, most)), _) => d.at(p.body, offset).hint(format!(
            "Give the loop at line {} a constant bound of at most {most} (it runs {t} steps now), or do less work in \
             each step.",
            line(offset)
        )),
        (None, Some(&(offset, ..))) => d.at(p.body, offset).hint(format!(
            "Do less work in the loop at line {}: even one step of it goes over the limits.",
            line(offset)
        )),
        (None, None) => d.hint("Do less work per pixel: fewer function calls and texture reads."),
    })
}

fn postorder(
    sums: &[Option<Summary>],
    h: Handle<Function>,
    seen: &mut [bool],
    out: &mut Vec<Handle<Function>>,
) {
    if std::mem::replace(&mut seen[h.index()], true) {
        return;
    }
    if let Some(s) = sums[h.index()].as_ref() {
        for &(callee, _) in &s.calls {
            postorder(sums, callee, seen, out);
        }
    }
    out.push(h);
}
