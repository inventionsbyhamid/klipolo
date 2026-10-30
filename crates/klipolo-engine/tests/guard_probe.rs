//! A colour or point setting that decides how much work a pixel does is never refused for it: it is marked Probe, and
//! the effect is measured at the probe set. Common effects whose taps, radius or key a point or colour decides pass.

use klipolo_engine::registry::ShaderKind;
use klipolo_engine::registry::guard::check;
use klipolo_model::contrib::{ContribDef, ContribKind};

const SETTINGS: &str = r##"[{"key":"center","label":"C","type":"point","default":{"x":0.5,"y":0.5}},
    {"key":"light","label":"L","type":"point","default":{"x":0.5,"y":0.1}},
    {"key":"key","label":"K","type":"color","default":"#00FF00"},
    {"key":"amount","label":"A","type":"float","min":0,"max":1,"default":0.3},
    {"key":"threshold","label":"T","type":"float","min":0,"max":1,"default":0.6},
    {"key":"weight","label":"W","type":"float","min":0,"max":2,"default":0.5},
    {"key":"radius","label":"R","type":"float","min":0.01,"max":0.5,"default":0.2},
    {"key":"zoom","label":"Z","type":"float","min":1,"max":4,"default":2},
    {"key":"tolerance","label":"Tol","type":"float","min":0,"max":1,"default":0.3}]"##;

fn def() -> ContribDef {
    ContribDef {
        params: serde_json::from_str(SETTINGS).unwrap(),
        ..ContribDef::new("lib.k3f9q2ma/effect.probe.v1", ContribKind::Effect, "Probe")
    }
}

/// The settings measured at probes. Read from the facts' JSON, as an agent sees them.
fn probed(body: &str) -> Vec<String> {
    let c = check(&def(), ShaderKind::Filter, body).unwrap_or_else(|d| panic!("refused: {d:?}"));
    let costly = serde_json::to_value(&c.facts.costly_params).unwrap();
    costly
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["costlyEnd"] == "probe")
        .map(|p| p["key"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_weighted_zoom_blur_around_a_point_is_probed() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var i = 0; i < 64; i++) {
        let t = f32(i) / 64.0;
        let s = kl_sample_a(mix(uv, params.center, t * params.amount));
        let w = 1.0 - t;
        if (s.a > 0.0) { acc += s * w; wsum += w; }
    }
    return acc / max(wsum, 0.0001);
}";
    assert_eq!(probed(body), ["center"]);
}

#[test]
fn god_rays_with_a_bright_pass_are_probed() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var acc = vec4<f32>(0.0);
    var decay = 1.0;
    let delta = (uv - params.light) / 63.0;
    var p = uv;
    for (var i = 0; i < 63; i++) {
        p -= delta;
        let s = kl_sample_a(p);
        if (kl_luma(s.rgb) > params.threshold) { acc += s * decay * params.weight; }
        decay *= 0.96;
    }
    return kl_sample_a(uv) + acc / 63.0;
}";
    assert_eq!(probed(body), ["light"]);
}

#[test]
fn a_ripple_with_a_radius_is_probed() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let d = distance(uv, params.center);
    if (d > params.radius) { return kl_sample_a(uv); }
    let off = normalize(uv - params.center) * sin(d * 60.0 - kl.time * 6.0) * 0.01;
    return kl_sample_a(uv + off);
}";
    assert_eq!(probed(body), ["center"]);
}

#[test]
fn a_magnifier_is_probed() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let d = distance(uv, params.center);
    if (d < params.radius) {
        return kl_sample_a(params.center + (uv - params.center) / params.zoom);
    }
    return kl_sample_a(uv);
}";
    assert_eq!(probed(body), ["center"]);
}

#[test]
fn a_chroma_keys_early_return_is_probed() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = kl_sample_a(uv);
    if (distance(c.rgb, params.key.rgb) < params.tolerance) { return vec4<f32>(0.0); }
    return c;
}";
    assert_eq!(probed(body), ["key"]);
}

#[test]
fn a_nan_guard_on_a_point_is_probed() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let v = uv - params.center;
    let d = length(v);
    if (d < 0.0001) { return kl_sample_a(uv); }
    let off = v / d * sin(d * 40.0 - kl.time * 4.0) * 0.01 * params.amount;
    return kl_sample_a(uv + off);
}";
    assert_eq!(probed(body), ["center"]);
}
