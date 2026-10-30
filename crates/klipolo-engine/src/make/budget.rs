//! What a measurement found: the `Cost` data types, the one definition the protocol re-exports
//! table and the time thresholds the measurement narrows by; the classes on time,
//! memory and CPU (`classify`), each kind's own memory (`Own`), the advice and the machine key. Read from the
//! worker's replies, so a field these types don't have is refused.

use klipolo_model::contrib::{ContribDef, Resolution};
use serde::{Deserialize, Serialize};

mod advice;
mod classify;
mod closed;
mod machine;
mod memory;

pub use super::envelope::Envelope;
pub use super::measure::{Batch, Batches, Fenced, MemoryUse, Pcts};
use super::measure::{Decided, Measured};
use crate::platform::gpu::GpuClass;
pub use classify::{DecidedBy, MEASURE_VERSION, classify, may_halve};
pub use closed::figure;
pub use machine::{MachineKey, this_computer};
pub use memory::{Own, SCENE_OWN_BYTES};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cost {
    /// GPU time a frame at preview size (the format's preview pixels within 0.92 MP), over 30 frames: the worst
    /// measured. With `cut`, the combinations that weren't measured aren't in it, and the class comes from the
    /// measurement's own class (`Measured::class`), never from these figures.
    pub preview_ms: Pcts,
    pub export_ms: ExportMs,
    /// A scene's CPU time a frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_us: Option<P95>,
    pub gpu_bytes: MemoryUse,
    pub class: CostClass,
    /// The time figure, in ms: the worst point's p95 at half size when `at_half`, else at preview
    /// size. A card compares it with `budget_ms` (never `preview_ms`) only when `decided_by` is the time (N-m2).
    #[serde(deserialize_with = "figure")]
    pub decided_ms: f64,
    pub at_half: bool,
    /// What decided the class: with `Cut`, `decided_ms` is Over's threshold, never a measured figure (N-m2).
    pub decided_by: DecidedBy,
    /// The class's GPU limit, in ms.
    #[serde(deserialize_with = "figure")]
    pub budget_ms: f64,
    pub runs_at: RunsAt,
    /// What `resolution: auto` resolved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Resolution>,
    pub envelope: Envelope,
    /// The computer it was measured on (its machine key).
    pub machine: String,
    /// What would make it cheaper, for the agent (English).
    #[serde(default)]
    pub advice: Vec<String>,
    /// A setting combination the envelope covers wasn't measured (the time or a cap cut it; the advice says which), so
    /// the class is Over: `fail_closed` makes it so, whatever class it names.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
    /// Timed by fences, in batches: their times, kept so a later trusted run can compare, and whether
    /// they resolved the cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fenced: Option<Fenced>,
}

impl Cost {
    /// The cost a measurement answers: `m` with its class, how it runs and its advice (`classify`), the
    /// class's own limit as `budget_ms`, 2160p as 4 × the 1080p figure unless it was measured, cut when a
    /// combination wasn't measured, through `fail_closed`. What `auto` resolved to is the caller's (`resolved`).
    pub fn of(
        m: &Measured,
        (class, runs_at, advice): (CostClass, RunsAt, Vec<String>),
        def: &ContribDef,
        machine: &MachineKey,
        b: &Budget,
    ) -> Cost {
        // An unmeasured figure is not finite: `fail_closed` sends it as Over's threshold, with `cut`.
        let (decided_ms, at_half) = match m.decided {
            Decided::Preview(ms) => (ms, false),
            Decided::Half(ms) => (ms, true),
            Decided::Unmeasured => (f64::INFINITY, false),
        };
        Cost {
            preview_ms: m.preview,
            export_ms: ExportMs {
                p1080: m.p1080.p95,
                p2160: m.p2160.unwrap_or(4.0 * m.p1080.p95),
            },
            cpu_us: m.cpu_us.map(|c| P95 { p95: c.p95 }),
            gpu_bytes: m.bytes,
            class,
            decided_ms,
            at_half,
            decided_by: classify::decided_by(m, def, b),
            budget_ms: b.limit_ms(class, may_halve(def)),
            runs_at,
            resolved: None,
            envelope: m.envelope.clone(),
            machine: machine.as_str().into(),
            advice,
            cut: m.decided == Decided::Unmeasured,
            fenced: m.fenced.clone(),
        }
        .fail_closed()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct P95 {
    #[serde(deserialize_with = "figure")]
    pub p95: f64,
}

/// p95 a frame at export sizes; 2160p is 4 × the 1080p figure unless it was measured.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportMs {
    #[serde(rename = "1080p", deserialize_with = "figure")]
    pub p1080: f64,
    #[serde(rename = "2160p", deserialize_with = "figure")]
    pub p2160: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostClass {
    Light,
    Normal,
    Heavy,
    /// Refused for placement.
    Over,
}

/// How it runs: none set means full size, every frame. Caching and stepping add to half size, never replace it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunsAt {
    pub half: bool,
    pub cached: bool,
    pub stepped: bool,
}

/// The classes' limits (one table for every GPU, calibrated on the Intel Mac by hand; it changes only by explicit
/// choice).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub light_ms: f64,
    pub normal_ms: f64,
    /// At half size.
    pub heavy_ms: f64,
    pub light_cpu_us: f64,
    pub normal_cpu_us: f64,
    pub heavy_cpu_us: f64,
    pub normal_extra_targets: u32,
    pub heavy_extra_targets: u32,
    pub normal_extra_bytes: u64,
    pub heavy_extra_bytes: u64,
}

