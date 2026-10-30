//! What this computer's check lets a contribution do: its cost, the
//! settings it was measured within (`envelope`) and how it runs (`runs_at`). The app's snapshot reads it from the
//! verified store for the preview; an export job carries exactly the verdicts the snapshot had (`export::contrib`), so
//! both draw a checked shader the same way: `ShaderEffect::verified`.

use std::sync::Arc;

use klipolo_model::contrib::Resolution;
use serde::{Deserialize, Serialize};

use crate::make::budget::{Cost, CostClass, RunsAt};
use crate::make::envelope::Envelope;
use crate::registry::{ShaderEffect, ShaderKind};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Verdict {
    pub cost: Cost,
    pub envelope: Envelope,
    pub runs_at: RunsAt,
}

impl Verdict {
    /// The verdict a stored cost gives: None for Over (a cut cost is Over, `Cost::fail_closed`). What `auto` resolved to
    /// at half size runs there whatever the class.
    pub fn of(cost: Cost) -> Option<Self> {
        let cost = cost.fail_closed();
        if cost.class == CostClass::Over {
            return None;
        }
        let mut runs_at = cost.runs_at;
        runs_at.half |= cost.resolved == Some(Resolution::Half);
        Some(Self {
            envelope: cost.envelope.clone(),
            runs_at,
            cost,
        })
    }
}

impl ShaderEffect {
    /// This checked shader as its verdict lets it run, the assembled text shared: its envelope, which
    /// `plan::effects::clamped` keeps its settings in; a generator at half size when the verdict runs it there, else its
    /// `auto` as the check resolved it (full until a check did); a filter through its `downsample` from any size when
    /// the verdict halves it, as it was measured.
    pub fn verified(&self, v: &Verdict) -> ShaderEffect {
        let mut shader = self.clone();
        shader.envelope = Some(Arc::new(v.envelope.clone()));
        shader.runs_at = v.runs_at;
        shader.cost = Some(Arc::new(v.cost.clone()));
        match shader.kind {
            ShaderKind::Generator if v.runs_at.half => shader.resolution = Resolution::Half,
            ShaderKind::Generator if shader.resolution == Resolution::Auto => {
                shader.resolution = v.cost.resolved.unwrap_or(Resolution::Full);
            }
            ShaderKind::Filter if v.runs_at.half => {
                if let Some(d) = &mut shader.downsample {
                    d.at_least_px = 0.0;
                }
            }
            _ => {}
        }
        shader
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Cost {
    /// Tests: a Normal cost (2.0 ms) on any computer, `ms` a frame at 1080p and 4 × that at 2160p in export (the
    /// 12's tiles), nothing bounded. Here rather than in `make::budget`, whose text pins `MEASURE_VERSION`.
    #[doc(hidden)]
    pub fn with_export_ms(ms: f64) -> Cost {
        use crate::make::budget::{DecidedBy, ExportMs, Pcts};
        Cost {
            preview_ms: Pcts::flat(1.0),
            export_ms: ExportMs {
                p1080: ms,
                p2160: 4.0 * ms,
            },
            cpu_us: None,
            gpu_bytes: Default::default(),
            class: CostClass::Normal,
            decided_ms: 1.0,
            at_half: false,
            decided_by: DecidedBy::Time,
            budget_ms: 2.0,
            runs_at: RunsAt::default(),
            resolved: None,
            envelope: Default::default(),
            machine: "test|any|0".into(),
            advice: Vec::new(),
            cut: false,
            fenced: None,
        }
    }
}
