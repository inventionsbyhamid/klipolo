//! Every cost is read fail-closed: a figure that isn't finite is
//! what the measurement couldn't make, so the cost is cut and Over.

use serde::Deserialize;

use super::{Budget, Cost, CostClass, DecidedBy, OVER_AT_PREVIEW_MS, RunsAt};

/// What a figure the measurement couldn't make is sent and read as: Over's preview threshold, in ms (a CPU figure:
/// 4 × Heavy's CPU limit, in µs).
const UNMEASURED_MS: f64 = OVER_AT_PREVIEW_MS;
const UNMEASURED_CPU_US: f64 = 4.0 * Budget::DEFAULT.heavy_cpu_us;

impl Cost {
    /// No figure that isn't finite: one is what the measurement couldn't make, so the cost is
    /// cut and Over, the figure Over's threshold. A cost that is cut is Over whatever class it names
    /// out what wasn't measured. The worker sends every cost through it, and the app
    /// reads every cost through it, a `null` figure (`figure`) included: Over, never a protocol error.
    pub fn fail_closed(mut self) -> Self {
        let mut cut = self.cut;
        for f in [
            &mut self.preview_ms.p50,
            &mut self.preview_ms.p95,
            &mut self.export_ms.p1080,
            &mut self.export_ms.p2160,
            &mut self.decided_ms,
            &mut self.budget_ms,
        ] {
            if !f.is_finite() {
                *f = UNMEASURED_MS;
                cut = true;
            }
        }
        if let Some(cpu) = &mut self.cpu_us
            && !cpu.p95.is_finite()
        {
            cpu.p95 = UNMEASURED_CPU_US;
            cut = true;
        }
        if let Some(fenced) = &mut self.fenced {
            for f in fenced.figures_mut() {
                if !f.is_finite() {
                    *f = UNMEASURED_MS;
                    cut = true;
                }
            }
        }
        if cut {
            // Over runs nowhere: no flags, and no resolution for `auto`. No measured figure decided it (N-m2).
            self.cut = true;
            self.class = CostClass::Over;
            self.decided_by = DecidedBy::Cut;
            self.decided_ms = UNMEASURED_MS;
            self.runs_at = RunsAt::default();
            self.resolved = None;
        }
        self
    }
}

impl crate::make::protocol::WorkerReply {
    /// A reply's cost, if it has one, through `Cost::fail_closed` (the app reads every reply so): `Compiled`,
    /// `Measured` and `Framed` are every reply that carries one.
    pub fn fail_closed(mut self) -> Self {
        use crate::make::protocol::WorkerReply as R;
        match &mut self {
            R::Compiled { cost, .. } | R::Framed { cost, .. } => {
                *cost = cost.take().map(Cost::fail_closed)
            }
            R::Measured { cost, .. } => *cost = cost.clone().fail_closed(),
            _ => {}
        }
        self
    }
}

/// A figure as read from the wire: `null`, serde_json's word for a non-finite number, is read as infinite, which
/// `Cost::fail_closed` makes Over.
pub fn figure<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::INFINITY))
}
