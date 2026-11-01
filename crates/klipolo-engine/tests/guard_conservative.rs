//! The settings analysis is conservative and bounded. Where tracking is uncertain a setting is costly at both ends, so
//! the measurement times it at both.

use std::sync::mpsc;
use std::time::Duration;

use klipolo_engine::registry::ShaderKind;
use klipolo_engine::registry::guard::{End, check};
use klipolo_model::contrib::{ContribDef, ContribKind};

const SETTINGS: &str = r##"[{"key":"k","label":"K","type":"float","min":0,"max":100,"default":50},
    {"key":"fast","label":"F","type":"bool","default":false},
    {"key":"skip","label":"S","type":"float","min":0,"max":1,"default":0},
    {"key":"steps","label":"N","type":"int","min":1,"max":64,"default":8},
    {"key":"tint","label":"T","type":"color","default":"#FF0000"}]"##;

fn def() -> ContribDef {
    ContribDef {
        params: serde_json::from_str(SETTINGS).unwrap(),
        ..ContribDef::new("lib.k3f9q2ma/effect.test.v1", ContribKind::Effect, "Test")
    }
}

fn effect(lines: &str) -> String {
    format!("fn effect(uv: vec2<f32>) -> vec4<f32> {{\n{lines}\n}}\n")
}

/// The costly end of `key`, or None when it isn't costly.
fn end(body: &str, key: &str) -> Option<End> {
    let c = check(&def(), ShaderKind::Filter, body).unwrap_or_else(|d| panic!("{d:?}"));
    c.facts
        .costly_params
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.costly_end)
}

fn codes(body: &str) -> Vec<&'static str> {
    match check(&def(), ShaderKind::Filter, body) {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(|d| d.code).collect(),
    }
}

/// `check` on a 2 MiB thread (the main process's blocking-pool stack), given `limit` to finish.
fn within(limit: Duration, body: String) -> Vec<&'static str> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let _ = tx.send(codes(&body));
        })
        .unwrap();
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("check took over {limit:?}"))
}

#[test]
fn a_condition_shared_by_lets_is_read_once() {
    let mut lets = String::from("    let c0 = uv.x > params.k;\n");
    for k in 1..=40 {
        lets.push_str(&format!("    let c{k} = c{0} && c{0};\n", k - 1));
    }
    let body = effect(&format!(
        "{lets}    var a = 0.0;\n    if (c40) {{ for (var i = 0; i < 8; i++) {{ a += sin(a); }} }}\n    return vec4<f32>(a);"
    ));
    within(Duration::from_millis(100), body);
}

#[test]
fn a_doubling_call_chain_keeps_one_entry_per_setting() {
    let mut body = String::from(
        "fn g0() -> f32 { var a = 0.0; for (var i = 0; i < i32(params.steps); i++) { a += 1.0; } return a; }\n",
    );
    for k in 1..=27 {
        body.push_str(&format!(
            "fn g{k}() -> f32 {{ return g{0}() + g{0}(); }}\n",
            k - 1
        ));
    }
    body.push_str("fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(g27()); }\n");
    within(Duration::from_millis(100), body);
}

#[test]
fn a_continue_that_shapes_a_later_count_is_costly_at_both_ends() {
    let body = effect(
        "    var m = 0;
    for (var j = 0; j < 64; j++) { if (f32(j) < params.k) { continue; } m += 1; }
    var c = uv.x;
    for (var i = 0; i < 64 - m; i++) { c += sin(c); }
    return vec4<f32>(c);",
    );
    assert_eq!(end(&body, "k"), Some(End::Both));
}

#[test]
fn an_early_return_before_a_pointer_store_decides_the_store() {
    let body =
        "fn set_steps(n: ptr<function, i32>) { if (params.fast == 1u) { return; } *n = 4000; }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var n = 0;
    set_steps(&n);
    var c = uv.x;
    for (var i = 0; i < n; i++) { c += sin(c); }
    return vec4<f32>(c);
}";
    assert_eq!(end(body, "fast"), Some(End::Both));
}

#[test]
fn a_discard_in_a_helper_without_work_is_costly() {
    let body = "fn bail() { if (params.fast == 1u) { discard; } }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = uv.x;
    bail();
    for (var i = 0; i < 32; i++) { c += sin(c + f32(i)); }
    return vec4<f32>(c);
}";
    assert_eq!(end(body, "fast"), Some(End::Both));
}

#[test]
fn a_pointer_handed_on_after_a_setting_ended_loop_is_costly_at_both_ends() {
    let body = "fn heavy(n: ptr<function, i32>) -> f32 {
    var c = 0.0;
    for (var i = 0; i < 100 - *n; i++) { c += sin(c); }
    return c;
}
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var n = 0;
    for (var j = 0; j < 100; j++) { if (f32(j) >= params.k) { break; } n += 1; }
    return vec4<f32>(heavy(&n));
}";
    assert_eq!(end(body, "k"), Some(End::Both));
}

#[test]
fn a_setting_in_a_stores_index_is_costly_at_both_ends() {
    let body = effect(
        "    var n = array<i32, 2>(0, 0);
    n[u32(params.skip > 0.5)] = 4000;
    var c = uv.x;
    for (var i = 0; i < n[1]; i++) { c += sin(c); }
    return vec4<f32>(c);",
    );
    assert_eq!(end(&body, "skip"), Some(End::Both));
}

