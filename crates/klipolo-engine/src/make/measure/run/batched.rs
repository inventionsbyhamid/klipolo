//! Fenced timing. Where the GPU's timestamps aren't trusted, a frame is timed from
//! its submission to its completion, and that time is quantised: on Metal wgpu's fence wait sleeps 1 ms between polls,
//! so work that ends within one poll step adds nothing to a single frame. So at each sample time the meter times a
//! single frame and a batch of copies in one submission with one wait (`Meter::batch`), and the cost a frame is the
//! slope between them, `(T(K) − T(1)) / (K − 1)`: every fixed cost (the submission, the poller's wake-up, the wait's
//! last step) is in both and cancels, and what is left of the quantisation is under one step over the span (`Batch`).
//!
//! - The span `K − 1` a sample needs is `FULL_SPAN` (16) while the point could still be Light, and `SHORT_SPAN` (6)
//!   once a sample's cost is over Light's limit by more than twice its resolution: then the point can't be Light, and
//!   a short batch tells Normal, Heavy and Over apart (its error is under a poll step over 6, ~0.25 ms on Iris Plus)
//!   at about half the time, so a draft at the setting caps fits the compile budget.
//! - Fewer when the copies, each at most the single frame's time, would keep the GPU busy past `BATCH_LIMIT_MS` (the
//!   step limit). A single frame read too slow for the span it needs is drawn once more and the faster kept (still no
//!   less than its GPU time). Not even `MIN_SPAN` fitting, or a batch whose copies didn't encode the same passes, the
//!   single frame alone is the figure: an upper bound.
//! - Each slope less the baseline's (`baseline`, timed the same way) is the sample's cost; their p50 and p95 across the
//!   sample times (8, a probe's 4) are the figures, so a cost that changes with time keeps its p95.
//! - Resolved, when the batches tell whether the point is Light: a sample was over Light by more than
//!   twice its resolution (so it isn't), or every sample reached Light's precision (a resolution, the single frame's
//!   time over the span, of at most half Light's limit: 0.25 ms), none read below the baseline by more than both
//!   resolutions, and the baseline is steady. A point that reads Light unresolved counts as Normal, never Light
//!   (`Point::held`).
//! - The single frames keep the stops: one over `OVER_AT_PREVIEW_MS` at preview size ends the point as Over, one over
//!   the step limit at an export size ends that size's frames.
//!
//! The cost on Iris Plus (~1.5 ms poll steps after ~1.5 ms of fixed cost: ~3 ms singles): a sample at the full span is
//! a single frame and a batch of about 16, ~25 ms for a light body; a short one a single frame and a batch of 7,
//! ~15–20 ms for a 1.5 ms body. So a point is ~0.2 s while it could be Light and ~0.15 s once it can't (a probe half),
//! and each size's baseline ~0.2 s, once a request.

use super::point::{At, Timed, over_count_for_p95};
use super::{FRAMES, Run};
use crate::make::budget::{OVER_AT_PREVIEW_MS, STEP_LIMIT_MS};
use crate::make::measure::{Batch, Batches, FrameReq, Pcts};

/// Frames in the long batch beyond the single one when they fit (K − K₀ ≥ 16), while a point could be
/// Light.
pub(super) const FULL_SPAN: u32 = 16;
/// Once a point can't be Light: enough to tell Normal, Heavy and Over apart.
pub(super) const SHORT_SPAN: u32 = 6;
/// Fewer than this beyond the single frame, and a batch can't tell its frames from the wait: the single frame alone.
pub(super) const MIN_SPAN: u32 = 4;
/// The most a batch may keep the GPU busy, predicted from its single frame: a single frame's step limit.
pub(super) const BATCH_LIMIT_MS: f64 = STEP_LIMIT_MS;
/// Sample times at a size, and a probe's.
pub(super) const FENCED_TIMES: usize = 8;
pub(super) const FENCED_PROBE_TIMES: usize = 4;
/// How far a steady workload's slopes may spread past twice their resolution: the poller's wake-up jitter, a few tenths
/// of a ms a submission, over the span, with room to spare. Past it the timing is noisier than the batches resolve.
pub(super) const JITTER_MS: f64 = 0.1;

/// One sample time, timed.
pub(super) enum Timing {
    /// Its batch (or single frame), and the peak they reported.
    Sampled(Batch, u64),
    /// Its single frame took this long, past the stop it was given.
    Stopped(f64),
}

/// The span a single frame of `single_ms` allows: as many copies as fit `BATCH_LIMIT_MS` at that time each (a frame's
/// wall time is never less than its GPU time), less the single one, at most `FULL_SPAN`.
pub(super) fn span_for(single_ms: f64) -> u32 {
    let fits = (BATCH_LIMIT_MS / single_ms.max(1e-6))
        .floor()
        .min(f64::from(FULL_SPAN + 1));
    (fits as u32).saturating_sub(1)
}

