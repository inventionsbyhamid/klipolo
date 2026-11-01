//! The budget classes: time and memory both decide the class, the time class being the measurement's own (decided per
//! point, never taken again from figures that leave out what wasn't measured); Heavy only for what may run at half
//! size; a padded filter's grown targets its own; the advice names the loop that costs most; and the contribution's key
//! covers what is drawn. Also: the advice measures against what could run (`advice`), the cost says what decided it
//! (`decided`), and the measurement version is pinned (`version`).

#[path = "budget/advice.rs"]
mod advice;
#[path = "budget/decided.rs"]
mod decided;
#[path = "budget/version.rs"]
mod version;

use klipolo_engine::make::budget::{
    Cost, CostClass, MachineKey, Own, RunsAt, budget_for, classify,
};
use klipolo_engine::make::envelope::Envelope;
use klipolo_engine::make::measure::{Decided, Measured, MemoryUse, Pcts};
use klipolo_engine::platform::gpu::GpuClass;
use klipolo_engine::registry::guard::{LoopFact, StaticFacts};
use klipolo_engine::registry::key::{ContribSource, contrib_key};
use klipolo_model::contrib::{ContribDef, ContribKind, PaddingSpec, Resolution};

use CostClass::{Heavy, Light, Normal, Over};
use Decided::{Half, Preview, Unmeasured};

/// A measurement whose time class is `class`, decided by `decided`, with that extra memory.
fn m(class: CostClass, decided: Decided, extra_targets: u32, extra_bytes: u64) -> Measured {
    let own = 1280 * 720 * 4;
    let p95 = match decided {
        Preview(ms) => ms,
        Half(ms) => ms * 3.0,
        Unmeasured => 0.3,
    };
    Measured {
        class,
        decided,
        preview: Pcts {
            p50: p95 * 0.8,
            p95,
        },
        p1080: Pcts {
            p50: p95 * 2.0,
            p95: p95 * 2.25,
        },
        p2160: None,
        cpu_us: None,
        worst: Pcts { p50: p95, p95 },
        envelope: Envelope::default(),
        bytes: MemoryUse {
            own,
            extra_targets,
            extra_bytes,
            peak: own + u64::from(extra_targets) * own + extra_bytes,
        },
        fenced: None,
    }
}

fn generator(res: Option<Resolution>) -> ContribDef {
    ContribDef {
        resolution: res,
        ..ContribDef::new("lib.k3f9q2ma/generator.x.v1", ContribKind::Generator, "X")
    }
}

fn facts(animates: bool) -> StaticFacts {
    StaticFacts {
        loops: Vec::new(),
        ops_per_pixel: 200,
        samples_per_pixel: 0,
        ir_expressions: 20,
        functions: 1,
        animates,
        reads_layer: false,
        costly_params: Vec::new(),
    }
}

fn animating_facts() -> StaticFacts {
    facts(true)
}

fn static_facts() -> StaticFacts {
    facts(false)
}

#[test]
fn time_and_memory_both_decide_the_class() {
    let b = budget_for(GpuClass::Integrated);
    assert_eq!(
        classify(&m(Light, Preview(0.4), 0, 0), None, &generator(None), &b).0,
        Light
    );
    // Fast but with an extra target: not Light.
    assert_eq!(
        classify(&m(Light, Preview(0.4), 1, 0), None, &generator(None), &b).0,
        Normal
    );
    assert_eq!(
        classify(&m(Normal, Preview(1.8), 0, 0), None, &generator(None), &b).0,
        Normal
    );
    // Three extra targets, or 20 MiB of buffers: Over whatever the time.
    assert_eq!(
        classify(&m(Light, Preview(0.3), 3, 0), None, &generator(None), &b).0,
        Over
    );
    let half = generator(Some(Resolution::Half));
    assert_eq!(
        classify(&m(Light, Half(0.1), 1, 20 << 20), None, &half, &b).0,
        Over
    );
    assert_eq!(classify(&m(Over, Half(7.0), 0, 0), None, &half, &b).0, Over);
    // Extra memory alone makes a definition that may run at half size Heavy, at half size.
    let auto = generator(Some(Resolution::Auto));
    let (class, runs, _) = classify(&m(Light, Preview(0.3), 2, 12 << 20), None, &auto, &b);
    assert_eq!((class, runs.half), (Heavy, true));
}