#[test]
fn integer_wrapping_on_the_way_to_a_bound_is_costly_at_both_ends() {
    let body = effect(
        "    let n = u32(i32(params.k) - 10);
    var c = uv.x;
    for (var i = 0u; i < n; i++) { c += sin(c); }
    return vec4<f32>(c);",
    );
    assert_eq!(
        end(&body, "k"),
        Some(End::Both),
        "k = 0…9 wraps to about 4.29e9"
    );
}

#[test]
fn matrix_products_count_their_multiply_adds() {
    let body = effect(
        "    var m = mat4x4<f32>(vec4<f32>(uv, 0.0, 1.0), vec4<f32>(1.0), vec4<f32>(0.5), vec4<f32>(0.25));
    for (var i = 0; i < 4096; i++) { m = m * m * m * m * m * m * m * m * m * m; }
    return m[0];",
    );
    assert_eq!(
        codes(&body),
        ["cost.static"],
        "9 products of 64 multiply-adds, 4,096 times"
    );
}

#[test]
fn a_non_ascii_member_chain_is_refused_before_naga() {
    // `℘` (U+2118) starts a WGSL identifier but isn't alphabetic: S0 … S1000, each holding the next.
    let mut body = String::from("struct S0 { x: f32 }\n");
    let mut line = String::new();
    for k in 1..=1000 {
        line.push_str(&format!("struct S{k} {{ ℘: S{} }} ", k - 1));
        if line.len() > 360 {
            body.push_str(&line);
            body.push('\n');
            line.clear();
        }
    }
    body.push_str(&line);
    let mut chain = String::from("\n    var s: S1000;\n    let v = s");
    let mut width = 0;
    for _ in 0..1000 {
        if width > 360 {
            chain.push_str("\n        ");
            width = 0;
        }
        chain.push_str(".℘");
        width += 4;
    }
    body.push_str(&format!(
        "fn effect(uv: vec2<f32>) -> vec4<f32> {{{chain}.x;\n    return vec4<f32>(v);\n}}\n"
    ));
    assert!(
        body.len() <= klipolo_engine::registry::guard::MAX_SOURCE_BYTES,
        "{}",
        body.len()
    );
    let found = within(Duration::from_secs(5), body);
    assert!(
        found.contains(&"scan.chain") && found.contains(&"rule.name"),
        "{found:?}"
    );
}

#[test]
fn an_unknown_bound_counts_all_of_the_counters_steps() {
    // About a hundred operations a step: under the cap at 1,024 steps, over it at 4,096.
    let step = "\n        c = sin(c) * 1.01 + cos(c) * 0.99;".repeat(12);
    let body = effect(&format!(
        "    var c = uv.x;\n    for (var i = 0; i < i32(uv.y * 100.0); i++) {{ {step}}}\n    return vec4<f32>(c);"
    ));
    let d = check(&def(), ShaderKind::Filter, &body).unwrap_err();
    let hint = d[0].hint.as_deref().unwrap_or("");
    assert!(
        d[0].code == "cost.static" && hint.contains("counts as 4096 steps"),
        "{d:?}"
    );
}

#[test]
fn a_gather_reads_four_texels() {
    let body = effect("    return textureGather(0, kl_a, kl_samp, uv);");
    let c = check(&def(), ShaderKind::Filter, &body).unwrap();
    assert_eq!(c.facts.samples_per_pixel, 4);
}

#[test]
fn the_module_rules_report_at_most_eight() {
    let lets: String = (0..12).map(|k| format!("    let kl{k} = 1.0;\n")).collect();
    let d = check(
        &def(),
        ShaderKind::Filter,
        &effect(&format!("{lets}    return vec4<f32>(uv, 0.0, 1.0);")),
    )
    .unwrap_err();
    assert!(
        d.len() == 8 && d.iter().all(|d| d.code == "rule.reserved"),
        "{d:?}"
    );
}

#[test]
fn a_constant_table_isnt_a_chain() {
    // 96 entries: three links each when templates and negative numbers count, past the 256 a statement may chain.
    let entries: Vec<String> = (0..96)
        .map(|k| format!("vec2<f32>(-0.{:03}, 0.{:03})", 100 + k, 900 - k))
        .collect();
    let mut table = String::new();
    for (k, e) in entries.iter().enumerate() {
        table.push_str(e);
        table.push_str(if k % 8 == 7 { ",\n    " } else { ", " });
    }
    let body = format!(
        "const DISK = array<vec2<f32>, 96>(\n    {table});\n{}",
        effect("    return vec4<f32>(DISK[0], uv);")
    );
    assert_eq!(codes(&body), Vec::<&str>::new());
}

#[test]
fn a_colour_reached_through_a_whole_params_copy_is_probed_and_says_what_to_change() {
    let body = "fn steps_of(p: Params) -> i32 { return i32(p.tint.a * 100.0); }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = uv.x;
    for (var i = 0; i < steps_of(params); i++) { c += sin(c); }
    return vec4<f32>(c);
}";
    let c = check(&def(), ShaderKind::Filter, body).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(end(body, "tint"), Some(End::Probe));
    let tint = c
        .notes
        .iter()
        .find(|d| d.code == "cost.probe" && d.message.contains("`tint`"))
        .unwrap();
    assert!(
        tint.hint
            .as_deref()
            .unwrap()
            .contains("instead of copying `params`"),
        "{tint:?}"
    );
}
