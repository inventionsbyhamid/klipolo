//! The worker's meter: the frame's GPU time from `Headless::render_timed` (its first and last
//! render passes' timestamps when trusted and plausible, else the wall clock around its one submission and its
//! completion); the effect targets' peak while it drew. It warms each size once per request and keeps that cold frame's
//! time. On a device whose timestamps aren't trusted its frames are fenced (`Meter::fenced`), and the measurement times
//! batches too (`Headless::render_batch`).

use futures::future::BoxFuture;

use super::{Cold, FrameReq, Meter, Sample};
use crate::headless::Headless;
use crate::render::effects::{effect_target_peak, reset_effect_target_peak};

#[derive(Default)]
pub struct GpuMeter {
    cold: Vec<((u32, u32, bool), f64)>,
    /// Fenced on any device (tests: the batched path on lavapipe, whose timestamps are trusted).
    always_fenced: bool,
}

impl GpuMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// A meter that times as fenced even where the timestamps are trusted, so lavapipe draws the batched path on a real
    /// device. Only in builds with the `test-support` feature.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn always_fenced() -> Self {
        Self {
            always_fenced: true,
            ..Self::default()
        }
    }
}

impl Meter for GpuMeter {
    fn frame<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
    ) -> BoxFuture<'a, Result<Sample, String>> {
        Box::pin(async move {
            reset_effect_target_peak();
            let t = h
                .render_timed(req.project.clone(), req.at_us, req.format, req.size)
                .await?;
            Ok(Sample {
                gpu_ms: t.gpu_ms,
                cpu_us: t.cpu_us,
                peak_bytes: effect_target_peak(),
                uneven: false,
                passes: t.passes.first().copied().unwrap_or(0),
            })
        })
    }

    fn batch<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
        frames: u32,
    ) -> BoxFuture<'a, Result<Sample, String>> {
        Box::pin(async move {
            reset_effect_target_peak();
            let t = h
                .render_batch(req.project.clone(), req.at_us, req.format, req.size, frames)
                .await?;
            Ok(Sample {
                gpu_ms: t.gpu_ms,
                cpu_us: t.cpu_us,
                peak_bytes: effect_target_peak(),
                uneven: t.passes.windows(2).any(|w| w[0] != w[1]),
                passes: t.passes.first().copied().unwrap_or(0),
            })
        })
    }

    fn warm<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let key = (req.size.0, req.size.1, req.half);
            if self.cold.iter().any(|(k, _)| *k == key) {
                return Ok(());
            }
            let t = h
                .render_timed(req.project.clone(), req.at_us, req.format, req.size)
                .await?;
            self.cold.push((key, t.gpu_ms));
            Ok(())
        })
    }

    fn begin(&mut self) {
        self.cold.clear();
    }

    fn cold(&self) -> Vec<Cold> {
        self.cold
            .iter()
            .map(|&((w, h, half), ms)| Cold {
                size: (w, h),
                half,
                ms,
            })
            .collect()
    }

    fn fenced(&self, h: &Headless) -> bool {
        self.always_fenced || !h.device().timestamps_trusted()
    }
}
