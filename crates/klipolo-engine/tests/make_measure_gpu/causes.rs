//! A half-size generator judged against Heavy, the budget counted from the
//! request's arrival and clamped to the server's, notes that name their real cause, and a failure
//! that names its step and size, or a timer whose reading failed made again.

use std::time::{Duration, Instant};

use klipolo_engine::make::budget::CostClass;
use klipolo_engine::make::measure::FakeMeter;
use klipolo_engine::make::protocol::MeasureSpec;
use klipolo_model::contrib::{ContribDef, Resolution};

use crate::support::*;

const FLAT: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.5, 1.0); }";

/// A `resolution: half` generator is drawn at half size and judged against Heavy (6 ms). Its cost rises over
/// the layer (at half preview size, 230,400 px: 10.85 ns a pixel is 2.5 ms at its start, 41 ns 9.4 ms at its end), so
/// its first frames are over Normal but under Heavy, and its p95 over 30 is over Heavy: Over, never Heavy from two
/// early frames.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_half_size_generator_whose_cost_rises_past_heavy_is_over() {
    let def = ContribDef {
        resolution: Some(Resolution::Half),
        ..draft_def_with("[]")
    };
    let mut w = InProcessWorker::new(FakeMeter::new(10.85).rising(41.0)).await;
    let c = w.compile(def, FLAT).await;
    let cost = c.compiled().unwrap().cost.clone().unwrap();
    assert_eq!(cost.class, CostClass::Over, "{cost:?}");
}

/// The budget runs from the request's arrival: one already spent when the measurement starts leaves it all
/// unmeasured (Over, with the note), and no frame is drawn.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_budget_spent_before_measuring_draws_nothing() {
    use klipolo_engine::make::measure::{COMPILE_BUDGET, Deadline, measure_noted};
    let mut h = headless_alone().await;
    let layer = draft(draft_def(), WAVES_BODY);
    let mut meter = FakeMeter::new(1.0);
    let log = meter.log();
    let arrived = Instant::now().checked_sub(COMPILE_BUDGET).unwrap();
    let deadline = Deadline::new(arrived, 0, COMPILE_BUDGET);
    let spec = MeasureSpec::default();
    let b = klipolo_engine::make::budget::Budget::DEFAULT;
    let (_, notes) = measure_noted(&mut meter, &mut h, &layer, &spec, &[], deadline, &b)
        .await
        .unwrap();
    assert!(log.lock().unwrap().is_empty(), "no frame drawn");
    assert!(notes.iter().any(|n| n.contains("ran out")), "{notes:?}");
}

/// A client that asks for more than the server allows gets the server's budget (a compile's 6 s): this draft's
/// full measurement is about 200 frames of 40 ms.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_budget_asked_past_the_servers_is_clamped() {
    let meter = FakeMeter::new(1.0).delay(Duration::from_millis(40));
    let mut w = InProcessWorker::new(meter).await;
    let spec = MeasureSpec {
        budget_ms: 60_000,
        ..MeasureSpec::default()
    };
    let c = w
        .compile_with(branch_gated_def(), BRANCH_GATED_BODY, spec)
        .await;
    assert!(c.compiled().is_some_and(|c| c.ok), "{:?}", c.failure());
    assert!(
        w.frames_timed() <= 6_000 / 40 + 6,
        "{} frames timed",
        w.frames_timed()
    );
}

/// Setting combinations past the 12-corner cap are noted as the cap's, never as the time's. Two numbers costly
/// at both ends (4 corners), a choice of 9 (7 more values) and two bools ask for 14 corners with the defaults.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn combinations_past_the_cap_are_noted_as_the_caps() {
    let def = draft_def_with(
        r#"[{"key":"c","label":"C","type":"float","min":0,"max":1,"default":0.5},
            {"key":"d","label":"D","type":"float","min":0,"max":1,"default":0.5},
            {"key":"mode","label":"M","type":"choice","options":["a","b","c","d","e","f","g","h","i"],"default":"a"},
            {"key":"x","label":"X","type":"bool","default":false},
            {"key":"y","label":"Y","type":"bool","default":false}]"#,
    );
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = 0.0;
    if (abs(params.c - 0.5) < 0.1) { for (var i = 0; i < 8; i++) { c += sin(f32(i) + uv.x); } }
    if (abs(params.d - 0.5) < 0.1) { for (var i = 0; i < 8; i++) { c += cos(f32(i) + uv.y); } }
    switch (params.mode) {
        case 8u: { for (var i = 0; i < 8; i++) { c += 0.01; } }
        default: {}
    }
    if (params.x == 1u) { for (var i = 0; i < 8; i++) { c += 0.02; } }
    if (params.y == 1u) { for (var i = 0; i < 8; i++) { c += 0.03; } }
    return vec4<f32>(vec3<f32>(fract(c)), 1.0);
}";
    let mut w = InProcessWorker::new(FakeMeter::new(1.0)).await;
    let c = w.compile(def, body).await;
    let cost = c.compiled().unwrap().cost.clone().unwrap();
    assert!(
        cost.advice.iter().any(|a| a.contains("at most 12 corners")),
        "{:?}",
        cost.advice
    );
    assert!(
        !cost.advice.iter().any(|a| a.contains("ran out")),
        "{:?}",
        cost.advice
    );
}

/// A frame that fails names the step and the size it failed at: the preview's step, or an export size's frames.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_failed_frame_names_its_step_and_size() {
    let mut w = InProcessWorker::new(FakeMeter::new(1.0).fail_at((1280, 720))).await;
    let reason = w.compile(draft_def(), WAVES_BODY).await.failure();
    assert!(
        reason
            .as_deref()
            .is_some_and(|r| r.contains("stepping up to 1280×720")),
        "{reason:?}"
    );
    let mut w = InProcessWorker::new(FakeMeter::new(1.0).fail_at((1920, 1080))).await;
    let reason = w.compile(draft_def(), WAVES_BODY).await.failure();
    assert!(
        reason
            .as_deref()
            .is_some_and(|r| r.contains("at 1920×1080")),
        "{reason:?}"
    );
}

/// A timed frame whose timestamps never arrived (its map left pending) drops the timer, so the next frame isn't
/// failed by that map, and makes a new one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_timer_whose_reading_failed_is_made_again() {
    let mut h = headless_alone().await;
    if !h.device().timestamps_trusted() {
        println!("no trusted timestamps on this device: nothing to check");
        return;
    }
    let layer = draft(draft_def(), WAVES_BODY);
    layer.prepare(&mut h, true, false);
    let project = std::sync::Arc::new(layer.project());
    let (at, format) = (layer.middle(), layer.format);
    h.render_timed(project.clone(), at, format, (64, 64))
        .await
        .unwrap();
    h.fail_next_timer_read();
    let failed = h
        .render_timed(project.clone(), at, format, (64, 64))
        .await
        .unwrap();
    assert!(!failed.timestamps, "the wall clock");
    h.render_timed(project.clone(), at, format, (64, 64))
        .await
        .expect("the next frame isn't failed by the pending map");
}
