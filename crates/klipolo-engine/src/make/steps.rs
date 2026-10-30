//! A measurement's safety steps: every setting combination is drawn first small, 16², 64²,
//! 256², then at preview size, each frame its own submission (each size warmed by one untimed frame first). Before
//! each size the time is predicted from the last (it scales with pixels), and a frame predicted over the limit (50 ms)
//! is never drawn; a preview-size frame over `OVER_AT_PREVIEW_MS` (4 × Heavy, 24 ms) ends the combination as Over,
//! with nothing more measured for it.

use super::budget::OVER_AT_PREVIEW_MS;
use super::draft::DraftLayer;
use super::measure::{FrameReq, Meter, Sample};
use crate::headless::Headless;

/// The sizes before the preview's.
pub const SMALL_SIZES: [(u32, u32); 3] = [(16, 16), (64, 64), (256, 256)];

#[derive(Clone, Debug, PartialEq)]
pub enum StepOutcome {
    /// One sample per size, the preview's last.
    Ok(Vec<Sample>),
    /// The frame at `at` was predicted over the limit, so it wasn't drawn.
    TooSlow { at: (u32, u32), predicted_ms: f64 },
    /// The preview-size frame at `at` took `ms`, over `OVER_AT_PREVIEW_MS`.
    Over { at: (u32, u32), ms: f64 },
    /// The GPU failed (the worker answers why).
    Failed(String),
}

impl StepOutcome {
    /// Its time at preview size, predicted when it was never drawn (what an Over corner reports).
    pub fn preview_ms(&self) -> Option<f64> {
        match self {
            Self::Ok(samples) => samples.last().map(|s| s.gpu_ms),
            Self::TooSlow { predicted_ms, .. } => Some(*predicted_ms),
            Self::Over { ms, .. } => Some(*ms),
            Self::Failed(_) => None,
        }
    }
}

/// `layer` at its own settings drawn at `sizes` in turn (the last is the preview's), at the middle of its time.
pub async fn stepped(
    meter: &mut dyn Meter,
    h: &mut Headless,
    layer: &DraftLayer,
    sizes: &[(u32, u32)],
    limit_ms: f64,
) -> StepOutcome {
    layer.prepare(h, true, false);
    let project = std::sync::Arc::new(layer.project());
    let mut samples = Vec::with_capacity(sizes.len());
    let mut last: Option<(f64, f64)> = None;
    for (i, &size) in sizes.iter().enumerate() {
        let pixels = f64::from(size.0) * f64::from(size.1);
        if let Some((ms, px)) = last {
            let predicted = ms * pixels / px;
            if predicted > limit_ms {
                return StepOutcome::TooSlow {
                    at: size,
                    predicted_ms: predicted,
                };
            }
        }
        let req = FrameReq::new(project.clone(), layer, layer.middle(), size);
        // Untimed first (`Meter::warm`): a size's first frame pays for its pipeline's compile and its targets, which
        // would inflate the sample and every prediction after it. Only a size the prediction allows is drawn at all.
        if let Err(e) = meter.warm(h, &req).await {
            return StepOutcome::Failed(e);
        }
        let sample = match meter.frame(h, &req).await {
            Ok(sample) => sample,
            Err(e) => return StepOutcome::Failed(e),
        };
        if i + 1 == sizes.len() && sample.gpu_ms > OVER_AT_PREVIEW_MS {
            return StepOutcome::Over {
                at: size,
                ms: sample.gpu_ms,
            };
        }
        last = Some((sample.gpu_ms.max(1e-6), pixels));
        samples.push(sample);
    }
    StepOutcome::Ok(samples)
}
