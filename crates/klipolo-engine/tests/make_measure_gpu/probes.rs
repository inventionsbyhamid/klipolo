//! The probed settings and the numbers measured inside their range.

use klipolo_engine::make::measure::FakeMeter;

use crate::support::*;

/// Probed settings move together by probe index, so the probe cap limits combinations and never
/// drops a value. Two points fill more than the 16 probes a setting-by-setting list would hold; the colour after them
/// is still probed, and its costly white counts.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn every_probed_setting_is_probed_whatever_the_cap() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = vec3<f32>(0.0);
    if (distance(uv, params.a) < 0.05) {
        for (var i = 0; i < 2; i++) { c += vec3<f32>(0.01); }
    }
    if (distance(uv, params.b) < 0.05) {
        for (var i = 0; i < 2; i++) { c += vec3<f32>(0.01); }
    }
    if (params.tint.g > 0.9 && params.tint.b > 0.9) {
        for (var i = 0; i < 40; i++) { c += vec3<f32>(sin(f32(i) + uv.x)) * 0.01; }
    }
    return vec4<f32>(c, 1.0);
}";
    let def = draft_def_with(
        r##"[{"key":"a","label":"A","type":"point","default":{"x":0.25,"y":0.25}},
            {"key":"b","label":"B","type":"point","default":{"x":0.75,"y":0.25}},
            {"key":"tint","label":"T","type":"color","default":"#000000"}]"##,
    );
    let mut w = InProcessWorker::new(FakeMeter::new(0.2).per_trip(0.07)).await;
    let c = w.compile(def, body).await;
    let c = c.compiled().unwrap();
    let cost = c.cost.as_ref().unwrap();
    assert_eq!(
        cost.class,
        klipolo_engine::make::budget::CostClass::Over,
        "white, the colour's third probe, takes the 40-trip loop: {cost:?}"
    );
    assert!(
        w.measured_corners() <= 1 + 9,
        "at most the 9 grid cells' combinations"
    );
}

/// A number costly at both ends is also measured at 25, 50 and 75 % of its
/// range, and the measured p95 decides the class; the static estimate no longer floors it. This generator's loop runs
/// only near the middle of `amount` (static estimate about 4 ms at preview size, which floored it Over), and measures
/// about 1 ms there.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_number_costly_at_both_ends_is_measured_in_its_middle_and_not_floored() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = 0.0;
    if (abs(params.amount - 0.5) < 0.1) {
        for (var i = 0; i < 96; i++) {
            c += sin(uv.x * f32(i) + c) * cos(uv.y + f32(i));
        }
    }
    return vec4<f32>(vec3<f32>(fract(c)), 1.0);
}";
    let def = draft_def_with(
        r#"[{"key":"amount","label":"A","type":"float","min":0,"max":1,"default":0}]"#,
    );
    let mut w = InProcessWorker::new(FakeMeter::new(0.2).per_trip(0.01)).await;
    let c = w.compile(def, body).await;
    let c = c.compiled().unwrap();
    let cost = c.cost.as_ref().unwrap();
    assert!(
        w.meter_frames_at_settings("amount", 0.5, (16, 16)) >= 1,
        "the middle of the range is measured"
    );
    assert_ne!(
        cost.class,
        klipolo_engine::make::budget::CostClass::Over,
        "{cost:?}"
    );
    assert!(
        cost.preview_ms.p95 > 0.9,
        "the middle's loop counts: {cost:?}"
    );
}
