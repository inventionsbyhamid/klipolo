//! The guard's remaining edges. Conservative where
//! tracking is uncertain: the setting is costly at both ends.

use std::sync::mpsc;
use std::time::Duration;

use klipolo_engine::registry::ShaderKind;
use klipolo_engine::registry::guard::{End, check};
use klipolo_model::contrib::{ContribDef, ContribKind};

const SETTINGS: &str = r##"[{"key":"k","label":"K","type":"float","min":0,"max":100,"default":50},
    {"key":"fast","label":"F","type":"bool","default":false},
    {"key":"steps","label":"N","type":"int","min":1,"max":64,"default":8},
    {"key":"bias","label":"B","type":"float","min":-1,"max":1,"default":0},
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

/// `check` on a 2 MiB thread (the main process's blocking-pool stack), given a few seconds.
fn on_small_stack(body: String) -> Vec<&'static str> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let _ = tx.send(codes(&body));
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(10))
        .expect("check finished")
}

/// `unit` repeated `n` times, `per_line` to a line.
fn lines(unit: &str, n: usize, per_line: usize) -> String {
    (0..n)
        .map(|k| {
            format!(
                "{unit}{}",
                if k % per_line == per_line - 1 {
                    "\n"
                } else {
                    " "
                }
            )
        })
        .collect()
}

#[test]
fn a_setting_copied_past_the_round_cap_is_costly_at_both_ends() {
    // Stores apply in statement order, so a straight-line chain settles in one round and keeps its
    // end; a chain carried around a loop moves one `var` a round, and 24 pass the round cap.
    let copies: String = (1..16)
        .map(|k| format!("    var v{k} = v{};\n", k - 1))
        .collect();
    let body = effect(&format!(
        "    var v0 = params.steps;\n{copies}    var c = uv.x;\n    for (var i = 0; i < v15; i++) {{ c += sin(c); }}\n    return vec4<f32>(c);"
    ));
    assert_eq!(end(&body, "steps"), Some(End::Max));
    let declared: String = (1..24).map(|k| format!("    var v{k} = 0.0;\n")).collect();
    let carried: String = (1..24)
        .rev()
        .map(|k| format!("v{k} = v{}; ", k - 1))
        .collect();
    let body = effect(&format!(
        "    var v0 = f32(params.steps);\n{declared}    for (var j = 0; j < 30; j++) {{\n        {carried}\n    }}
    var c = uv.x;\n    for (var i = 0; f32(i) < v23; i++) {{ c += sin(c); }}\n    return vec4<f32>(c);"
    ));
    assert_eq!(end(&body, "steps"), Some(End::Both));
}

#[test]
fn a_call_that_discards_is_a_discard() {
    let body = "fn bail() { discard; }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = uv.x;
    if (params.fast == 1u) { bail(); }
    for (var i = 0; i < 32; i++) { c += sin(c + f32(i)); }
    return vec4<f32>(c);
}";
    assert_eq!(end(body, "fast"), Some(End::Both));
}

#[test]
fn a_branch_around_heavy_straight_line_work_is_costly() {
    let products = lines("m = m * m * m * m * m * m * m * m;", 300, 8);
    let body = effect(&format!(
        "    var m = mat4x4<f32>(vec4<f32>(uv, 0.0, 1.0), vec4<f32>(1.0), vec4<f32>(0.5), vec4<f32>(0.25));
    if (params.k > 50.0) {{
{products}    }}
    return m[0];"
    ));
    // Deciding more than 1,024 operations makes a setting costly at both ends.
    assert_eq!(end(&body, "k"), Some(End::Both));
}

#[test]
fn templates_cant_hide_blocks() {
    let open = lines("if c > 0.0 { var a: array<f32,(4>>1)>;", 120, 6);
    let close = lines("}", 120, 40);
    let body = effect(&format!(
        "    let c = uv.x;\n{open}\n{close}\n    return vec4<f32>(c);"
    ));
    assert!(on_small_stack(body).contains(&"scan.nesting"));
}

