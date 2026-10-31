//! The Mac probe's schedule, pure so the Linux CI runs test it: which run of which encoder comes when,
//! and for how long, so every number `probe::decide` reads is measured within the budget whatever order VideoToolbox
//! lists its encoders in.
//!
//! Runs go by what `decide` needs, across every encoder rather than one encoder after another:
//! 1. every encoder's 1080p60, paced (games' Smooth and Light, and the capacity a camera is fitted against);
//! 2. every encoder's 1440p as fast as it goes (the native rule's headroom, and the capacity past 60);
//! 3. on Apple silicon, every dedicated encoder's 1080p120, paced (Slow motion);
//! 4. every dedicated encoder's 1440p60, paced (Sharp), unless its flat-out run already showed it (`infer_1440p60`).
//!
//! The windows are 1 s for 1080p60 and 0.5 s for the rest when they all fit; otherwise every one shrinks by the same
//! factor, down to a floor that still measures (24 frames at 60 fps), and the least needed runs (last in the order) are
//! left out if even the floors don't fit. Each run is costed with `PER_RUN` for its session's making and its last
//! frames, and `RESERVE` is kept free at the end.

use std::time::Duration;

/// What a run measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    /// 1080p60 paced: `fps_1080p60` and the p95 latency.
    Hd60,
    /// 1440p flat out: `max_fps_1440p`.
    QhdFlatOut,
    /// 1080p120 paced: `fps_1080p120`.
    Hd120,
    /// 1440p60 paced: `fps_1440p60`.
    Qhd60,
}

/// One run: the encoder's place in the list, what it measures and for how long.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub encoder: usize,
    pub measure: Measure,
    pub window: Duration,
}

/// A session's making, its prepare and its last frames, as each run is costed.
pub const PER_RUN: Duration = Duration::from_millis(150);
/// Kept free at the end of the budget (the report's writing, a slow last session).
pub const RESERVE: Duration = Duration::from_millis(300);
/// A flat-out 1440p rate at least this high holds 1440p60 paced, with a 10 % margin: its paced run is skipped.
pub const INFERS_1440P60: f32 = 66.0;

const FULL_HD60: Duration = Duration::from_millis(1_000);
const FULL: Duration = Duration::from_millis(500);
const FLOOR_HD60: Duration = Duration::from_millis(400);
const FLOOR: Duration = Duration::from_millis(250);

/// The runs for encoders listed with whether each is `dedicated`, on a Mac of `cpu_arch`, within `budget`.
pub fn plan(dedicated: &[bool], cpu_arch: &str, budget: Duration) -> Vec<Run> {
    let mut runs: Vec<(usize, Measure)> = Vec::new();
    let all = || 0..dedicated.len();
    let own = || all().filter(|&i| dedicated[i]);
    runs.extend(all().map(|i| (i, Measure::Hd60)));
    runs.extend(all().map(|i| (i, Measure::QhdFlatOut)));
    if cpu_arch == "aarch64" {
        runs.extend(own().map(|i| (i, Measure::Hd120)));
    }
    runs.extend(own().map(|i| (i, Measure::Qhd60)));
    loop {
        if let Some(planned) = fitted(&runs, budget) {
            return planned;
        }
        // Even the floors don't fit: the least needed run goes. With none left, the budget is under the reserve and
        // nothing is measured (it once looped for ever).
        if runs.pop().is_none() {
            return Vec::new();
        }
    }
}

/// `runs` with windows that fit `budget`, or None when even the floors don't.
fn fitted(runs: &[(usize, Measure)], budget: Duration) -> Option<Vec<Run>> {
    let full = |m| if m == Measure::Hd60 { FULL_HD60 } else { FULL };
    let floor = |m| {
        if m == Measure::Hd60 {
            FLOOR_HD60
        } else {
            FLOOR
        }
    };
    let fixed = PER_RUN * runs.len() as u32 + RESERVE;
    let room = budget.checked_sub(fixed)?;
    let wanted: Duration = runs.iter().map(|&(_, m)| full(m)).sum();
    let scale = (room.as_secs_f64() / wanted.as_secs_f64().max(1e-9)).min(1.0);
    let planned: Vec<Run> = (runs.iter())
        .map(|&(encoder, measure)| Run {
            encoder,
            measure,
            window: full(measure).mul_f64(scale).max(floor(measure)),
        })
        .collect();
    (total(&planned) <= budget).then_some(planned)
}

/// What a plan is costed at: its windows, `PER_RUN` each and the `RESERVE`.
pub fn total(plan: &[Run]) -> Duration {
    plan.iter().map(|r| r.window + PER_RUN).sum::<Duration>() + RESERVE
}

/// Whether a dedicated encoder's 1440p60 is known from its flat-out run (`max_fps_1440p`), so its paced run is skipped
/// and `fps_1440p60` read as held.
pub fn infer_1440p60(max_fps_1440p: f32) -> bool {
    max_fps_1440p >= INFERS_1440P60
}