/// A measurement whose probes were cut before they started is Over, though the figures it measured (its
/// defaults' 0.3 ms) are Light's, and so is the cost it answers: cut, with finite figures.
#[test]
fn a_measurement_cut_before_its_probes_is_over() {
    let b = budget_for(GpuClass::Integrated);
    let cut = m(Over, Unmeasured, 0, 0);
    assert!(cut.preview.p95 <= b.light_ms);
    let verdict = classify(&cut, None, &generator(None), &b);
    assert_eq!(verdict.0, Over);
    // The time class is the measurement's even where a figure would read otherwise: never Light from 0.3 ms.
    assert_eq!(
        classify(&m(Light, Unmeasured, 0, 0), None, &generator(None), &b).0,
        Over
    );
    let machine = MachineKey::from_parts("vulkan", "llvmpipe", "6.8");
    let cost = Cost::of(&cut, verdict, &generator(None), &machine, &b);
    assert_eq!((cost.class, cost.cut), (Over, true), "{cost:?}");
    assert!(cost.preview_ms.p95.is_finite() && cost.export_ms.p2160.is_finite());
    assert_eq!(cost.runs_at, RunsAt::default());
}

#[test]
fn heavy_needs_a_definition_that_may_run_at_half_size() {
    let b = budget_for(GpuClass::Integrated);
    // Allowed: Heavy, at half.
    let (class, runs, advice) = classify(
        &m(Heavy, Half(1.7), 0, 0),
        None,
        &generator(Some(Resolution::Auto)),
        &b,
    );
    assert_eq!((class, runs.half), (Heavy, true));
    assert!(advice.iter().any(|a| a.contains("half")), "{advice:?}");
    // Heavy with `fps` and facts that animate: still half size, and stepped as well.
    let stepped = ContribDef {
        fps: Some(12),
        ..generator(Some(Resolution::Half))
    };
    let (class, runs, _) = classify(
        &m(Heavy, Half(1.7), 0, 0),
        Some(&animating_facts()),
        &stepped,
        &b,
    );
    assert_eq!((class, runs.half, runs.stepped), (Heavy, true, true));
    // Heavy and static: half and cached.
    let (_, runs, _) = classify(
        &m(Heavy, Half(1.7), 0, 0),
        Some(&static_facts()),
        &generator(Some(Resolution::Half)),
        &b,
    );
    assert_eq!((runs.half, runs.cached), (true, true));
    // Not allowed (full size only): Over, and the advice says what would make it Heavy.
    let (class, _, advice) = classify(&m(Over, Preview(5.0), 0, 0), None, &generator(None), &b);
    assert_eq!(class, Over);
    assert!(
        advice
            .iter()
            .any(|a| a.contains("allow `resolution: half`")),
        "{advice:?}"
    );
    // A Heavy time for a definition that may not run at half size is Over all the same.
    assert_eq!(
        classify(&m(Heavy, Half(1.7), 0, 0), None, &generator(None), &b).0,
        Over
    );
}

#[test]
fn a_padded_filters_grown_targets_are_its_own() {
    // Its stack's ping-pong pair grown by its padding (48 px at 1080p: 32 px at 720p) is its own, so a fast
    // padded filter drawing both is Light; a third target is extra.
    let b = budget_for(GpuClass::Integrated);
    let filter = ContribDef {
        padding: Some(PaddingSpec {
            px: 0.0,
            param: Some("size".into()),
            per_unit: 1.0,
            max_px: 48.0,
        }),
        ..ContribDef::new(
            "lib.k3f9q2ma/effect.sparkle.v1",
            ContribKind::Effect,
            "Sparkle",
        )
    };
    let own = Own::filter((896, 504), 32.0);
    let grown = (896 + 64) * (504 + 64) * 4;
    let mut fast = m(Light, Preview(0.45), 0, 0);
    fast.bytes = own.split(2 * grown, 0);
    assert_eq!((fast.bytes.extra_targets, fast.bytes.extra_bytes), (0, 0));
    assert_eq!(classify(&fast, None, &filter, &b).0, Light);
    fast.bytes = own.split(3 * grown, 0);
    assert_eq!(classify(&fast, None, &filter, &b).0, Normal);
}

