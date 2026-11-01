//! The measurement itself. Every combination of settings is stepped
//! first (`steps`); one that stops TooSlow or Over is Over, with nothing more drawn for it. One that passes gets 30
//! frames at spread-out times at preview size (and at half size when it may run there and needs it to be in class).
//!
//! 1. The defaults (the request's values over them). Over: nothing else is measured.
//! 2. The worst number corners. When one is Over, the costly numbers try 50 % then 25 % of the way from the default
//!    (each stepped again); the farthest point that isn't Over bounds each range that corner pushed.
//! 3. Each costly bool and choice value on the worst number point inside the envelope: one that is Over is left out of
//!    `allowed`, never narrowed.
//! 4. The probe combinations on that point (`envelope::probe_combos`): colours and points that decide work, and the
//!    numbers costly at both ends or bounding a loop at 25, 50 and 75 % of their envelope's range, all moved together
//!    by probe index, with fewer frames. A probe can't be left out, so its cost counts toward what the
//!    envelope covers; there is no static floor.
//! 5. 1080p (and 2160p when asked) at the worst point within class only, stopping at a frame over `STEP_LIMIT_MS`.
//!
//! The layer's cost is each frame less the baseline's median at that size; where frames are timed by fences, each sample
//! time's slope between a single frame and a batch, less the baseline's (`batched`). A point stops as soon as its p95 is surely over its class's limit
//! (Heavy's for a `resolution: half` generator, whose frames are its half-size ones), and points are ranked by the
//! figure that decides their class. The whole measurement has a time budget below the request's
//! deadline, counted from the request's arrival (`Deadline`): what it leaves unmeasured counts as Over, with a
//! note naming the cause, the time, the corner or probe cap, or an export size estimated from the preview.

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use super::{COMPILE_BUDGET, Deadline, Decided, Fenced, Measured, Meter, Pcts};
use crate::headless::Headless;
use crate::make::budget::{Budget, CostClass, OVER_AT_PREVIEW_MS, budget_for};
use crate::make::draft::DraftLayer;
use crate::make::envelope::{
    self, Envelope, MAX_PROBES, NARROWING, Params, index_of, probe_combos, toward, with_index,
};
use crate::make::protocol::{MeasureSpec, SizeName};
use crate::make::sizes::{export_size, preview_size};
use crate::platform::gpu::GpuClass;
use crate::registry::guard::{CostlyParam, End};

/// Frames a combination gets at preview size, and a probe; a request may ask for up to `MAX_FRAMES`.
pub const FRAMES: usize = 30;
pub const MAX_FRAMES: usize = 60;
const PROBE_FRAMES: usize = 8;
/// Frames of the baseline at each size.
const BASELINE_FRAMES: usize = 8;

mod baseline;
mod batched;
mod covered;
mod point;
mod state;
use covered::{Cut, costliest, cut_notes, inside, interior_keys, memory, numeric_gt, worst_inside};
use point::over;
use state::{Point, Run};

/// `draft` measured for `costly` (the guard's list plus the padding's setting, or a scene's). An error is the GPU's
/// (a lost device): the worker answers it as a failure.
pub async fn measure(
    meter: &mut dyn Meter,
    h: &mut Headless,
    draft: &DraftLayer,
    spec: &MeasureSpec,
    costly: &[CostlyParam],
) -> Result<Measured, String> {
    let deadline = Deadline::new(Instant::now(), spec.budget_ms, COMPILE_BUDGET);
    let b = budget_for(GpuClass::of(&h.device().adapter.get_info()));
    measure_noted(meter, h, draft, spec, costly, deadline, &b)
        .await
        .map(|(m, _)| m)
}

