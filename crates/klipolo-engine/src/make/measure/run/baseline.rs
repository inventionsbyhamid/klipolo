//! What a layer's frames are measured less of: its draft's baseline at a size, the same project
//! without the pass, measured once per size a request. Where the timing is trusted, the median of 8 single frames.
//! Where it is fenced, timed in batches as the layer is (`batched`), always at the full span: the median slope, which
//! carries the baseline's own work a frame and none of the wait's, and whether its slopes, a steady workload's, agree as
//! closely as their resolution allows (`steady`).

use std::sync::Arc;

use super::batched::{FENCED_TIMES, FULL_SPAN, JITTER_MS, MIN_SPAN, Timing};
use super::{BASELINE_FRAMES, Run};
use crate::make::measure::{Batch, FrameReq, Pcts};

/// A size's baseline.
#[derive(Clone, Debug)]
pub(super) struct Base {
    /// Its time a frame, in ms: a single frame's median (trusted), or its batches' median slope (fenced).
    pub ms: f64,
    /// Its single frames' median, in ms (the same as `ms` where trusted).
    pub single_ms: f64,
    pub peak: u64,
    /// Fenced: its sample times' batches.
    pub batches: Option<Vec<Batch>>,
    /// Fenced: every batch reached Light's precision, and its slopes agree within twice their resolution plus
    /// `JITTER_MS`. A steady workload whose slopes don't shows timing noisier than the batches resolve. Always where
    /// trusted.
    pub steady: bool,
    /// Fenced: how far its slope can be off, the worst of its batches' (`Batch::resolution`, each from its own span).
    /// 0 where trusted.
    pub res: f64,
}

impl Run<'_> {
    /// The baseline at `size` (measured once per size a request). None: the time ran out while it was measured, and
    /// nothing is kept.
    pub(super) async fn baseline(&mut self, size: (u32, u32)) -> Result<Option<Base>, String> {
        if let Some(b) = self.baselines.get(&size) {
            return Ok(Some(b.clone()));
        }
        self.draft.prepare(self.h, true, false);
        let project = Arc::new(self.draft.baseline());
        let base = if self.fenced {
            let mut batches = Vec::with_capacity(FENCED_TIMES);
            let mut peak = 0;
            for at in self.draft.times(FENCED_TIMES) {
                if self.out_of_time() {
                    return Ok(None);
                }
                let mut req = FrameReq::new(project.clone(), self.draft, at, size);
                req.baseline = true;
                let timing = self
                    .time_at(&req, f64::INFINITY, FULL_SPAN)
                    .await
                    .map_err(|e| format!("the baseline at {}×{}: {e}", size.0, size.1))?;
                if let Timing::Sampled(b, p) = timing {
                    batches.push(b);
                    peak = peak.max(p);
                }
            }
            fenced_base(batches, peak, self.budget.light_ms)
        } else {
            let (mut ms, mut peak) = (Vec::new(), 0);
            for at in self.draft.times(BASELINE_FRAMES) {
                let mut req = FrameReq::new(project.clone(), self.draft, at, size);
                req.baseline = true;
                let s = self
                    .meter
                    .frame(self.h, &req)
                    .await
                    .map_err(|e| format!("the baseline at {}×{}: {e}", size.0, size.1))?;
                ms.push(s.gpu_ms);
                peak = peak.max(s.peak_bytes);
            }
            let median = Pcts::of(&ms).p50;
            Base {
                ms: median,
                single_ms: median,
                peak,
                batches: None,
                steady: true,
                res: 0.0,
            }
        };
        self.baselines.insert(size, base.clone());
        Ok(Some(base))
    }
}

/// A fenced baseline from its batches: the median slope (never below 0), the worst resolution, and whether it is
/// steady: every batch at Light's precision (its resolution at most half `light_ms`), and the slopes within twice that
/// resolution plus `JITTER_MS` of each other (the precision reached, not an exact span).
pub(super) fn fenced_base(batches: Vec<Batch>, peak: u64, light_ms: f64) -> Base {
    let slopes: Vec<f64> = batches.iter().map(Batch::per_frame).collect();
    let singles: Vec<f64> = batches.iter().map(|b| b.single_ms).collect();
    let res = batches
        .iter()
        .map(Batch::resolution)
        .fold(0.0_f64, f64::max);
    let (low, high) = slopes
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &s| {
            (l.min(s), h.max(s))
        });
    let steady = !batches.is_empty()
        && batches.iter().all(|b| b.span() >= MIN_SPAN)
        && res <= light_ms / 2.0
        && high - low <= 2.0 * res + JITTER_MS;
    Base {
        ms: Pcts::of(&slopes).p50.max(0.0),
        single_ms: Pcts::of(&singles).p50,
        peak,
        batches: Some(batches),
        steady,
        res,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHT: f64 = 0.5;

    fn batch(frames: u32, single_ms: f64, batch_ms: f64) -> Batch {
        Batch {
            frames,
            single_ms,
            batch_ms,
        }
    }

    /// A steady baseline's slopes agree within twice their resolution plus the jitter; one stalled batch (5 ms over 16
    /// frames, 0.31 ms of slope) is past it, as is a single frame alone.
    #[test]
    fn a_baseline_is_steady_only_when_its_slopes_agree() {
        let steady = fenced_base(vec![batch(17, 1.0, 1.0); 8], 0, LIGHT);
        assert!(steady.steady && steady.ms == 0.0, "{steady:?}");
        let mut stalled = vec![batch(17, 1.0, 1.0); 8];
        stalled[3] = batch(17, 1.0, 6.0);
        let stalled = fenced_base(stalled, 0, LIGHT);
        assert!(!stalled.steady && stalled.ms == 0.0, "{stalled:?}");
        let mut short = vec![batch(17, 1.0, 2.0); 8];
        short[0] = Batch::single(1.0);
        assert!(!fenced_base(short, 0, LIGHT).steady);
        assert!(
            !fenced_base(Vec::new(), 0, LIGHT).steady,
            "nothing measured is never steady"
        );
    }

    /// Iris-like 3 ms singles allow 16 copies (a span of 15), whose resolution (0.2 ms) is within
    /// Light's precision: steady. Singles of 5 ms (a span of 9, 0.56 ms) aren't.
    #[test]
    fn a_baseline_is_steady_on_the_precision_it_reached() {
        let iris = fenced_base(vec![batch(16, 3.0, 3.0); 8], 0, LIGHT);
        assert!(iris.steady && (iris.res - 0.2).abs() < 1e-9, "{iris:?}");
        let coarse = fenced_base(vec![batch(10, 5.0, 5.0); 8], 0, LIGHT);
        assert!(!coarse.steady, "{coarse:?}");
    }
}
