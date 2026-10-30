//! The render worker's measurement: every combination stepped from
//! 16² before any big frame, the worst corners narrowing the envelope, bools and choices measured at every value,
//! colours at their probes (`probes`), the time budget, the export stop, the worker's own GPU meter (`budget`),
//! fenced timing in batches (`fenced`), and every copy of a batch paying its passes where the renderer keeps results
//! results between frames (`caches`).
//! `FakeMeter` makes the numbers the same on any GPU (lavapipe in CI).

#[path = "make_measure_gpu/budget.rs"]
mod budget;
#[path = "make_measure_gpu/caches.rs"]
mod caches;
#[path = "make_measure_gpu/causes.rs"]
mod causes;
#[path = "make_measure_gpu/classes.rs"]
mod classes;
#[path = "make_measure_gpu/fenced.rs"]
mod fenced;
#[path = "make_measure_gpu/paths.rs"]
mod paths;
#[path = "make_measure_gpu/probes.rs"]
mod probes;
mod support;
use klipolo_engine::make::measure::FakeMeter;
use klipolo_engine::make::steps::{StepOutcome, stepped};
use support::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_slow_shader_stops_at_the_small_size_before_a_big_frame() {
    // 400 ns a pixel: 64² is 1.6 ms, 256² 26 ms, preview (1280 × 720) predicted 369 ms > 50: stops before it.
    let mut h = headless_alone().await;
    let out = stepped(
        &mut FakeMeter::new(400.0),
        &mut h,
        &draft(draft_def(), WAVES_BODY),
        &[(16, 16), (64, 64), (256, 256), (1280, 720)],
        50.0,
    )
    .await;
    assert!(
        matches!(
            out,
            StepOutcome::TooSlow {
                at: (1280, 720),
                ..
            }
        ),
        "{out:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_over_shader_stops_measuring_at_its_first_preview_frame() {
    // 30 ns a pixel: 256² is 2 ms (predicts 27.6 ms at preview, under 50), and the preview frame is 27.6 ms > 24 ms.
    let mut w = InProcessWorker::new(FakeMeter::new(30.0)).await;
    let c = w.compile(draft_def(), WAVES_BODY).await;
    let c = c.compiled().unwrap();
    assert_eq!(
        c.cost.as_ref().unwrap().class,
        klipolo_engine::make::budget::CostClass::Over
    );
    assert_eq!(
        w.meter_frames_at((1920, 1080)),
        0,
        "no 1080p frames for an Over shader"
    );
    assert_eq!(
        w.meter_frames_at((1280, 720)),
        1,
        "one preview frame proved it"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_worst_corner_is_measured_and_bounds_the_envelope() {
    // `loop_param.wgsl`: steps 1–64 bounds the loop; the fake meter scales with the loop's trips (FakeMeter::per_trip:
    // the host's counter, which counts the check that ends the loop too, so N steps are N + 1 trips). At preview size
    // (921,600 px): 0.2 ns a pixel is 0.18 ms, plus 0.07 ns a pixel per trip. No `resolution`, so anything over
    // Normal's 2 ms is Over. Default 8 → 0.77 ms (Normal); 64 → 4.4 ms (Over); 50 % of the way (36) → 2.6 ms
    // (Over); 25 % (22) → 1.7 ms (Normal): the envelope stops at 22 and the class is Normal.
    let mut w = InProcessWorker::new(FakeMeter::new(0.2).per_trip(0.07)).await;
    let def = draft_def_with(
        r#"[{"key":"steps","label":"Steps","type":"int","min":1,"max":64,"default":8}]"#,
    );
    let c = w.compile(def, LOOP_PARAM_BODY).await;
    let c = c.compiled().unwrap();
    assert_eq!(
        c.cost.as_ref().unwrap().class,
        klipolo_engine::make::budget::CostClass::Normal
    );
    let env = &c.cost.as_ref().unwrap().envelope;
    // 64 is stepped first (its 256² frame predicts 4.4 ms at preview, under 50 ms), then measured at preview size,
    // where it is over Normal: an Over corner gets no 1080p frames.
    assert_eq!(w.meter_frames_at_settings("steps", 64.0, (1920, 1080)), 0);
    assert!(
        w.meter_frames_at_settings("steps", 64.0, (256, 256)) >= 1,
        "stepped before any preview-size frame"
    );
    assert_eq!(env.ranges["steps"], (1.0, 22.0));
    // The defaults, 64, 36 and 22, then `steps` bounds the loop, so 25, 50 and 75 % of 1–22 too (6, 12, 17).
    assert_eq!(w.measured_corners(), 4 + 3);
    assert!(w.meter_frames_at_settings("steps", 12.0, (16, 16)) >= 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_worst_corner_too_slow_to_draw_is_never_drawn_big() {
    // `steps` 1–4096: cheap at its default 8; at 4096 the host's counter stops the loop at 4,096 trips a pixel. The
    // fake meter makes 4096 predict over 50 ms at preview size from its 256² frame, so it is stopped there.
    let mut w = InProcessWorker::new(FakeMeter::new(0.2).per_trip(0.1)).await;
    let def = draft_def_with(
        r#"[{"key":"steps","label":"Steps","type":"int","min":1,"max":4096,"default":8}]"#,
    );
    let c = w.compile(def, LOOP_PARAM_BODY).await;
    let c = c.compiled().unwrap();
    assert_eq!(
        w.meter_frames_at_settings("steps", 4096.0, (1280, 720)),
        0,
        "no preview-size frame"
    );
    assert_eq!(
        w.meter_frames_at_settings("steps", 4096.0, (1920, 1080)),
        0,
        "no 1080p frame"
    );
    assert!(
        c.cost.as_ref().unwrap().envelope.ranges["steps"].1 < 4096.0,
        "the envelope still narrows"
    );
    assert_eq!(
        w.failed_replies(),
        0,
        "answered as Over, never as a failure (strikes are the client's)"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn bools_and_choices_are_measured_at_every_value_and_never_narrowed() {
    // `branch_gated.wgsl`'s `fancy` (bool) and `mode` (choice of 3). At preview size `fancy` adds a 33-trip
    // loop (2.3 ms: Over), the third `mode` a 17-trip one (1.3 ms: Normal), and `detail` over 0.5 a 49-trip one, so its
    // range narrows to 0–0.5.
    let mut w = InProcessWorker::new(FakeMeter::new(0.2).per_trip(0.07)).await;
    let c = w.compile(branch_gated_def(), BRANCH_GATED_BODY).await;
    let env = &c.compiled().unwrap().cost.as_ref().unwrap().envelope;
    assert_eq!(
        w.measured_values_of("mode"),
        vec![0, 1, 2],
        "every option measured"
    );
    assert!(
        env.allowed["fancy"].contains(&0) && !env.allowed["fancy"].contains(&1),
        "true was Over: left out, not narrowed"
    );
    assert_eq!(
        env.allowed["mode"].iter().copied().collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(env.ranges["detail"], (0.0, 0.5));
}

/// Colour probes: a colour that decides work is measured at its probe set, never narrowed or
/// left out, and the worst probe counts: here pure red takes the costly arm, so the shader is Over.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_colour_that_decides_work_is_measured_at_its_probes_and_the_worst_counts() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = vec3<f32>(0.0);
    if (params.key.r > 0.9 && params.key.g < 0.1) {
        for (var i = 0; i < 40; i++) { c += vec3<f32>(sin(f32(i) + uv.x)) * 0.01; }
    }
    return vec4<f32>(c, 1.0);
}";
    let def = draft_def_with(r##"[{"key":"key","label":"K","type":"color","default":"#00FF00"}]"##);
    let mut w = InProcessWorker::new(FakeMeter::new(0.2).per_trip(0.07)).await;
    let c = w.compile(def, body).await;
    let c = c.compiled().unwrap();
    let cost = c.cost.as_ref().unwrap();
    assert_eq!(
        cost.class,
        klipolo_engine::make::budget::CostClass::Over,
        "{cost:?}"
    );
    assert!(
        cost.envelope.ranges.is_empty() && cost.envelope.allowed.is_empty(),
        "a colour is never bounded"
    );
    assert!(
        c.diagnostics.iter().any(|d| d.code == "cost.probe"),
        "the guard's note is forwarded"
    );
    assert_eq!(
        w.measured_corners(),
        1 + 4,
        "the defaults, then black, white, red and blue (green is the default)"
    );
}
