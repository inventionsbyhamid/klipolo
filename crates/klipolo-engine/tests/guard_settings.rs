//! Every setting that decides how much work a pixel does is found, with its costly end, or the shader is refused
//! The measurement times only these settings' ends, and the envelope leaves the others free, so one that's missed runs
//! unmeasured.

use klipolo_engine::registry::ShaderKind;
use klipolo_engine::registry::guard::{CostlyParam, End, Severity, check};
use klipolo_model::contrib::{ContribDef, ContribKind};

fn generator(params: &str) -> ContribDef {
    ContribDef {
        params: serde_json::from_str(params).unwrap(),
        ..ContribDef::new(
            "lib.k3f9q2ma/generator.test.v1",
            ContribKind::Generator,
            "Test",
        )
    }
}

const SETTINGS: &str = r#"[{"key":"fast","label":"F","type":"bool","default":false},
    {"key":"skip","label":"S","type":"float","min":0,"max":1,"default":0},
    {"key":"cheap","label":"C","type":"bool","default":false},
    {"key":"mode","label":"M","type":"choice","options":["a","b"],"default":"a"},
    {"key":"steps","label":"N","type":"int","min":1,"max":64,"default":8},
    {"key":"k","label":"K","type":"float","min":0,"max":100,"default":50}]"#;

/// The costly end of each setting the body uses.
fn ends(body: &str) -> Vec<(String, End)> {
    let c = check(&generator(SETTINGS), ShaderKind::Generator, body).unwrap();
    c.facts
        .costly_params
        .into_iter()
        .map(|p| (p.key, p.costly_end))
        .collect()
}

const HEAVY: &str = "for (var i = 0; i < 32; i++) { c += sin(c + f32(i)); }";

#[test]
fn a_setting_that_skips_the_work_early_is_costly() {
    let early_return = format!(
        "fn effect(uv: vec2<f32>) -> vec4<f32> {{ var c = uv.x; if (params.fast == 1u) {{ return vec4<f32>(c); }} {HEAVY} return vec4<f32>(c); }}"
    );
    assert_eq!(
        ends(&early_return),
        [("fast".to_string(), End::Both)],
        "an early exit in a function with work: both ends"
    );
    let discard = format!(
        "fn effect(uv: vec2<f32>) -> vec4<f32> {{ var c = uv.x; if (params.skip > 0.5) {{ discard; }} {HEAVY} return vec4<f32>(c); }}"
    );
    assert_eq!(ends(&discard), [("skip".to_string(), End::Both)]);
    let skip_step = "fn effect(uv: vec2<f32>) -> vec4<f32> { var c = uv.x;
        for (var i = 0; i < 32; i++) { if (params.cheap == 1u) { continue; } c += sin(c + f32(i)); }
        return vec4<f32>(c); }";
    assert_eq!(ends(skip_step), [("cheap".to_string(), End::Both)]);
    let switch_return = format!(
        "fn effect(uv: vec2<f32>) -> vec4<f32> {{ var c = uv.x; switch (params.mode) {{ case 1u: {{ return vec4<f32>(c); }} default: {{}} }} {HEAVY} return vec4<f32>(c); }}"
    );
    assert_eq!(ends(&switch_return), [("mode".to_string(), End::Both)]);
}

#[test]
fn a_colour_or_point_that_decides_the_work_is_probed_with_a_note_where() {
    // Never refused: measured at the probe set, with a note on the line that decides and a hint for its kind.
    let def = generator(
        r##"[{"key":"tint","label":"T","type":"color","default":"#FF0000"},
            {"key":"center","label":"C","type":"point","default":{"x":0.5,"y":0.5}}]"##,
    );
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = 0.0;
    for (var i = 0; f32(i) < params.tint.a * 1000.0; i++) { c += 0.001; }
    if (distance(uv, params.center) > 0.3) { for (var j = 0; j < 8; j++) { c += sin(c); } }
    return vec4<f32>(c);
}";
    let c = check(&def, ShaderKind::Generator, body).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(
        ends_of(&c.facts.costly_params),
        [
            ("tint".to_string(), End::Probe),
            ("center".to_string(), End::Probe)
        ]
    );
    let note = |key: &str| {
        c.notes
            .iter()
            .find(|d| d.code == "cost.probe" && d.message.contains(&format!("`{key}`")))
            .unwrap_or_else(|| panic!("{key}: {:?}", c.notes))
    };
    let (tint, center) = (note("tint"), note("center"));
    assert_eq!(
        (tint.severity, tint.line, center.line),
        (Severity::Info, 3, 4)
    );
    assert!(tint.message.contains("probe colours") && center.message.contains("probe points"));
    assert!(
        center
            .source_line
            .as_deref()
            .unwrap()
            .contains("distance(uv, params.center)")
    );
    assert!(
        center
            .hint
            .as_deref()
            .unwrap()
            .contains("this point reaches"),
        "{center:?}"
    );
    assert!(
        tint.hint
            .as_deref()
            .unwrap()
            .contains("this colour reaches"),
        "{tint:?}"
    );
}

fn ends_of(costly: &[CostlyParam]) -> Vec<(String, End)> {
    costly
        .iter()
        .map(|p| (p.key.clone(), p.costly_end))
        .collect()
}

#[test]
fn a_helper_writing_through_a_pointer_carries_the_setting() {
    let body = "fn fetch_steps(n: ptr<function, i32>) { *n = i32(params.steps); }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var n = 0;
    fetch_steps(&n);
    var c = uv.x;
    for (var i = 0; i < n; i++) { c += sin(c); }
    return vec4<f32>(c);
}";
    // A setting that reaches a pointer handed to a call is costly at both ends.
    assert_eq!(ends(body), [("steps".to_string(), End::Both)]);
}

#[test]
fn a_count_that_a_setting_ends_is_costly_at_both_ends_where_its_reused() {
    // The first loop is costliest at a high `k`, the second at a low one.
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var n = 0;
    for (var j = 0; j < 100; j++) {
        if (f32(j) >= params.k) { break; }
        n += 1;
    }
    var c = uv.x;
    for (var i = 0; i < 100 - n; i++) { c += sin(c); }
    return vec4<f32>(c);
}";
    assert_eq!(ends(body), [("k".to_string(), End::Both)]);
}