#[test]
fn templates_cant_hide_an_else_if_chain() {
    let links = lines(
        "else if array<f32,(4>>1)>()[0] > 0.0 && f32(texture_x < y) > 0.5 { c = 1.0; }",
        400,
        4,
    );
    let body = effect(&format!(
        "    let texture_x = uv.x;\n    let y = uv.y;\n    var c = 0.0;\n    if c > 0.0 {{ c = 1.0; }}\n{links}\n    return vec4<f32>(c);"
    ));
    assert!(
        body.len() <= klipolo_engine::registry::guard::MAX_SOURCE_BYTES,
        "{}",
        body.len()
    );
    assert!(on_small_stack(body).contains(&"scan.nesting"));
}

#[test]
fn a_value_named_like_a_texture_type_isnt_a_template() {
    let body = effect(
        "    let texture_x = uv.x;\n    let y = uv.y;\n    if (f32(texture_x < y) > 0.5) { return vec4<f32>(1.0); }\n    return vec4<f32>(0.0);",
    );
    assert_eq!(codes(&body), Vec::<&str>::new());
    // 127 pairs, 762 links (3× the cap): the old prefix match showed only 254 of them.
    let pairs = lines("texture_a < b && texture_a > b &&", 127, 6);
    let chain = effect(&format!(
        "    let texture_a = uv.x;\n    let b = uv.y;\n    let c = {pairs} true;\n    return vec4<f32>(f32(c));"
    ));
    assert!(on_small_stack(chain).contains(&"scan.chain"));
}

#[test]
fn a_returned_constant_is_a_copy() {
    let body = "const K = array<vec4<f32>, 256>();
fn table() -> array<vec4<f32>, 256> { return K; }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < 4096; i++) { let t = table(); acc += t[i % 256]; }
    return acc;
}";
    assert_eq!(
        codes(body),
        ["cost.static"],
        "1,024 numbers copied 4,096 times"
    );
}

#[test]
fn bool_casts_and_nan_making_maths_keep_no_direction() {
    let bound = |expr: &str| {
        effect(&format!(
            "    var c = uv.x;\n    for (var i = 0; f32(i) < {expr}; i++) {{ c += sin(c); }}\n    return vec4<f32>(c);"
        ))
    };
    assert_eq!(
        end(&bound("f32(bool(params.bias)) * 100.0"), "bias"),
        Some(End::Both)
    );
    assert_eq!(
        end(&bound("sqrt(params.bias) * 100.0"), "bias"),
        Some(End::Both)
    );
}

#[test]
fn the_size_cap_counts_a_matrixs_numbers() {
    let body = effect("    var big: array<mat4x4<f32>, 128>;\n    return big[0][0];");
    assert!(codes(&body).contains(&"rule.size"), "2,048 numbers");
}

#[test]
fn an_exponents_minus_isnt_a_link() {
    let entries = lines("vec2<f32>(1.5e-3, 2.5e-1),", 140, 8);
    let body = format!(
        "const T = array<vec2<f32>, 140>(\n{entries});\n{}",
        effect("    return vec4<f32>(T[0], uv);")
    );
    assert_eq!(codes(&body), Vec::<&str>::new());
}

#[test]
fn a_colour_through_a_helpers_pointer_is_probed_naming_the_helper() {
    let body = "fn rotate(p: ptr<function, vec2<f32>>, a: f32) {
    *p = vec2<f32>(cos(a) * (*p).x - sin(a) * (*p).y, sin(a) * (*p).x + cos(a) * (*p).y);
}
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var p = uv - params.tint.xy;
    rotate(&p, 0.5);
    return vec4<f32>(p, 0.0, 1.0);
}";
    assert_eq!(end(body, "tint"), Some(End::Probe));
    let c = check(&def(), ShaderKind::Filter, body).unwrap();
    let tint = c.notes.iter().find(|d| d.code == "cost.probe").unwrap();
    assert!(
        tint.message.contains("`rotate`") && tint.hint.as_deref().unwrap().contains("by value"),
        "{tint:?}"
    );
}
