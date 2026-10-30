//! Measuring a draft layer: a meter times one frame (`Meter`: `GpuMeter` on the worker's device,
//! `FakeMeter` in CI), and `measure` bounds the layer's cost with its settings at both ends, at probes, inside the range
//! of a number costly at both ends, and at every bool and choice value (`run`), each combination stepped first
//! (`steps`). What it returns, `Measured`, carries the class by time, decided per point with the worst point deciding
//! (`budget::class_of_time` on each point), and the worst figures measured. With a combination cut (the time or a cap),
//! those figures leave out what wasn't measured, so a reader takes the class from `Measured::class` and never from the
//! figures; `budget::classify` adds memory, CPU and advice, which can only raise it.
//! Where the GPU's timestamps aren't trusted, frames are timed by fences, in batches whose slope cancels the wait
//! (`run/batched.rs`); `Measured::fenced` keeps the batches.

mod deadline;
mod fake;
mod fenced;
mod gpu;
mod run;

use std::sync::Arc;

use futures::future::BoxFuture;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};

pub use deadline::{COMPILE_BUDGET, Deadline, MEASURE_BUDGET};
pub use fake::{FakeMeter, Metered};
pub use fenced::{Batch, Batches, Fenced};
pub use gpu::GpuMeter;
pub use run::{measure, measure_noted};

use super::draft::DraftLayer;
use super::envelope::Envelope;
use crate::headless::Headless;
use crate::make::budget::CostClass;

/// One frame's cost.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub gpu_ms: f64,
    pub cpu_us: f64,
    /// The effect targets' bytes at their most while it drew.
    pub peak_bytes: u64,
    /// A batch whose copies didn't all encode the same passes: its time isn't K copies of one
    /// frame, so the meter doesn't take a slope from it.
    pub uneven: bool,
    /// The render passes it encoded: a single frame's, or a batch's first copy's (a batch is compared
    /// with the single frame it is differenced against). 0 where the meter doesn't count them.
    pub passes: u32,
}

/// One frame a meter times.
pub struct FrameReq {
    pub project: Arc<Project>,
    pub at_us: Us,
    pub size: (u32, u32),
    pub format: FormatKind,
    /// The layer it draws, at its settings (a fake meter reads them).
    pub layer: DraftLayer,
    /// The same project without the pass.
    pub baseline: bool,
    /// The measured copy at half size (`DraftLayer::prepare`).
    pub half: bool,
}

impl FrameReq {
    pub fn new(project: Arc<Project>, layer: &DraftLayer, at_us: Us, size: (u32, u32)) -> Self {
        Self {
            project,
            at_us,
            size,
            format: layer.format,
            layer: layer.clone(),
            baseline: false,
            half: false,
        }
    }
}

/// Times frames.
pub trait Meter: Send {
    fn frame<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
    ) -> BoxFuture<'a, Result<Sample, String>>;

    /// Draws `req` once untimed, so the next timed frame at its size doesn't pay for compiling pipelines or making
    /// targets (`steps`): once per size per request. Nothing by default (`FakeMeter` draws nothing).
    fn warm<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
    ) -> BoxFuture<'a, Result<(), String>> {
        let _ = (h, req);
        Box::pin(async { Ok(()) })
    }

    /// `frames` copies of `req` in one submission with one wait: the batch's whole time in `gpu_ms`,
    /// what a fenced meter's slope is taken from.
    fn batch<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
        frames: u32,
    ) -> BoxFuture<'a, Result<Sample, String>>;

    /// A new request starts: every size is cold again.
    fn begin(&mut self) {}

    /// A scene's CPU time a frame (µs) as this meter reports it: what was timed (`GpuMeter`), or the fake
    /// meter's fixed figure, so CI's classes don't follow a loaded machine's clock.
    fn scene_cpu(&self, timed: Pcts) -> Pcts {
        timed
    }

    /// The first (cold) frame at each size and scale this request warmed: what a first frame costs while its pipeline
    /// compiles.
    fn cold(&self) -> Vec<Cold> {
        Vec::new()
    }

    /// Whether its frames are timed by fences rather than trusted GPU timestamps: `GpuMeter` on a device
    /// whose timestamps aren't trusted. Then the measurement times batches (`batch`). Not by default.
    fn fenced(&self, h: &Headless) -> bool {
        let _ = h;
        false
    }
}

