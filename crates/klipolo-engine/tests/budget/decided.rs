//! A cost says what decided its class, so a card compares `decidedMs` with `budgetMs` only when the time did (never
//! "0.3 of 2.0 ms" next to Over), and a cut carries no figure presented as measured.

use klipolo_engine::make::budget::DecidedBy;

use super::*;

#[test]
fn the_cost_says_what_decided_its_class() {
    let b = budget_for(GpuClass::Integrated);
    let machine = MachineKey::from_parts("vulkan", "llvmpipe", "6.8");
    let cost_of = |m: Measured, def: &ContribDef| {
        Cost::of(&m, classify(&m, None, def, &b), def, &machine, &b)
    };
    let plain = generator(None);
    assert_eq!(
        cost_of(m(Normal, Preview(1.2), 0, 0), &plain).decided_by,
        DecidedBy::Time
    );
    // Light by time (0.3 ms), but two extra targets for a definition that may not run at half size: Over by memory.
    let by_memory = cost_of(m(Light, Preview(0.3), 2, 0), &plain);
    assert_eq!(
        (by_memory.class, by_memory.decided_by),
        (Over, DecidedBy::Memory)
    );
    assert_eq!(
        serde_json::to_value(&by_memory).unwrap()["decidedBy"],
        "memory"
    );
    // A scene's CPU time.
    let mut busy = m(Light, Preview(0.3), 0, 0);
    busy.cpu_us = Some(Pcts {
        p50: 200.0,
        p95: 250.0,
    });
    let by_cpu = cost_of(busy, &plain);
    assert_eq!((by_cpu.class, by_cpu.decided_by), (Normal, DecidedBy::Cpu));
    // Heavy for a definition that may not run at half size: the half-size rule.
    let i2 = cost_of(m(Heavy, Half(1.7), 0, 0), &plain);
    assert_eq!((i2.class, i2.decided_by), (Over, DecidedBy::HalfSize));
    // Cut before its probes: the figure is Over's threshold, never the 0.3 ms its defaults measured.
    let cut = cost_of(m(Over, Unmeasured, 0, 0), &plain);
    assert_eq!(
        (cut.decided_by, cut.decided_ms),
        (DecidedBy::Cut, 24.0),
        "{cut:?}"
    );
    // A cost a worker says is cut, with a finite figure: read the same way.
    let said = Cost {
        cut: true,
        ..cost_of(m(Light, Preview(0.3), 0, 0), &plain)
    }
    .fail_closed();
    assert_eq!(
        (said.class, said.decided_by, said.decided_ms),
        (Over, DecidedBy::Cut, 24.0)
    );
}
