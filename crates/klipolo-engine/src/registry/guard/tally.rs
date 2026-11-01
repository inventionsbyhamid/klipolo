//! The work tally: each setting and argument carries
//! the straight-line operations it decides, times the trips of the loops around them, the static cap's trip model:
//! - the arms of each `if` and `switch` whose condition or selector it reaches;
//! - the rest of the function an early exit (`return`, `discard`, `break`, `continue`) on its condition can skip,
//!   counted as the whole function.
//!
//! It is summed over branches and calls: a call maps its callee's tally through its arguments, as `costly` is mapped,
//! times the trips around the call. A number setting whose tally passes `HEAVY_ARM` is costly at both ends, and a
//! colour or point is measured at the probe set. Loops and samples under a branch keep the branch rule (the end that
//! takes it), and an early exit in a function that holds them stays costly at both ends. One entry per source, so a
//! function's tally is as long as its settings and arguments, whatever the number of branches or calls. Nested branches
//! on the same source count the inner arms twice: an over-count, which is safe.

use super::taint::Source;
use super::work::HUGE;

/// Operations a setting may decide and still be measured only where its conditions say: about 0.4 % of the cap.
pub(super) const HEAVY_ARM: f64 = 1024.0;

/// Operations decided per source, sorted by source, one entry each.
pub(super) type Tally = Vec<(Source, f64)>;

pub(super) fn add(t: &mut Tally, s: Source, w: f64) {
    if w <= 0.0 {
        return;
    }
    match t.binary_search_by(|e| e.0.cmp(&s)) {
        Ok(i) => t[i].1 = (t[i].1 + w).min(HUGE),
        Err(i) => t.insert(i, (s, w.min(HUGE))),
    }
}

/// Where a source decides the most work in a function, for the Probe note: the body offset of the
/// branch, loop or call, and the work there (`HUGE` for a loop's test, a branch around loops or samples, a call that
/// makes it costly, or an early exit in a function with work).
#[derive(Clone, Copy, Debug)]
pub(super) struct Where {
    pub weight: f64,
    pub offset: usize,
}

/// Keeps `w` as where `s` decides the most, one entry per source, sorted by source.
pub(super) fn place(list: &mut Vec<(Source, Where)>, s: Source, w: Where) {
    match list.binary_search_by(|e| e.0.cmp(&s)) {
        Ok(i) if w.weight > list[i].1.weight => list[i].1 = w,
        Ok(_) => {}
        Err(i) => list.insert(i, (s, w)),
    }
}

/// What a branch's condition decides, from its arms' operations. A group of pixels runs one arm, several or all of
/// them, so its work moves by at most the arms' total less the cheapest arm: the larger arm of an `if`.
pub(super) fn decided(arms: &[f64]) -> f64 {
    let least = arms.iter().copied().fold(f64::INFINITY, f64::min);
    if arms.is_empty() {
        0.0
    } else {
        (arms.iter().sum::<f64>() - least).clamp(0.0, HUGE)
    }
}