#[test]
fn the_key_covers_the_definitions_rendering_fields_and_the_source_but_not_its_name() {
    let body = ContribSource::Wgsl(
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }".into(),
    );
    let a = generator(None);
    let renamed = ContribDef {
        name: "Another name".into(),
        tags: vec!["x".into()],
        ..generator(None)
    };
    let halved = generator(Some(Resolution::Half));
    assert_eq!(contrib_key(&a, &body), contrib_key(&renamed, &body));
    assert_ne!(
        contrib_key(&a, &body),
        contrib_key(&halved, &body),
        "a changed definition is a new check"
    );
    let other = ContribSource::Wgsl(
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }".into(),
    );
    assert_ne!(contrib_key(&a, &body), contrib_key(&a, &other));
    // A scene's bytes exactly as stored: the same JSON written another way is another key.
    let scene = ContribSource::Scene(br#"{"scene":1}"#.to_vec());
    let spaced = ContribSource::Scene(br#"{ "scene": 1 }"#.to_vec());
    assert_ne!(contrib_key(&a, &scene), contrib_key(&a, &spaced));
}

/// `octaves8.wgsl`'s facts: an 8-trip loop at line 7, a 2-trip one at line 12.
fn octaves() -> StaticFacts {
    klipolo_engine::registry::guard::check(
        &generator(None),
        klipolo_engine::registry::ShaderKind::Generator,
        include_str!("fixtures/wgsl/octaves8.wgsl"),
    )
    .unwrap()
    .facts
}

#[test]
fn advice_names_the_loop_that_costs_most() {
    let facts = octaves();
    let (_, _, advice) = classify(
        &m(Over, Preview(3.6), 0, 0),
        Some(&facts),
        &generator(None),
        &budget_for(GpuClass::Integrated),
    );
    assert!(
        advice
            .iter()
            .any(|a| a.contains("line 7") && a.contains("1.8×")),
        "{advice:?}"
    );
}

/// The advice never promises a fix that can't work, names the loop where a stop ended it, and always
/// says why a definition that may not run at half size is Over.
#[test]
fn the_advice_names_what_would_work() {
    let b = budget_for(GpuClass::Integrated);
    let five = StaticFacts {
        loops: vec![LoopFact {
            line: 7,
            max_trips: Some(5),
            bound_param: None,
        }],
        ..facts(true)
    };
    // 9.7× over Normal with a 5-trip loop: lowering that loop alone can't do it.
    let (_, _, advice) = classify(
        &m(Over, Preview(19.4), 0, 0),
        Some(&five),
        &generator(None),
        &b,
    );
    assert!(
        !advice.iter().any(|a| a.contains("lower it to 1")),
        "{advice:?}"
    );
    assert!(
        advice
            .iter()
            .any(|a| a.contains("line 7") && a.contains("isn't enough")),
        "{advice:?}"
    );
    // Stopped at 24 ms: the loop is named all the same, at least 12× over Normal.
    let (_, _, advice) = classify(
        &m(Over, Preview(24.0), 0, 0),
        Some(&octaves()),
        &generator(None),
        &b,
    );
    assert!(
        advice
            .iter()
            .any(|a| a.contains("line 7") && a.contains("At least 12")),
        "{advice:?}"
    );
    // Heavy by time or by memory, for a definition that may not run at half size: Over, and the advice says why.
    for over in [m(Heavy, Half(1.7), 0, 0), m(Light, Preview(0.3), 2, 0)] {
        let (class, _, advice) = classify(&over, None, &generator(None), &b);
        assert_eq!(class, Over);
        assert!(
            advice.iter().any(|a| a.contains("Heavy needs")),
            "{advice:?}"
        );
    }
}

/// The figure that decided the class goes on the wire, so an allowed Heavy reads 1.7 ms at half size
/// against its 6.0 ms, never its 5.1 ms at full size; a cut cost's figure is finite.
#[test]
fn the_cost_carries_the_figure_that_decided_it() {
    let b = budget_for(GpuClass::Integrated);
    let machine = MachineKey::from_parts("vulkan", "llvmpipe", "6.8");
    let auto = generator(Some(Resolution::Auto));
    let heavy = m(Heavy, Half(1.7), 0, 0);
    let cost = Cost::of(
        &heavy,
        classify(&heavy, None, &auto, &b),
        &auto,
        &machine,
        &b,
    );
    assert_eq!(
        (cost.decided_ms, cost.at_half, cost.budget_ms),
        (1.7, true, 6.0)
    );
    let wire = serde_json::to_value(&cost).unwrap();
    assert_eq!(
        (wire["decidedMs"].as_f64(), wire["atHalf"].as_bool()),
        (Some(1.7), Some(true))
    );
    let normal = m(Normal, Preview(1.2), 0, 0);
    let cost = Cost::of(
        &normal,
        classify(&normal, None, &auto, &b),
        &auto,
        &machine,
        &b,
    );
    assert_eq!((cost.decided_ms, cost.at_half), (1.2, false));
    let cut = m(Over, Unmeasured, 0, 0);
    let cost = Cost::of(&cut, classify(&cut, None, &auto, &b), &auto, &machine, &b);
    assert!(cost.cut && cost.decided_ms.is_finite(), "{cost:?}");
}

/// A fenced cost carries its batches on the wire (each sample time's single frame and batch, and the
/// baseline's) and reads back the same; a trusted one sends no `fenced` at all; a fenced figure that isn't finite
/// (`null`) fails closed, as every other figure does.
#[test]
fn a_fenced_cost_keeps_its_batches_on_the_wire() {
    let b = budget_for(GpuClass::Integrated);
    let machine = MachineKey::from_parts("metal", "Intel(R) Iris(TM) Plus Graphics", "15.7.5");
    let plain = generator(None);
    let fenced = Measured {
        fenced: Some(fenced_record(false)),
        ..m(Light, Preview(0.2), 0, 0)
    };
    let cost = Cost::of(
        &fenced,
        classify(&fenced, None, &plain, &b),
        &plain,
        &machine,
        &b,
    );
    assert_eq!(cost.fenced, fenced.fenced);
    let wire = serde_json::to_value(&cost).unwrap();
    let preview = &wire["fenced"]["preview"];
    assert_eq!(preview["samples"][0]["frames"].as_u64(), Some(17), "{wire}");
    assert_eq!(
        preview["samples"][0]["singleMs"].as_f64(),
        Some(1.2),
        "{wire}"
    );
    assert_eq!(
        preview["samples"][0]["batchMs"].as_f64(),
        Some(4.4),
        "{wire}"
    );
    assert_eq!(
        preview["baseline"].as_array().map(Vec::len),
        Some(8),
        "{wire}"
    );
    assert_eq!(
        wire["fenced"]["1080p"]["resolved"].as_bool(),
        Some(true),
        "{wire}"
    );
    assert!(wire["fenced"].get("2160p").is_none(), "{wire}");
    let back: Cost = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(back.fail_closed(), cost);

    let trusted = m(Light, Preview(0.2), 0, 0);
    let cost = Cost::of(
        &trusted,
        classify(&trusted, None, &plain, &b),
        &plain,
        &machine,
        &b,
    );
    assert!(serde_json::to_value(&cost).unwrap().get("fenced").is_none());

    let mut broken = wire;
    broken["fenced"]["1080p"]["baseline"][2]["batchMs"] = serde_json::Value::Null;
    let read: Cost = serde_json::from_value(broken).unwrap();
    let read = read.fail_closed();
    assert!(read.cut && read.class == Over, "{read:?}");
    assert!(
        read.fenced.as_ref().is_some_and(|f| f
            .p1080
            .as_ref()
            .is_some_and(|b| b.baseline[2].batch_ms.is_finite())),
        "{read:?}"
    );
}

fn fenced_record(held: bool) -> klipolo_engine::make::budget::Fenced {
    use klipolo_engine::make::budget::{Batch, Batches, Fenced};
    let batch = |single_ms, batch_ms| Batch {
        frames: 17,
        single_ms,
        batch_ms,
    };
    let preview = Batches {
        samples: vec![batch(1.2, 4.4); 8],
        baseline: vec![batch(1.2, 1.2); 8],
        resolved: !held,
        baseline_steady: true,
        over_light: false,
    };
    Fenced {
        resolved: !held,
        held,
        preview: Some(preview.clone()),
        half: None,
        p1080: Some(preview),
        p2160: None,
    }
}
