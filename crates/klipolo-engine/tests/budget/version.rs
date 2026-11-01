//! A verdict's meaning is pinned to `MEASURE_VERSION`. Changing the classes' table, the classes, the memory rules or
//! the measurement fails here until this is re-pinned, so nobody changes them without
//! deciding whether the verdicts already stored still hold. The renderer's own cost can't be pinned this way: a change
//! to what it draws is on the release checklist.

use klipolo_engine::make::budget::{Budget, MEASURE_VERSION};
use sha2::{Digest, Sha256};

use super::*;

/// What the version below was pinned against: the SHA-256 of these sources, each named and NUL-separated.
/// Version 7 pins batched, fenced timing (version 6) together with the scene measurement (`run/covered.rs` reads a
/// draft's facts through `DraftLayer::facts()`; `Meter::scene_cpu` reports a scene's CPU time), which needed no version
/// of its own.
/// Re-pinned without a version for the crates' rename to `klipolo-*` and for comment-only rewording: no code changed,
/// so no stored verdict can.
const PINNED: &str = "0a8433c33b4a540fea1a6f04099c8a47eac58ec8eab0da8aa21c6b6818bd8e9f";

#[test]
fn the_measurement_version_is_pinned_to_the_table_and_the_sources() {
    assert_eq!(MEASURE_VERSION, 7);
    let table = Budget {
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
    for class in [
        GpuClass::Apple,
        GpuClass::Integrated,
        GpuClass::Discrete,
        GpuClass::Software,
    ] {
        assert_eq!(budget_for(class), table, "{class:?}");
    }
    let sources = [
        ("budget.rs", include_str!("../../src/make/budget.rs")),
        (
            "budget/classify.rs",
            include_str!("../../src/make/budget/classify.rs"),
        ),
        (
            "budget/memory.rs",
            include_str!("../../src/make/budget/memory.rs"),
        ),
        // A cost read fail-closed, and the machine key a verdict is kept under.
        (
            "budget/closed.rs",
            include_str!("../../src/make/budget/closed.rs"),
        ),
        (
            "budget/machine.rs",
            include_str!("../../src/make/budget/machine.rs"),
        ),
        ("measure.rs", include_str!("../../src/make/measure.rs")),
        (
            "measure/run.rs",
            include_str!("../../src/make/measure/run.rs"),
        ),
        // The budgets that decide when a measurement is cut (and so Over).
        (
            "measure/deadline.rs",
            include_str!("../../src/make/measure/deadline.rs"),
        ),
        (
            "run/point.rs",
            include_str!("../../src/make/measure/run/point.rs"),
        ),
        (
            "run/covered.rs",
            include_str!("../../src/make/measure/run/covered.rs"),
        ),
        // The baselines, fenced timing in batches, the run's state, the batches' record and the worker's meter that
        // times them.
        (
            "run/baseline.rs",
            include_str!("../../src/make/measure/run/baseline.rs"),
        ),
        (
            "run/batched.rs",
            include_str!("../../src/make/measure/run/batched.rs"),
        ),
        (
            "run/state.rs",
            include_str!("../../src/make/measure/run/state.rs"),
        ),
        (
            "measure/fenced.rs",
            include_str!("../../src/make/measure/fenced.rs"),
        ),
        (
            "measure/gpu.rs",
            include_str!("../../src/make/measure/gpu.rs"),
        ),
        ("envelope.rs", include_str!("../../src/make/envelope.rs")),
        (
            "envelope/values.rs",
            include_str!("../../src/make/envelope/values.rs"),
        ),
        ("steps.rs", include_str!("../../src/make/steps.rs")),
        // `px_for` sizes the padded targets a measurement draws.
        (
            "model/contrib/meta.rs",
            include_str!("../../../klipolo-model/src/contrib/meta.rs"),
        ),
    ];
    let mut hash = Sha256::new();
    for (name, text) in sources {
        hash.update(name);
        hash.update([0]);
        hash.update(text);
        hash.update([0]);
    }
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex, PINNED,
        "the classes or the measurement changed: bump MEASURE_VERSION if a stored verdict could change, then re-pin"
    );
}
