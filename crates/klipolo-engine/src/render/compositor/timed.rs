//! The render worker's frames: drawn without a background when it asks (`set_transparent`), with the
//! worker's own fonts (`set_fonts`), and timed for its meter (`render_timed`): the frame drawn into the session texture
//! and submitted with nothing read back, its GPU time from the timestamps of its first and last render passes
//! (`FrameTimer`) when the device has trusted ones and the reading is plausible (`gpu_time`), else the wall clock from
//! its submission to its completion. Where the timestamps aren't trusted, the meter also times batches
//! (`render_batch`): copies of the frame in one submission with one wait, whose difference cancels the
//! wait's own cost.

use std::time::Instant;

use super::Compositor;
use super::frame::FrameTimer;
use crate::plan::FramePlan;
use crate::render::device::WorkDone;
use crate::render::pool::DecodedSet;
use crate::render::shared_fonts::SharedFonts;

/// What a timed frame that the graphics card failed answers (the worker names the step and size around it).
const TIMED_FRAME_FAILED: &str = "the graphics card failed while drawing a timed frame";

/// The frame's GPU time from its timestamp `reading` when it is plausible: finite, above 0 and no longer than the
/// fenced wall clock around the same submission. Anything else (a sample the driver wrote as 0, an error value, a
/// stage-boundary sample that landed early or late) gives the wall clock, which can't read short.
/// Whether the timestamps were used.
pub(crate) fn gpu_time(reading: Option<f64>, wall_ms: f64) -> (f64, bool) {
    match reading {
        Some(t) if t.is_finite() && t > 0.0 && t <= wall_ms => (t, true),
        _ => (wall_ms, false),
    }
}

/// One timed frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameTime {
    /// The frame's GPU time.
    pub gpu_ms: f64,
    /// The CPU time spent encoding it.
    pub cpu_us: f64,
    /// Whether `gpu_ms` came from timestamps (else from the wall clock around one submission).
    pub timestamps: bool,
    pub warnings: Vec<String>,
    /// The render passes each copy encoded (one for a single frame): a batch's copies must match.
    pub passes: Vec<u32>,
    /// The effect stage's uniform slots each copy wrote (one for a single frame). A batch's writes all land before its
    /// one submission, so every copy draws with the last ones written: a copy after the first writes none, as every
    /// copy is the same plan's and draws with the uniforms the first wrote (the batch timing assumes
    /// identical uniforms).
    pub uniform_writes: Vec<u32>,
}

impl Compositor {
    /// No background: every frame starts cleared to transparent (the worker's draft layers drawn alone).
    pub(crate) fn set_transparent(&mut self, on: bool) {
        self.transparent = on;
    }

    /// The render worker's registry changed: other drafts' pipelines and kept pictures go.
    pub(crate) fn keep_shaders_of(&mut self, registry: &crate::registry::Registry) {
        self.effects.keep_shaders_of(registry);
    }

    /// The effect pipelines kept (tests).
    pub(crate) fn pipelines_kept(&self) -> usize {
        self.effects.pipelines_kept()
    }

    /// The font system texts are shaped with, made by the worker before its first frame.
    pub(crate) fn set_fonts(&mut self, fonts: SharedFonts) {
        self.fonts = Some(fonts);
        self.fonts_given = true;
        self.texts = None;
        self.captions = None;
    }