/// `measure`, with notes for the agent: what the time budget or a cap left unmeasured. `b` is this GPU's table
/// (`budget_for`), the one the class by time is decided against, as memory and CPU are.
pub async fn measure_noted(
    meter: &mut dyn Meter,
    h: &mut Headless,
    draft: &DraftLayer,
    spec: &MeasureSpec,
    costly: &[CostlyParam],
    deadline: Deadline,
    b: &Budget,
) -> Result<(Measured, Vec<String>), String> {
    let frames = if spec.frames == 0 {
        FRAMES
    } else {
        (spec.frames as usize).clamp(FRAMES, MAX_FRAMES)
    };
    let fenced = meter.fenced(h);
    let mut run = Run {
        meter,
        h,
        draft,
        frames,
        corners: 0,
        budget: *b,
        baselines: HashMap::new(),
        worst: Pcts::default(),
        deadline: deadline.at,
        cut: Cut::default(),
        timed_out: false,
        fenced,
    };
    let def = &draft.def;
    let plan = envelope::plan(def, costly, &spec.params);
    let defaults = run.point(plan.base.clone(), frames).await?;
    let mut env = Envelope::default();
    let mut all = vec![defaults.clone()];
    let mut probes = Vec::new();
    if defaults.within {
        let mut corners = Vec::new();
        for c in &plan.numbers {
            corners.push(run.point(c.clone(), frames).await?);
        }
        let over_corners: Vec<Params> = corners
            .iter()
            .filter(|p| !p.within)
            .map(|p| p.params.clone())
            .collect();
        let mut pushed = BTreeSet::new();
        for c in &over_corners {
            for (key, value) in c {
                if plan.base.get(key) != Some(value) {
                    let up = numeric_gt(value, plan.base.get(key));
                    pushed.insert((key.clone(), up));
                }
            }
        }
        let mut fraction = 1.0;
        if !over_corners.is_empty() {
            fraction = 0.0;
            for f in NARROWING {
                let mut tried = Vec::new();
                for c in &over_corners {
                    let p = run.point(toward(def, &plan.base, c, f), frames).await?;
                    let ok = p.within;
                    tried.push(p);
                    if !ok {
                        break;
                    }
                }
                let ok = tried.len() == over_corners.len() && tried.iter().all(|p| p.within);
                all.extend(tried);
                if ok {
                    fraction = f;
                    break;
                }
            }
        }
        all.extend(corners);
        for c in costly.iter().filter(|c| c.costly_end != End::Probe) {
            let Some(s) = def.param(&c.key) else { continue };
            let narrowed = (
                pushed.contains(&(c.key.clone(), false)),
                pushed.contains(&(c.key.clone(), true)),
            );
            if let Some(r) = envelope::range(s, &plan.base, narrowed, fraction) {
                env.ranges.insert(c.key.clone(), r);
            }
        }
        let vbase = worst_inside(&all, &env, draft).map_or(plan.base.clone(), |p| p.params.clone());
        for (key, _) in &plan.values {
            if let Some(s) = def.param(key)
                && let Some(i) = index_of(s, plan.base.get(key))
            {
                env.allowed.entry(key.clone()).or_default().insert(i);
            }
        }
        for (key, i) in &plan.values {
            let p = run.point(with_index(def, &vbase, key, *i), frames).await?;
            if p.within {
                env.allowed.entry(key.clone()).or_default().insert(*i);
            }
            all.push(p);
        }
        // Inside the range of each number costly at both ends, or bounding a loop.
        let interior: Vec<(String, (f64, f64))> = interior_keys(draft, &plan.interior)
            .into_iter()
            .filter_map(|key| Some((key.clone(), *env.ranges.get(&key)?)))
            .collect();
        let combos = probe_combos(def, &vbase, &plan.probed, &interior);
        for (k, params) in combos.into_iter().enumerate() {
            probes.push(if k < MAX_PROBES {
                run.probe(params).await?
            } else {
                run.cut.cap += 1;
                over(params, f64::INFINITY, false)
            });
        }
    }
    let covered: Vec<&Point> = all
        .iter()
        .filter(|p| p.within && inside(&env, &p.params, draft))
        .chain(probes.iter())
        .collect();
    // Each point has its own class; the worst point decides. A figure that wasn't measured is never
    // reported (the class says Over, and the cost says it was cut).
    let decider = costliest(std::iter::once(&defaults).chain(covered.iter().copied()));
    let (class, decided) = decider.map_or((CostClass::Over, Decided::Unmeasured), |p| {
        (p.class, p.decided)
    });
    let counted = || std::iter::once(&defaults).chain(covered.iter().copied());
    let preview = counted()
        .map(|p| p.preview)
        .filter(|p| p.p50.is_finite() && p.p95.is_finite())
        .reduce(Pcts::worse)
        .unwrap_or(Pcts::flat(OVER_AT_PREVIEW_MS));
    let within: Option<&Point> = costliest(covered.iter().copied().filter(|p| p.within));
    let ratio = |size: (u32, u32)| {
        let p = preview_size(draft.format);
        f64::from(size.0) * f64::from(size.1) / (f64::from(p.0) * f64::from(p.1))
    };
    let p1080_size = export_size(draft.format, SizeName::P1080);
    let r1080 = ratio(p1080_size);
    let (p1080, batches_1080, peak) = match within {
        Some(w) => match run.export(w, SizeName::P1080, p1080_size, r1080).await? {
            Some(t) => (t.ms, t.batches, w.peak),
            None => (Pcts::flat(w.preview.p95 * r1080), None, w.peak),
        },
        None => (Pcts::flat(preview.p95 * r1080), None, defaults.peak),
    };
    let (p2160, batches_2160) = match (within, spec.sizes.contains(&SizeName::P2160)) {
        (Some(w), true) => {
            let size = export_size(draft.format, SizeName::P2160);
            match run.export(w, SizeName::P2160, size, ratio(size)).await? {
                Some(t) => (Some(t.ms.p95), t.batches),
                None => (None, None),
            }
        }
        _ => (None, None),
    };
    let baseline_peak = run
        .baselines
        .get(&preview_size(draft.format))
        .map_or(0, |b| b.peak);
    let notes = cut_notes(&run.cut, deadline.budget);
    // What the batches measured, and whether every point counted was resolved at preview size.
    let fenced = run.fenced.then(|| Fenced {
        resolved: counted().all(|p| p.batches.as_ref().is_some_and(|b| b.resolved)),
        // m-d: only when the held point decided the class.
        held: decider.is_some_and(|p| p.held),
        preview: decider.and_then(|p| p.batches.clone()),
        half: decider.and_then(|p| p.half_batches.clone()),
        p1080: batches_1080,
        p2160: batches_2160,
    });
    let measured = Measured {
        class,
        decided,
        preview,
        p1080,
        p2160,
        cpu_us: None,
        bytes: memory(draft, peak.saturating_sub(baseline_peak)),
        envelope: env,
        worst: run.worst.worse(preview),
        fenced,
    };
    Ok((measured, notes))
}
