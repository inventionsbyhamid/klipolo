//! One combination measured (`Run::point`, a probe), its frames less the baseline (`frames_at`; in batches where the
//! timing is fenced, `batched`), and the worst point at an export size. A point stops as soon as its p95 is surely over its limit, and
//! nothing is measured past the run's deadline; an export size stops at a frame over the step limit
//!.

use std::sync::Arc;
use std::time::Instant;

use klipolo_model::project::Project;

use super::{PROBE_FRAMES, Point, Run};
use crate::make::budget::{Budget, CostClass, OVER_AT_PREVIEW_MS, STEP_LIMIT_MS, class_of_time};
use crate::make::draft::DraftLayer;
use crate::make::envelope::{MAX_CORNERS, Params};
use crate::make::measure::{Batches, Decided, FrameReq, Pcts};
use crate::make::protocol::SizeName;
use crate::make::sizes::preview_size;
use crate::make::steps::{SMALL_SIZES, StepOutcome, stepped};

/// How many of `n` samples must be over a limit for their p95 (nearest rank) to be over it: `n − ⌈0.95 n⌉ + 1`.
pub(crate) fn over_count_for_p95(n: usize) -> usize {
    n - (0.95 * n as f64).ceil() as usize + 1
}

impl Run<'_> {
    /// One combination: stepped, then `n` frames at preview size (and at half size when it needs that to be in class).
    /// Past `MAX_CORNERS` or the deadline it isn't measured, and counts as Over.
    pub(super) async fn point(&mut self, params: Params, n: usize) -> Result<Point, String> {
        self.corners += 1;
        if self.corners > MAX_CORNERS {
            self.cut.cap += 1;
            return Ok(over(params, f64::INFINITY, false));
        }
        self.measured(params, n).await
    }

    /// A probe combination: as `point`, with fewer frames, outside the corners' cap (`MAX_PROBES` bounds them).
    pub(super) async fn probe(&mut self, params: Params) -> Result<Point, String> {
        self.measured(params, PROBE_FRAMES).await
    }

    /// `params` measured unless the time is up; one the time cut, before or during, is counted as the time's.
    async fn measured(&mut self, params: Params, n: usize) -> Result<Point, String> {
        if self.out_of_time() {
            self.cut.time += 1;
            return Ok(over(params, f64::INFINITY, false));
        }
        self.timed_out = false;
        let mut point = self.measure_point(params, n).await?;
        if std::mem::take(&mut self.timed_out) {
            // Whatever it measured before, the time cut it: it is Over, and its figure unknown.
            self.cut.time += 1;
            point.within = false;
            point.class = CostClass::Over;
            point.decided = Decided::Unmeasured;
        }
        Ok(point)
    }

    pub(super) fn out_of_time(&self) -> bool {
        Instant::now() >= self.deadline
    }

    async fn measure_point(&mut self, params: Params, n: usize) -> Result<Point, String> {
        let layer = self.draft.at(params.clone());
        let preview = preview_size(layer.format);
        let mut sizes = SMALL_SIZES.to_vec();
        sizes.push(preview);
        let stopped = match stepped(&mut *self.meter, self.h, &layer, &sizes, STEP_LIMIT_MS).await {
            StepOutcome::Failed(e) => {
                return Err(format!("stepping up to {}×{}: {e}", preview.0, preview.1));
            }
            StepOutcome::Ok(_) => None,
            other => other.preview_ms(),
        };
        if let Some(ms) = stopped {
            self.worst = self.worst.worse(Pcts::flat(ms));
            return Ok(over(params, ms, layer.half_always()));
        }
        let normal = self.budget.normal_ms;
        // A `resolution: half` generator's frames are its half-size ones, whose class limit is Heavy's.
        let limit = if layer.half_always() {
            self.budget.heavy_ms
        } else {
            normal
        };
        let Some(full) = self
            .frames_at(&layer, preview, n, false, Some(limit))
            .await?
        else {
            self.worst = self.worst.worse(Pcts::flat(OVER_AT_PREVIEW_MS));
            return Ok(over(params, OVER_AT_PREVIEW_MS, layer.half_always()));
        };
        let may_halve = layer.may_halve();
        // Its half-size path's memory counts too: a Heavy point runs there.
        let (half, half_peak, half_batches) = if layer.half_always() {
            (Some(full.ms), 0, None)
        } else if may_halve && full.ms.p95 > normal {
            let heavy = self.budget.heavy_ms;
            match self
                .frames_at(&layer, preview, n, true, Some(heavy))
                .await?
            {
                Some(h) => (Some(h.ms), h.peak, h.batches),
                None => (None, 0, None),
            }
        } else {
            (None, 0, None)
        };
        self.worst = self.worst.worse(full.ms);
        let (mut class, decided) =
            decide(full.ms, half, may_halve, layer.half_always(), &self.budget);
        // Light needs batches that resolved it; without, never Light.
        let held = class == CostClass::Light && full.batches.as_ref().is_some_and(|b| !b.resolved);
        if held {
            class = CostClass::Normal;
        }
        Ok(Point {
            params,
            preview: full.ms,
            single_p95: full.single_p95,
            within: class != CostClass::Over,
            class,
            decided,
            peak: full.peak.max(half_peak),
            batches: full.batches,
            half_batches,
            held,
        })
    }

    /// Up to `n` frames of `layer` at `size`, less the baseline's median; where the timing is fenced, sample times
    /// timed in batches instead (`batched`). At preview size: None once a frame is over `OVER_AT_PREVIEW_MS`,
    /// and the frames so far once their p95 is surely over `over`. At an export size: the frames so far once one
    /// is over `STEP_LIMIT_MS`. None past the deadline, the baseline's measurement included.
    pub(super) async fn frames_at(
        &mut self,
        layer: &DraftLayer,
        size: (u32, u32),
        n: usize,
        half: bool,
        over: Option<f64>,
    ) -> Result<Option<Timed>, String> {
        if self.out_of_time() {
            // Nothing past the deadline, not even the baseline or the warm-up: the caller counts it.
            self.timed_out = true;
            return Ok(None);
        }
        let Some(base) = self.baseline(size).await? else {
            self.timed_out = true;
            return Ok(None);
        };
        layer.prepare(self.h, true, half);
        let project = Arc::new(layer.project());
        // The first frame at a size and scale pays for its pipelines and targets: it is drawn untimed first
        // (`Meter::warm`, once a request for each), as the steps do for the full size, so a half-size or export
        // path is never timed cold.
        let mut first = FrameReq::new(project.clone(), layer, layer.middle(), size);
        first.half = half;
        self.meter
            .warm(self.h, &first)
            .await
            .map_err(|e| format!("at {}×{}: {e}", size.0, size.1))?;
        let preview = size == preview_size(layer.format);
        if self.fenced {
            let at = At {
                layer,
                project,
                size,
                half,
                over,
                preview,
            };
            return self.batched(&at, n, &base).await;
        }
        let base = base.ms;
        let (mut ms, mut peak, mut above) = (Vec::with_capacity(n), 0, 0);
        for at in layer.times(n) {
            if self.out_of_time() {
                // The caller counts it: a point's, an export size's or the final half-size time.
                self.timed_out = true;
                return Ok(None);
            }
            let mut req = FrameReq::new(project.clone(), layer, at, size);
            req.half = half;
            let s = self
                .meter
                .frame(self.h, &req)
                .await
                .map_err(|e| format!("at {}×{}: {e}", size.0, size.1))?;
            if preview && s.gpu_ms > OVER_AT_PREVIEW_MS {
                return Ok(None);
            }
            let cost = (s.gpu_ms - base).max(0.0);
            ms.push(cost);
            peak = peak.max(s.peak_bytes);
            above += usize::from(over.is_some_and(|limit| cost > limit));
            if above >= over_count_for_p95(n) || (!preview && s.gpu_ms > STEP_LIMIT_MS) {
                break;
            }
        }
        let ms = Pcts::of(&ms);
        Ok(Some(Timed {
            ms,
            peak,
            single_p95: ms.p95,
            batches: None,
        }))
    }

    /// The worst point within class at the export size `name` (`size`), unless predicted over the step limit (then
    /// None); one the time cut is noted.
    pub(super) async fn export(
        &mut self,
        w: &Point,
        name: SizeName,
        size: (u32, u32),
        ratio: f64,
    ) -> Result<Option<Timed>, String> {
        // From its single frames: a batch's slope leaves out the wait a frame also takes.
        if w.single_p95 * ratio > STEP_LIMIT_MS {
            return Ok(None);
        }
        let layer = self.draft.at(w.params.clone());
        self.timed_out = false;
        let m = self
            .frames_at(&layer, size, self.frames, false, None)
            .await?;
        if std::mem::take(&mut self.timed_out) {
            self.cut.exports.push((name, size));
        }
        Ok(m)
    }
}

