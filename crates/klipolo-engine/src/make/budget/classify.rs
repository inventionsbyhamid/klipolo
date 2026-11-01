//! The budget classes: a measurement's class on time, memory and (scenes) CPU, how it
//! runs, and the advice for the agent.
//!
//! The time class is the measurement's own (`Measured::class`): decided per point, the worst point deciding, Over
//! whenever a combination wasn't measured. It is never derived again from the
//! figures, which leave out what wasn't measured. Memory and CPU can only raise it, and Heavy needs a definition that
//! may run at half size: a generator with `resolution: half` or `auto`, a filter with a `downsample`.

use klipolo_model::contrib::{ContribDef, ContribKind, Resolution};

use serde::{Deserialize, Serialize};

use super::advice::{Verdict, advise};
use super::{Budget, CostClass, RunsAt};
use crate::make::measure::{Decided, Measured, MemoryUse};
use crate::registry::guard::StaticFacts;

/// The measurement's version: what the classes, the measurement and the renderer's cost are. A
/// verdict from another version is never read (the verified store keys and files carry it): bump it whenever `Budget`,
/// `classify`, `measure` or what the renderer costs changes. `tests/budget/version.rs` pins it to the table and these
/// sources. 2: `Cost` says what decided its class. 3: where the timing is fenced, the
/// floor it adds is taken off every figure. 4: where it is fenced, frames are timed in batches, whose slope
/// cancels the wait. 5: Light by the precision reached, short batches once a point can't be Light,
/// and no kept picture in a timed frame. 6: a batch whose copies drew other passes than its single frame
/// gives no slope. 7: the merged meter: the batched timing beside the scene measurement (`Meter::scene_cpu`, a
/// draft's facts through `DraftLayer::facts`).
pub const MEASURE_VERSION: u32 = 7;

/// What decided a cost's class, so a card compares `decided_ms` with `budget_ms` only when
/// the time did: the time figure; the extra memory; a scene's CPU time; the half-size rule (Heavy for a definition that
/// may not run at half size is Over); or a cut (a combination wasn't measured, so no measured figure decided it and
/// `decided_ms` is Over's threshold).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecidedBy {
    Time,
    Memory,
    Cpu,
    HalfSize,
    Cut,
}

/// Whether `def` may run at half size, and so be Heavy.
pub fn may_halve(def: &ContribDef) -> bool {
    match def.kind {
        ContribKind::Generator => {
            matches!(def.resolution, Some(Resolution::Half | Resolution::Auto))
        }
        ContribKind::Effect => def.downsample.is_some(),
        _ => false,
    }
}

/// `m`'s class with its memory and CPU, how it runs (independent flags; Heavy always at half size, caching and
/// stepping only added to that), and the advice.
pub fn classify(
    m: &Measured,
    facts: Option<&StaticFacts>,
    def: &ContribDef,
    b: &Budget,
) -> (CostClass, RunsAt, Vec<String>) {
    let may_halve = may_halve(def);
    let (by_time, by_memory, by_cpu) = classes(m, may_halve, b);
    let worst = by_time.max(by_memory).max(by_cpu);
    // Heavy only for a definition that may run at half size.
    let raised_by_i2 = worst == CostClass::Heavy && !may_halve;
    let class = if raised_by_i2 { CostClass::Over } else { worst };
    let runs = class != CostClass::Over;
    let generator = def.kind == ContribKind::Generator;
    let runs_at = RunsAt {
        half: runs
            && (class == CostClass::Heavy
                || (generator && def.resolution == Some(Resolution::Half))),
        cached: runs && generator && facts.is_some_and(|f| !f.animates),
        stepped: runs && generator && def.fps.is_some(),
    };
    let verdict = Verdict {
        class,
        by_time,
        by_memory,
        by_cpu,
        may_halve,
        raised_by_i2,
        runs_at,
    };
    (class, runs_at, advise(m, facts, def, b, &verdict))
}

/// `m`'s class on time, memory and CPU, each on its own.
fn classes(m: &Measured, may_halve: bool, b: &Budget) -> (CostClass, CostClass, CostClass) {
    let by_time = if m.decided == Decided::Unmeasured {
        CostClass::Over
    } else {
        m.class
    };
    let by_memory = memory_class(&m.bytes, may_halve, b);
    let by_cpu = m
        .cpu_us
        .map_or(CostClass::Light, |c| cpu_class(c.p95, may_halve, b));
    (by_time, by_memory, by_cpu)
}

/// What decided `m`'s class for `def`: a cut first, then the half-size rule, then the time where it is the worst
/// (a tie goes to the time), then memory, then CPU.
pub(super) fn decided_by(m: &Measured, def: &ContribDef, b: &Budget) -> DecidedBy {
    if m.decided == Decided::Unmeasured {
        return DecidedBy::Cut;
    }
    let may_halve = may_halve(def);
    let (by_time, by_memory, by_cpu) = classes(m, may_halve, b);
    if by_time.max(by_memory).max(by_cpu) == CostClass::Heavy && !may_halve {
        DecidedBy::HalfSize
    } else if by_time >= by_memory && by_time >= by_cpu {
        DecidedBy::Time
    } else if by_memory >= by_cpu {
        DecidedBy::Memory
    } else {
        DecidedBy::Cpu
    }
}

/// The class its extra memory allows: none for Light, one layer-sized target and 8 MiB for Normal, two and 16 MiB for
/// Heavy, when it may run at half size.
fn memory_class(bytes: &MemoryUse, may_halve: bool, b: &Budget) -> CostClass {
    let (t, x) = (bytes.extra_targets, bytes.extra_bytes);
    if t == 0 && x == 0 {
        CostClass::Light
    } else if t <= b.normal_extra_targets && x <= b.normal_extra_bytes {
        CostClass::Normal
    } else if may_halve && t <= b.heavy_extra_targets && x <= b.heavy_extra_bytes {
        CostClass::Heavy
    } else {
        CostClass::Over
    }
}

/// The class a scene's CPU time a frame (µs, p95) allows (the scene measurement measures it).
fn cpu_class(us: f64, may_halve: bool, b: &Budget) -> CostClass {
    if !us.is_finite() {
        CostClass::Over
    } else if us <= b.light_cpu_us {
        CostClass::Light
    } else if us <= b.normal_cpu_us {
        CostClass::Normal
    } else if may_halve && us <= b.heavy_cpu_us {
        CostClass::Heavy
    } else {
        CostClass::Over
    }
}
