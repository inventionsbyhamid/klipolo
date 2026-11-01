//! The advice measures a definition against what it could run as. At a stop, one that may run at half size is held to
//! Heavy there (a quarter of the pixels), never to Normal; "or let it run at half size" is offered only to what could
//! (a generator or a filter, never a transition); and the CPU line says what Heavy needs, as the memory line does.

use super::*;

/// A loop of 5 trips at line 7.
fn five_trips() -> StaticFacts {
    StaticFacts {
        loops: vec![LoopFact {
            line: 7,
            max_trips: Some(5),
            bound_param: None,
        }],
        ..facts(true)
    }
}

#[test]
fn a_stop_is_measured_against_heavy_for_what_may_run_at_half_size() {
    let b = budget_for(GpuClass::Integrated);
    let auto = generator(Some(Resolution::Auto));
    // Stopped at 24 ms at full size: about 6 ms at half size, so the 8-trip loop isn't hopeless and Normal isn't the
    // class it is held to.
    let (class, _, advice) = classify(&m(Over, Preview(24.0), 0, 0), Some(&octaves()), &auto, &b);
    assert_eq!(class, Over);
    assert!(
        !advice
            .iter()
            .any(|a| a.contains("isn't enough") || a.contains("over Normal")),
        "{advice:?}"
    );
    // The costliest loop is still named, held to Heavy at half size.
    assert!(
        advice
            .iter()
            .any(|a| a.contains("line 7") && a.contains("Heavy") && a.contains("lower it")),
        "{advice:?}"
    );
}

#[test]
fn half_size_is_offered_only_to_what_could_run_there() {
    let b = budget_for(GpuClass::Integrated);
    let transition = ContribDef::new("lib.k3f9q2ma/transition.x.v1", ContribKind::Transition, "X");
    let (_, _, advice) = classify(
        &m(Over, Preview(19.4), 0, 0),
        Some(&five_trips()),
        &transition,
        &b,
    );
    assert!(advice.iter().any(|a| a.contains("line 7")), "{advice:?}");
    assert!(
        !advice.iter().any(|a| a.contains("half size")),
        "{advice:?}"
    );
    // A generator that may not run at half size still hears it.
    let (_, _, advice) = classify(
        &m(Over, Preview(19.4), 0, 0),
        Some(&five_trips()),
        &generator(None),
        &b,
    );
    assert!(
        advice
            .iter()
            .any(|a| a.contains("line 7") && a.contains("or let it run at half size")),
        "{advice:?}"
    );
}

#[test]
fn the_cpu_line_says_what_heavy_needs() {
    let b = budget_for(GpuClass::Integrated);
    let mut busy = m(Light, Preview(0.3), 0, 0);
    busy.cpu_us = Some(Pcts {
        p50: 600.0,
        p95: 800.0,
    });
    let (class, _, advice) = classify(&busy, None, &generator(None), &b);
    assert_eq!(class, Over);
    let line = advice
        .iter()
        .find(|a| a.contains("µs of CPU"))
        .unwrap_or_else(|| panic!("no CPU line: {advice:?}"));
    assert!(line.contains("Heavy needs"), "{line}");
}

/// An agent is told when its effect was timed in batches by fences (with the batch's frames), and, when a
/// point that read Light wasn't resolved, that it counts as Normal, never Light; a trusted measurement says nothing of
/// fences.
#[test]
fn the_advice_says_when_the_timing_was_fenced() {
    let b = budget_for(GpuClass::Integrated);
    let plain = generator(None);
    let resolved = Measured {
        fenced: Some(fenced_record(false)),
        ..m(Light, Preview(0.2), 0, 0)
    };
    let (class, _, advice) = classify(&resolved, Some(&animating_facts()), &plain, &b);
    assert_eq!(class, Light);
    let line = advice
        .iter()
        .find(|a| a.contains("fences"))
        .unwrap_or_else(|| panic!("{advice:?}"));
    assert!(
        line.contains("batch of 17 frames") && line.contains("cancels"),
        "{line}"
    );
    assert!(
        !advice.iter().any(|a| a.contains("never Light")),
        "{advice:?}"
    );
    let held = Measured {
        fenced: Some(fenced_record(true)),
        ..m(Normal, Preview(0.2), 0, 0)
    };
    let (class, _, advice) = classify(&held, Some(&animating_facts()), &plain, &b);
    assert_eq!(class, Normal);
    assert!(
        advice.iter().any(|a| a.contains("never Light")),
        "{advice:?}"
    );
    assert!(
        !advice.iter().any(|a| a.contains("over Light")),
        "no loop advice for a figure within Light: {advice:?}"
    );
    let (_, _, advice) = classify(
        &m(Light, Preview(0.2), 0, 0),
        Some(&animating_facts()),
        &plain,
        &b,
    );
    assert!(!advice.iter().any(|a| a.contains("fences")), "{advice:?}");
}