/// Frames at one size: their figures, the effect targets' peak, their single frames' p95 less the baseline's (what an
/// export size is predicted from), and where fenced, the batches they came from.
pub(super) struct Timed {
    pub ms: Pcts,
    pub peak: u64,
    pub single_p95: f64,
    pub batches: Option<Batches>,
}

/// What `frames_at` times, for `batched`: the layer, its project, the size, at half size, the limit its p95 is surely
/// over by, and whether the size is the preview's.
pub(super) struct At<'l> {
    pub layer: &'l DraftLayer,
    pub project: Arc<Project>,
    pub size: (u32, u32),
    pub half: bool,
    pub over: Option<f64>,
    pub preview: bool,
}

/// A point's class by time, and the figure that decided it: its half-size time where it ran there, else its
/// full-size time. One that may run at half size and was over Normal at full size is decided at half size even when its
/// half-size frames stopped at a frame over `OVER_AT_PREVIEW_MS` (`half` None: the time's cut is the caller's), at that
/// stop: never by a full-size time within Heavy's limit.
pub(super) fn decide(
    full: Pcts,
    half: Option<Pcts>,
    may_halve: bool,
    half_always: bool,
    b: &Budget,
) -> (CostClass, Decided) {
    let class = class_of_time(full.p95, half.map(|h| h.p95), may_halve, b);
    let at_half = half_always || (may_halve && full.p95 > b.normal_ms);
    let decided = match half {
        Some(h) if at_half => Decided::Half(h.p95),
        None if at_half => Decided::Half(OVER_AT_PREVIEW_MS),
        _ => Decided::Preview(full.p95),
    };
    (class, decided)
}

/// A point that is Over without a measured time: `ms` is what stopped it (a step's, or the first frame over
/// `OVER_AT_PREVIEW_MS`), infinite when the time or a cap cut it before it started. `half`: its frames are half-size
/// ones (a `resolution: half` generator's).
pub(super) fn over(params: Params, ms: f64, half: bool) -> Point {
    Point {
        params,
        preview: Pcts::flat(ms),
        single_p95: ms,
        within: false,
        class: CostClass::Over,
        decided: match (ms.is_finite(), half) {
            (false, _) => Decided::Unmeasured,
            (true, true) => Decided::Half(ms),
            (true, false) => Decided::Preview(ms),
        },
        peak: 0,
        batches: None,
        half_batches: None,
        held: false,
    }
}

#[cfg(test)]
mod tests;
