//! What fenced timing measured. Where the GPU's timestamps aren't trusted, every figure is a
//! slope: at each sample time a single frame and a batch of copies in one submission, each timed from its submission to
//! its completion (`run/batched.rs`). These are those times, kept with the cost so a later trusted run can compare.

use serde::{Deserialize, Serialize};

/// One sample time's two submissions, in ms: a single frame, and a batch of `frames` copies (0: too slow to batch, the
/// single frame alone).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Batch {
    pub frames: u32,
    #[serde(deserialize_with = "crate::make::budget::figure")]
    pub single_ms: f64,
    #[serde(deserialize_with = "crate::make::budget::figure")]
    pub batch_ms: f64,
}

impl Batch {
    /// The single frame alone: what a size too slow to batch is timed with.
    pub fn single(ms: f64) -> Self {
        Self {
            frames: 0,
            single_ms: ms,
            batch_ms: ms,
        }
    }

    /// Frames in the batch beyond the single one (0 when it wasn't batched).
    pub fn span(&self) -> u32 {
        self.frames.saturating_sub(1)
    }

    /// Its time a frame: the slope `(batch − single) / span`, in which every fixed cost cancels; or, not batched, the
    /// single frame's whole time, which is never less than its GPU time.
    pub fn per_frame(&self) -> f64 {
        match self.span() {
            0 => self.single_ms,
            span => (self.batch_ms - self.single_ms) / f64::from(span),
        }
    }

    /// How far its slope can be off: each submission is seen done at the first poll after it ends, so late by under one
    /// poll step, and a step is no longer than the single frame's whole time; over the span, that is the single frame's
    /// time / span. Not batched: its whole time.
    pub fn resolution(&self) -> f64 {
        match self.span() {
            0 => self.single_ms,
            span => self.single_ms / f64::from(span),
        }
    }
}

/// What was timed at one size: the layer's sample times, and its baseline's (the same project without the pass).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Batches {
    pub samples: Vec<Batch>,
    pub baseline: Vec<Batch>,
    /// Whether they told whether the cost is Light (`run/batched.rs`).
    pub resolved: bool,
    /// Whether the baseline's slopes were steady.
    pub baseline_steady: bool,
    /// Settled because a sample was over Light by more than twice its resolution, not by Light's precision: `resolved`
    /// then says only that the point isn't Light.
    #[serde(default)]
    pub over_light: bool,
}

/// A measurement timed in batches by fences: whether it resolved every figure that decides Light, whether
/// a point that read Light was held at Normal because it didn't, and the deciding point's batches at preview size and at
/// half size, with 1080p's and 2160p's where they were measured.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fenced {
    pub resolved: bool,
    pub held: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Batches>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub half: Option<Batches>,
    #[serde(default, rename = "1080p", skip_serializing_if = "Option::is_none")]
    pub p1080: Option<Batches>,
    #[serde(default, rename = "2160p", skip_serializing_if = "Option::is_none")]
    pub p2160: Option<Batches>,
}

impl Fenced {
    /// Every figure, for `Cost::fail_closed`.
    pub fn figures_mut(&mut self) -> Vec<&mut f64> {
        [
            &mut self.preview,
            &mut self.half,
            &mut self.p1080,
            &mut self.p2160,
        ]
        .into_iter()
        .flatten()
        .flat_map(|b| b.samples.iter_mut().chain(b.baseline.iter_mut()))
        .flat_map(|b| [&mut b.single_ms, &mut b.batch_ms])
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A batch's slope cancels what both submissions share, and its resolution is a single frame over the span.
    #[test]
    fn a_batch_reads_its_slope_and_resolution() {
        let b = Batch {
            frames: 17,
            single_ms: 1.0,
            batch_ms: 14.0,
        };
        assert_eq!(
            (b.span(), b.per_frame(), b.resolution()),
            (16, 0.8125, 0.0625)
        );
        let alone = Batch::single(4.0);
        assert_eq!(
            (alone.span(), alone.per_frame(), alone.resolution()),
            (0, 4.0, 4.0)
        );
    }
}