impl Run<'_> {
    /// `req` timed at its time: a single frame (again, the faster kept, when it reads too slow for the `needed` span),
    /// then a batch at the span it allows, at most `needed`, or the single frame alone. A single frame over `stop` ends
    /// it.
    pub(super) async fn time_at(
        &mut self,
        req: &FrameReq,
        stop: f64,
        needed: u32,
    ) -> Result<Timing, String> {
        let first = self.meter.frame(self.h, req).await?;
        if first.gpu_ms > stop {
            return Ok(Timing::Stopped(first.gpu_ms));
        }
        let (mut single, mut peak) = (first.gpu_ms, first.peak_bytes);
        if span_for(single) < needed {
            let again = self.meter.frame(self.h, req).await?;
            single = single.min(again.gpu_ms);
            peak = peak.max(again.peak_bytes);
        }
        let span = span_for(single).min(needed);
        if span < MIN_SPAN {
            return Ok(Timing::Sampled(Batch::single(single), peak));
        }
        let batch = self.meter.batch(self.h, req, span + 1).await?;
        if batch.uneven || batch.passes != first.passes {
            // Its copies didn't all draw the same passes as each other, or as the single frame they are differenced
            // against: no slope from it.
            tracing::warn!(target: "make", "a batch's copies drew other passes: timed as a single frame");
            return Ok(Timing::Sampled(Batch::single(single), peak));
        }
        let b = Batch {
            frames: span + 1,
            single_ms: single,
            batch_ms: batch.gpu_ms,
        };
        Ok(Timing::Sampled(b, peak.max(batch.peak_bytes)))
    }

    /// `at`'s frames timed in batches (fenced): `n` is what a trusted meter would time (fewer than `FRAMES` for a
    /// probe). None at a stop at preview size, or when the time ran out (counted as the caller's cut).
    pub(super) async fn batched(
        &mut self,
        at: &At<'_>,
        n: usize,
        base: &super::baseline::Base,
    ) -> Result<Option<Timed>, String> {
        let times = if n < FRAMES {
            FENCED_PROBE_TIMES
        } else {
            FENCED_TIMES
        };
        let stop = if at.preview {
            OVER_AT_PREVIEW_MS
        } else {
            STEP_LIMIT_MS
        };
        let light = self.budget.light_ms;
        let (mut samples, mut costs, mut singles) = (Vec::new(), Vec::new(), Vec::new());
        let (mut peak, mut above, mut needed, mut over_light) = (0, 0, FULL_SPAN, false);
        for t in at.layer.times(times) {
            if self.out_of_time() {
                self.timed_out = true;
                return Ok(None);
            }
            let mut req = FrameReq::new(at.project.clone(), at.layer, t, at.size);
            req.half = at.half;
            let timing = self
                .time_at(&req, stop, needed)
                .await
                .map_err(|e| format!("at {}×{}: {e}", at.size.0, at.size.1))?;
            let (batch, stopped) = match timing {
                Timing::Stopped(_) if at.preview => return Ok(None),
                Timing::Stopped(ms) => (Batch::single(ms), true),
                Timing::Sampled(b, p) => {
                    peak = peak.max(p);
                    (b, false)
                }
            };
            let cost = batch.per_frame() - base.ms;
            // Over Light by more than twice its resolution, the point isn't Light: short batches from here.
            if cost - 2.0 * batch.resolution() > light {
                over_light = true;
                needed = SHORT_SPAN;
            }
            costs.push(cost);
            singles.push((batch.single_ms - base.single_ms).max(0.0));
            samples.push(batch);
            above += usize::from(at.over.is_some_and(|limit| cost > limit));
            if stopped || above >= over_count_for_p95(times) {
                break;
            }
        }
        // On the precision reached, not on an exact span; a cost below the baseline's by more than both
        // resolutions is noise the batches don't resolve.
        let resolved = over_light
            || (base.steady
                && samples.iter().zip(&costs).all(|(b, &cost)| {
                    let res = b.resolution();
                    b.span() >= MIN_SPAN && res <= light / 2.0 && cost >= -(res + base.res)
                }));
        let figures: Vec<f64> = costs.iter().map(|c| c.max(0.0)).collect();
        Ok(Some(Timed {
            ms: Pcts::of(&figures),
            peak,
            single_p95: Pcts::of(&singles).p95,
            batches: Some(Batches {
                samples,
                baseline: base.batches.clone().unwrap_or_default(),
                resolved,
                baseline_steady: base.steady,
                over_light,
            }),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single frame's time sets the span: up to 16 while 17 copies of it fit 50 ms, fewer past that, none below 4.
    #[test]
    fn the_span_keeps_a_batch_within_the_step_limit() {
        assert_eq!(span_for(0.0), FULL_SPAN);
        assert_eq!(span_for(1.0), FULL_SPAN);
        assert_eq!(span_for(2.9), FULL_SPAN, "17 copies of 2.9 ms fit 50 ms");
        assert_eq!(span_for(4.0), 11, "12 copies of 4 ms fit 50 ms");
        assert_eq!(span_for(7.0), 6);
        assert!(span_for(12.0) < MIN_SPAN);
        for ms in [0.5, 2.9, 3.0, 6.0, 9.9] {
            let copies = span_for(ms) + 1;
            assert!(
                f64::from(copies) * ms <= BATCH_LIMIT_MS,
                "{ms} ms: {copies} copies"
            );
        }
    }
}
