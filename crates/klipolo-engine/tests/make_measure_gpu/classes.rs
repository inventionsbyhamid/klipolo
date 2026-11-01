//! A measurement the time cut is answered Over with finite figures, the class is decided per point and the worst point
//! decides, the Over advice quotes the deciding figure, and the time cut of 2160p is noted. No half-size time is
//! measured that nothing reads.

use std::time::Duration;

use klipolo_engine::make::budget::CostClass;
use klipolo_engine::make::measure::FakeMeter;
use klipolo_engine::make::protocol::{MeasureSpec, SizeName};
use klipolo_model::contrib::{ContribDef, Resolution};

use crate::support::*;

const FLAT: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.5, 1.0); }";
/// Pure red takes a 40-trip loop; the default, green, takes none.
const KEYED: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = vec3<f32>(0.0);
    if (params.key.r > 0.9 && params.key.g < 0.1) {
        for (var i = 0; i < 40; i++) { c += vec3<f32>(sin(f32(i) + uv.x)) * 0.01; }
    }
    return vec4<f32>(c, 1.0);
}";
const KEY: &str = r##"[{"key":"key","label":"K","type":"color","default":"#00FF00"}]"##;

/// A measurement the time cuts before its probes are measured (20 ms a frame, a 1 s budget from the request's
/// arrival) is answered `Compiled`, Over, with its cut said explicitly and the note, and no figure on the line is
/// non-finite (serde_json writes those as `null`, which no reader takes).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_cut_measurement_is_answered_over_with_finite_figures() {
    let meter = FakeMeter::new(1.0).delay(Duration::from_millis(20));
    let mut w = InProcessWorker::new(meter).await;
    let spec = MeasureSpec {
        budget_ms: 1_000,
        ..MeasureSpec::default()
    };
    let c = w.compile_with(draft_def_with(KEY), KEYED, spec).await;
    let line = c.lines().last().cloned().unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    let cost = &v["cost"];
    for figure in [
        &cost["previewMs"]["p50"],
        &cost["previewMs"]["p95"],
        &cost["exportMs"]["1080p"],
        &cost["exportMs"]["2160p"],
    ] {
        assert!(figure.is_number(), "{line}");
    }
    assert_eq!(w.failed_replies(), 0);
    let cost = c.compiled().unwrap().cost.clone().unwrap();
    assert_eq!(cost.class, CostClass::Over, "{cost:?}");
    assert!(cost.cut, "{cost:?}");
    assert!(
        cost.advice.iter().any(|a| a.contains("ran out")),
        "{:?}",
        cost.advice
    );
}

/// The class is decided per point, and the worst point decides. A `resolution: half` generator whose default
/// costs 2.5 ms at half size (Heavy) and whose red probe costs about 30 ms (its step stops it, with no half-size
/// figure of its own) is Over, never Heavy from the default's half-size time. At about 11.7 ms the probe's frames stop
/// at Heavy: Over too.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_half_size_generator_with_a_costly_probe_is_over() {
    for (per_trip, probe) in [(3.0, "about 30 ms"), (1.0, "about 11.7 ms")] {
        let def = ContribDef {
            resolution: Some(Resolution::Half),
            ..draft_def_with(KEY)
        };
        let mut w = InProcessWorker::new(FakeMeter::new(10.85).per_trip(per_trip)).await;
        let c = w.compile(def, KEYED).await;
        let cost = c.compiled().unwrap().cost.clone().unwrap();
        assert_eq!(cost.class, CostClass::Over, "red at {probe}: {cost:?}");
    }
}

/// The Over advice quotes the figure that decided the class: for a definition that may run at half size and is
/// over Normal at full size, its half-size time (9 ms here) against Heavy's 6 ms.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_over_advice_quotes_the_deciding_half_size_time() {
    let def = ContribDef {
        resolution: Some(Resolution::Auto),
        ..draft_def_with("[]")
    };
    let mut w = InProcessWorker::new(FakeMeter::new(1.0).at_size((1280, 720), 9.0)).await;
    let c = w.compile(def, FLAT).await;
    let cost = c.compiled().unwrap().cost.clone().unwrap();
    assert_eq!(cost.class, CostClass::Over, "{cost:?}");
    assert!(
        cost.advice
            .iter()
            .any(|a| a.contains("9.0 ms a frame at half size against 6.0 ms")),
        "{:?}",
        cost.advice
    );
}

/// A time cut after the points is noted for what it is. At 20 ms a frame, a draft with no costly setting
/// spends about 0.7 s on its defaults and 0.6 s on 1080p, so a 1.6 s budget cuts 2160p (sent as 4 × the 1080p figure).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_cut_2160p_time_is_noted() {
    let meter = FakeMeter::new(1.0).delay(Duration::from_millis(20));
    let mut w = InProcessWorker::new(meter).await;
    w.compile(draft_def_with("[]"), FLAT).await; // the device, made before the timed requests
    let spec = MeasureSpec {
        sizes: vec![SizeName::P2160],
        budget_ms: 1_600,
        ..MeasureSpec::default()
    };
    let c = w.compile_with(draft_def_with("[]"), FLAT, spec).await;
    let advice = c.compiled().unwrap().cost.clone().unwrap().advice;
    assert!(
        advice
            .iter()
            .any(|a| a.contains("3840×2160 time is 4 × the 1080p figure")),
        "{advice:?}"
    );
}

/// No reader takes a half-size time from a point within Normal, so none is measured: an
/// `auto` generator within Normal (0.9 ms at preview size) times no half-size frame.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_draft_within_normal_times_no_half_size_frame() {
    let auto = ContribDef {
        resolution: Some(Resolution::Auto),
        ..draft_def_with("[]")
    };
    let meter = FakeMeter::new(1.0);
    let log = meter.log();
    let mut w = InProcessWorker::new(meter).await;
    let cost = w
        .compile(auto, FLAT)
        .await
        .compiled()
        .unwrap()
        .cost
        .clone()
        .unwrap();
    assert_eq!(cost.class, CostClass::Normal, "{cost:?}");
    let half = log.lock().unwrap().iter().filter(|m| m.half).count();
    assert_eq!(half, 0, "half-size frames timed");
}

/// A definition that may run at half size has its half-size path warmed, untimed, before it is timed, as each size of
/// the steps is (`Meter::warm`): a first half-size frame that pays for its pipelines and targets never stops it at
/// 24 ms. An `auto` generator at 2.8 ms full size (over Normal) and 0.7 ms at half size (Heavy), whose first frame at
/// each size and scale costs 30 ms more unless warmed, is Heavy.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_half_size_path_is_warmed_before_it_is_timed() {
    let auto = ContribDef {
        resolution: Some(Resolution::Auto),
        ..draft_def_with("[]")
    };
    let mut w = InProcessWorker::new(FakeMeter::new(3.0).first_frame(30.0)).await;
    let cost = w
        .compile(auto, FLAT)
        .await
        .compiled()
        .unwrap()
        .cost
        .clone()
        .unwrap();
    assert_eq!(cost.class, CostClass::Heavy, "{cost:?}");
}