impl Budget {
    pub const DEFAULT: Budget = Budget {
        light_ms: 0.5,
        normal_ms: 2.0,
        heavy_ms: 6.0,
        light_cpu_us: 100.0,
        normal_cpu_us: 300.0,
        heavy_cpu_us: 1000.0,
        normal_extra_targets: 1,
        heavy_extra_targets: 2,
        normal_extra_bytes: 8 << 20,
        heavy_extra_bytes: 16 << 20,
    };

    /// A class's own GPU limit (Over's: the largest a class allows, `heavy_ms` when it may run at half, else
    /// `normal_ms`).
    pub fn limit_ms(&self, class: CostClass, may_halve: bool) -> f64 {
        match class {
            CostClass::Light => self.light_ms,
            CostClass::Normal => self.normal_ms,
            CostClass::Heavy => self.heavy_ms,
            CostClass::Over if may_halve => self.heavy_ms,
            CostClass::Over => self.normal_ms,
        }
    }
}

/// The classes' limits on a GPU of `class`: one table for every class, since the
/// thresholds come from the playback budget and every computer measures its own cost.
pub fn budget_for(class: GpuClass) -> Budget {
    let _ = class;
    Budget::DEFAULT
}

/// A frame predicted over this is never drawn: the measurement steps up to it from small sizes.
pub const STEP_LIMIT_MS: f64 = 50.0;
/// A preview-size frame over 4 × the Heavy limit ends its combination as Over: nothing more is measured for it.
pub const OVER_AT_PREVIEW_MS: f64 = 4.0 * Budget::DEFAULT.heavy_ms;

/// A point's class by time alone (`classify` adds memory and CPU): Light and Normal by the preview p95; Heavy only for
/// a definition that may run at half size, by its half-size p95; else Over.
pub fn class_of_time(
    preview_p95: f64,
    half_p95: Option<f64>,
    may_halve: bool,
    b: &Budget,
) -> CostClass {
    if !preview_p95.is_finite() {
        CostClass::Over
    } else if preview_p95 <= b.light_ms {
        CostClass::Light
    } else if preview_p95 <= b.normal_ms {
        CostClass::Normal
    } else if may_halve && half_p95.is_some_and(|h| h.is_finite() && h <= b.heavy_ms) {
        CostClass::Heavy
    } else {
        CostClass::Over
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light and Normal by the preview p95; Heavy only at half size for what may run there; anything unmeasured
    /// (not finite) is Over.
    #[test]
    fn the_class_by_time() {
        let b = Budget::DEFAULT;
        assert_eq!(class_of_time(0.4, None, false, &b), CostClass::Light);
        assert_eq!(class_of_time(1.5, None, false, &b), CostClass::Normal);
        assert_eq!(class_of_time(3.0, Some(5.0), true, &b), CostClass::Heavy);
        assert_eq!(class_of_time(3.0, Some(5.0), false, &b), CostClass::Over);
        assert_eq!(class_of_time(3.0, Some(7.0), true, &b), CostClass::Over);
        assert_eq!(
            class_of_time(3.0, Some(f64::NAN), true, &b),
            CostClass::Over
        );
        assert_eq!(
            class_of_time(f64::INFINITY, Some(1.0), true, &b),
            CostClass::Over
        );
    }
}
