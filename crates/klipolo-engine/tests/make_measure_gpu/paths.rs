//! What each path of a measurement costs: a size reached after the budget ran out draws nothing, Heavy's memory is its
//! half-size path's, a Measure answers with the hardened text's hash, the class by time is decided against the table it
//! is given, and a padded filter's own share is its grown pair.

use std::time::{Duration, Instant};

use klipolo_engine::make::budget::{Budget, CostClass};
use klipolo_engine::make::measure::{COMPILE_BUDGET, Deadline, FakeMeter, measure_noted};
use klipolo_engine::make::protocol::{MeasureSpec, SourceRef, WorkerReply, WorkerRequest};
use klipolo_model::contrib::{ContribDef, Resolution};

use crate::support::*;

const FLAT: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.5, 1.0); }";
const PASS: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return kl_sample_a(uv); }";

/// A size the measurement reaches after its budget ran out draws nothing, not even its baseline or its warm-up.
/// The defaults' last frame (the 34th timed: 4 steps and 30 frames) takes 700 ms of a 500 ms budget, so 1080p is
/// reached after it: no frame at 1920×1080, and its time is noted as estimated.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_size_reached_after_the_budget_draws_nothing() {
    let mut h = headless_alone().await;
    let layer = draft(draft_def_with("[]"), FLAT);
    let mut meter = FakeMeter::new(1.0).delay_from(34, Duration::from_millis(700));
    let touched = meter.touched();
    let deadline = Deadline::new(Instant::now(), 500, COMPILE_BUDGET);
    let spec = MeasureSpec::default();
    let (m, notes) = measure_noted(
        &mut meter,
        &mut h,
        &layer,
        &spec,
        &[],
        deadline,
        &Budget::DEFAULT,
    )
    .await
    .unwrap();
    assert_eq!(
        m.class,
        CostClass::Normal,
        "the defaults were measured in time"
    );
    assert!(
        !touched.lock().unwrap().contains(&(1920, 1080)),
        "a frame was drawn at 1920×1080 after the budget ran out"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("1920×1080 time is estimated")),
        "{notes:?}"
    );
}

/// Heavy's memory is what its half-size path uses. An `auto` generator Heavy by time (2.8 ms full size, 0.7 ms at
/// half size) whose half-size frames hold two layer-sized targets more than its own has those two as extra.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn heavys_memory_is_measured_on_its_half_size_path() {
    let auto = ContribDef {
        resolution: Some(Resolution::Auto),
        ..draft_def_with("[]")
    };
    let target = 1280 * 720 * 4;
    let mut w = InProcessWorker::new(FakeMeter::new(3.0).peaks(target, 3 * target)).await;
    let c = w.compile(auto, FLAT).await;
    let cost = c.compiled().unwrap().cost.clone().unwrap();
    assert_eq!(
        (cost.class, cost.gpu_bytes.extra_targets),
        (CostClass::Heavy, 2),
        "{cost:?}"
    );
}

/// A Measure answers with the hash of the hardened text it measured, the same as a Compile of that draft, so a
/// cost from either is kept against the hardened text (a naga update that changes it is checked again).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_measure_answers_with_the_hardened_hash() {
    let def = draft_def_with("[]");
    let mut w = InProcessWorker::new(FakeMeter::new(1.0)).await;
    let compiled = w.compile(def.clone(), FLAT).await;
    let hardened = compiled
        .lines()
        .iter()
        .find_map(|l| match serde_json::from_str(l).ok()? {
            WorkerReply::Compiled { hardened, .. } => hardened,
            _ => None,
        })
        .expect("Compile carries the hash");
    let lines = w
        .request(|id| WorkerRequest::Measure {
            id,
            source: SourceRef::Draft {
                def: Box::new(def),
                body: FLAT.into(),
            },
            measure: MeasureSpec::default(),
        })
        .await;
    let last = lines.last().unwrap();
    let WorkerReply::Measured { hardened: got, .. } = serde_json::from_str(last).unwrap() else {
        panic!("{last}");
    };
    assert_eq!(got, Some(hardened));
}

/// The class by time is decided against the table it is given (`budget_for`'s), as memory and CPU are: a 0.9 ms
/// draft is Normal against the default table and Over against one whose Normal ends at 0.5 ms.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_class_by_time_uses_the_table_it_is_given() {
    let mut h = headless_alone().await;
    let layer = draft(draft_def_with("[]"), FLAT);
    let strict = Budget {
        light_ms: 0.25,
        normal_ms: 0.5,
        ..Budget::DEFAULT
    };
    for (b, class) in [
        (Budget::DEFAULT, CostClass::Normal),
        (strict, CostClass::Over),
    ] {
        let deadline = Deadline::new(Instant::now(), 0, COMPILE_BUDGET);
        let mut meter = FakeMeter::new(1.0);
        let spec = MeasureSpec::default();
        let (m, _) = measure_noted(&mut meter, &mut h, &layer, &spec, &[], deadline, &b)
            .await
            .unwrap();
        assert_eq!(m.class, class, "{b:?}");
    }
}

/// The worker's own share of a padded filter: its ping-pong pair at the test image's size, 70 % of 1280×720
/// (896×504), each grown on every side by its padding at its largest, 64 px at 1080p (42.7 px at 720p): 982×590 RGBA
/// each. Two of them are its own; a third is an extra target.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_padded_filters_own_share_is_its_grown_pair() {
    let def = padded_filter_def("pad", Some(8.0));
    let grown: u64 = 982 * 590 * 4;
    for (peak, extra) in [(2 * grown, 0), (3 * grown, 1)] {
        let mut w = InProcessWorker::new(FakeMeter::new(1.0).peaks(peak, peak)).await;
        let c = w.compile(def.clone(), PASS).await;
        let cost = c.compiled().unwrap().cost.clone().unwrap();
        assert_eq!(
            (cost.gpu_bytes.own, cost.gpu_bytes.extra_targets),
            (2 * grown, extra),
            "{cost:?}"
        );
    }
}