    /// Draws `plan` at `size` and submits it with nothing read back; resolves once the GPU has finished it.
    pub(crate) async fn render_timed(
        &mut self,
        plan: &FramePlan,
        frames: &DecodedSet,
        size: (u32, u32),
        fail_read: bool,
    ) -> Result<FrameTime, String> {
        let dev = self.dev.clone();
        if self.timer.is_none() {
            self.timer = FrameTimer::new(&dev);
        }
        let target = self.session_view(size);
        self.timing = true;
        self.effects.set_fresh(true);
        let encoding = Instant::now();
        let (encoder, warnings, _missing) = self.draw(plan, frames, size, &target).await;
        self.effects.set_fresh(false);
        self.timing = false;
        let cpu_us = encoding.elapsed().as_secs_f64() * 1e6;
        let submitted = Instant::now();
        dev.queue.submit([encoder.finish()]);
        let done = std::sync::Arc::new(WorkDone::default());
        done.arm(&dev);
        done.wait().await;
        let wall_ms = submitted.elapsed().as_secs_f64() * 1000.0;
        if dev.take_fault().is_some() || dev.is_lost() {
            return Err(TIMED_FRAME_FAILED.into());
        }
        // A timer whose reading failed is made again for the next frame: its buffer may still be mapping.
        let reading = match &self.timer {
            Some(timer) => match timer.read(&dev, fail_read).await {
                Ok(ms) => Some(ms),
                Err(e) => {
                    tracing::warn!(target: "make", "a frame's timestamps: {e}");
                    self.timer = None;
                    None
                }
            },
            None => None,
        };
        let (gpu_ms, timestamps) = gpu_time(reading, wall_ms);
        Ok(FrameTime {
            gpu_ms,
            cpu_us,
            timestamps,
            warnings,
            passes: vec![self.passes_last],
            uniform_writes: vec![self.effects.uniform_writes()],
        })
    }
}

impl Compositor {
    /// `frames` copies of `plan` drawn at `size` (at least one) and submitted together, as that many command buffers in
    /// one submission with one wait; resolves once the GPU has finished them all. Its time is the wall clock from the
    /// submission to its completion, and its CPU time the encoding's, a frame (no timestamps: what it measures is the
    /// fenced wait, which a short batch's time cancels). No copy reuses a kept picture, and each copy's passes are kept
    /// (`FrameTime::passes`), so the meter can check every copy drew what the first did.
    pub(crate) async fn render_batch(
        &mut self,
        plan: &FramePlan,
        frames: &DecodedSet,
        size: (u32, u32),
        copies: u32,
    ) -> Result<FrameTime, String> {
        let dev = self.dev.clone();
        let target = self.session_view(size);
        let copies = copies.max(1);
        let encoding = Instant::now();
        let mut buffers = Vec::with_capacity(copies as usize);
        let mut passes = Vec::with_capacity(copies as usize);
        let mut uniform_writes = Vec::with_capacity(copies as usize);
        let mut warnings = Vec::new();
        self.effects.set_fresh(true);
        for _ in 0..copies {
            let (encoder, said, _missing) = self.draw(plan, frames, size, &target).await;
            warnings = said;
            passes.push(self.passes_last);
            uniform_writes.push(self.effects.uniform_writes());
            buffers.push(encoder.finish());
        }
        self.effects.set_fresh(false);
        let cpu_us = encoding.elapsed().as_secs_f64() * 1e6 / f64::from(copies);
        let submitted = Instant::now();
        dev.queue.submit(buffers);
        let done = std::sync::Arc::new(WorkDone::default());
        done.arm(&dev);
        done.wait().await;
        let wall_ms = submitted.elapsed().as_secs_f64() * 1000.0;
        if dev.take_fault().is_some() || dev.is_lost() {
            return Err(TIMED_FRAME_FAILED.into());
        }
        Ok(FrameTime {
            gpu_ms: wall_ms,
            cpu_us,
            timestamps: false,
            warnings,
            passes,
            uniform_writes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::gpu_time;

    #[test]
    fn a_timestamp_reading_that_isnt_plausible_gives_way_to_the_wall_clock() {
        assert_eq!(gpu_time(Some(1.5), 2.0), (1.5, true));
        assert_eq!(
            gpu_time(Some(0.0), 2.0),
            (2.0, false),
            "a sample written as 0"
        );
        assert_eq!(gpu_time(Some(f64::NAN), 2.0), (2.0, false));
        assert_eq!(gpu_time(Some(-1.0), 2.0), (2.0, false));
        assert_eq!(
            gpu_time(Some(3.0), 2.0),
            (2.0, false),
            "longer than the fenced wall clock"
        );
        assert_eq!(
            gpu_time(None, 2.0),
            (2.0, false),
            "no timer, or untrusted (Metal)"
        );
    }
}