/// A first frame drawn untimed (`Meter::warm`): its size, whether at half size, and its time in ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cold {
    pub size: (u32, u32),
    pub half: bool,
    pub ms: f64,
}

/// The 50th and 95th percentiles, in ms (or µs for CPU).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pcts {
    #[serde(deserialize_with = "crate::make::budget::figure")]
    pub p50: f64,
    #[serde(deserialize_with = "crate::make::budget::figure")]
    pub p95: f64,
}

impl Pcts {
    /// Of `values` (nearest rank); zeros for none.
    pub fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let at =
            |q: f64| sorted[((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1];
        Self {
            p50: at(0.5),
            p95: at(0.95),
        }
    }

    /// Both at `ms` (a single sample, or a prediction).
    pub fn flat(ms: f64) -> Self {
        Self { p50: ms, p95: ms }
    }

    /// The worse of two (by p95).
    pub fn worse(self, other: Self) -> Self {
        if other.p95 > self.p95 { other } else { self }
    }
}

/// The layer's GPU memory while drawn alone: its own share per kind and what is extra.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryUse {
    pub own: u64,
    /// Layer-sized targets beyond its own (offscreen groups, backdrops, half-size copies).
    pub extra_targets: u32,
    pub extra_bytes: u64,
    pub peak: u64,
}

/// What a measurement found: the class by time and the figure that decided it, the worst measured at preview size and
/// at 1080p at the worst point within class (2160p when asked), the envelope itself, and the worst measured anywhere.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// Decided per point, then the worst point decides. Over whenever a combination the envelope
    /// covers wasn't measured (`Decided::Unmeasured`).
    pub class: CostClass,
    /// The figure that decided it: the worst point's own.
    pub decided: Decided,
    /// The worst measured at preview size: with a combination cut, what wasn't measured isn't in it, so it never
    /// decides a class.
    pub preview: Pcts,
    pub p1080: Pcts,
    pub p2160: Option<f64>,
    pub cpu_us: Option<Pcts>,
    pub bytes: MemoryUse,
    pub envelope: Envelope,
    pub worst: Pcts,
    /// Timed by fences, in batches: what they measured, and whether they resolved it.
    pub fenced: Option<Fenced>,
}

/// The figure that decided a class: a point's own, and so the worst point's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Decided {
    /// Its p95 at preview size, in ms: what decides a point within Normal, or one with no half-size time.
    Preview(f64),
    /// Its p95 at half size, in ms: a definition that may run there and was over Normal at full size, or a
    /// `resolution: half` generator (its frames are all at half size).
    Half(f64),
    /// The time or a cap cut it before it was measured.
    #[default]
    Unmeasured,
}

impl Decided {
    /// The figure, for ranking: an unmeasured one ranks above any measured one.
    pub fn ms(self) -> f64 {
        match self {
            Self::Preview(ms) | Self::Half(ms) => ms,
            Self::Unmeasured => f64::INFINITY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Pcts;

    /// Nearest-rank percentiles, whatever the order; zeros for none.
    #[test]
    fn percentiles_by_nearest_rank() {
        assert_eq!(Pcts::of(&[]), Pcts::default());
        assert_eq!(Pcts::of(&[5.0]), Pcts::flat(5.0));
        let mut v: Vec<f64> = (1..=20).map(f64::from).rev().collect();
        assert_eq!(
            Pcts::of(&v),
            Pcts {
                p50: 10.0,
                p95: 19.0
            }
        );
        v = (1..=100).map(f64::from).collect();
        assert_eq!(
            Pcts::of(&v),
            Pcts {
                p50: 50.0,
                p95: 95.0
            }
        );
    }
}
