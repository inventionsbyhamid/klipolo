//! The work a setting decides is tallied: the arms and early exits each setting reaches, times the trips of the loops
//! around them, summed over branches and calls. A setting that decides more than 1,024 operations is costly at both
//! ends. With smaller edge cases and the type-depth rule.

use klipolo_engine::registry::ShaderKind;
use klipolo_engine::registry::guard::{End, check};
use klipolo_model::contrib::{ContribDef, ContribKind};

const SETTINGS: &str = r##"[{"key":"k","label":"K","type":"float","min":0,"max":100,"default":50},
    {"key":"fast","label":"F","type":"bool","default":false},
    {"key":"steps","label":"N","type":"int","min":1,"max":64,"default":8},
    {"key":"tint","label":"T","type":"color","default":"#FF0000"}]"##;

fn def(kind: ContribKind) -> ContribDef {
    let id = match kind {
        ContribKind::Generator => "lib.k3f9q2ma/generator.test.v1",
        _ => "lib.k3f9q2ma/effect.test.v1",
    };
    ContribDef {
        params: serde_json::from_str(SETTINGS).unwrap(),
        ..ContribDef::new(id, kind, "Test")
    }
}

fn effect(lines: &str) -> String {
    format!("fn effect(uv: vec2<f32>) -> vec4<f32> {{\n{lines}\n}}\n")
}

/// The costly end of `key` in a filter, or None when it isn't costly.
fn end(body: &str, key: &str) -> Option<End> {
    end_in(ContribKind::Effect, ShaderKind::Filter, body, key)
}

fn end_in(kind: ContribKind, shader: ShaderKind, body: &str, key: &str) -> Option<End> {
    let c = check(&def(kind), shader, body).unwrap_or_else(|d| panic!("{d:?}"));
    c.facts
        .costly_params
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.costly_end)
}

fn codes(body: &str) -> Vec<&'static str> {
    match check(&def(ContribKind::Effect), ShaderKind::Filter, body) {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(|d| d.code).collect(),
    }
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

const MAT: &str = "var m = mat4x4<f32>(vec4<f32>(uv, 0.0, 1.0), vec4<f32>(1.0), vec4<f32>(0.5), vec4<f32>(0.25));";
/// About 590 operations: seven 4×4 products of 64 multiply-adds, eight loads and a store.
const PRODUCTS: &str = "m = m * m * m * m * m * m * m * m;";

#[test]
fn a_light_branch_in_a_loop_counts_the_loops_trips() {
    let body = effect(&format!(
        "    {MAT}\n    for (var i = 0; i < 300; i++) {{\n        if (params.k > 50.0) {{ {PRODUCTS} }}\n    }}\n    return m[0];"
    ));
    assert_eq!(
        end(&body, "k"),
        Some(End::Both),
        "about 590 operations × 300 trips"
    );
}

#[test]
fn light_branches_add_up() {
    let arms = lines(&format!("if (params.k > 50.0) {{ {PRODUCTS} }}"), 300, 5);
    let body = effect(&format!("    {MAT}\n{arms}    return m[0];"));
    assert_eq!(
        end(&body, "k"),
        Some(End::Both),
        "300 arms of about 590 operations"
    );
}

#[test]
fn a_helpers_branch_counts_the_trips_around_the_call() {
    let body = format!(
        "fn step(m0: mat4x4<f32>, k: f32) -> mat4x4<f32> {{
    var m = m0;
    if (k > 50.0) {{ {PRODUCTS} }}
    return m;
}}
fn effect(uv: vec2<f32>) -> vec4<f32> {{
    {MAT}
    for (var i = 0; i < 250; i++) {{ m = step(m, params.k); }}
    return m[0];
}}"
    );
    assert_eq!(
        end(&body, "k"),
        Some(End::Both),
        "about 590 operations × 250 calls"
    );
}

#[test]
fn an_early_return_before_straight_line_work_is_costly() {
    let products = lines(PRODUCTS, 300, 8);
    // A generator without loops or samples.
    let body = effect(&format!(
        "    {MAT}\n    if (params.k < 60.0) {{ return m[0]; }}\n{products}    return m[0];"
    ));
    assert_eq!(
        end_in(ContribKind::Generator, ShaderKind::Generator, &body, "k"),
        Some(End::Both)
    );
    // The same in a helper without work, called by a filter that samples.
    let helper = format!(
        "fn shade(m0: mat4x4<f32>, k: f32) -> vec4<f32> {{
    var m = m0;
    if (k < 60.0) {{ return m[0]; }}
{products}    return m[0];
}}
fn effect(uv: vec2<f32>) -> vec4<f32> {{
    let a = kl_sample_a(uv);
    return shade(mat4x4<f32>(a, a, a, a), params.k);
}}"
    );
    assert_eq!(end(&helper, "k"), Some(End::Both));
}

#[test]
fn a_colour_deciding_a_light_branch_is_still_allowed() {
    // A few operations, 8 times: far under 1,024.
    let body = effect(
        "    var c = uv.x;
    for (var i = 0; i < 8; i++) { if (params.tint.a > 0.5) { c = c * 0.5 + 0.1; } c += sin(c); }
    return vec4<f32>(c);",
    );
    assert_eq!(codes(&body), Vec::<&str>::new());
}

#[test]
fn sixteen_chained_vars_settle_without_the_fallback() {
    let chain: String = (1..16)
        .map(|k| format!("    var v{k} = v{} * 1.01;\n", k - 1))
        .collect();
    let body = effect(&format!(
        "    var v0 = uv.x;\n{chain}    var c = v15;\n    for (var i = 0; i < 8; i++) {{ c += sin(c); }}
    return vec4<f32>(c * params.tint.rgb * (params.k / 100.0), 1.0);"
    ));
    let c = check(&def(ContribKind::Effect), ShaderKind::Filter, &body)
        .unwrap_or_else(|d| panic!("the colour was refused: {d:?}"));
    assert_eq!(
        c.facts.costly_params,
        [],
        "k and tint only colour the pixel"
    );
}

#[test]
fn an_exponents_plus_and_an_empty_fraction_arent_links() {
    let entries = lines("vec2<f32>(1.5e+3, 1.e-3),", 140, 8);
    let body = format!(
        "const T = array<vec2<f32>, 140>(\n{entries});\n{}",
        effect("    return vec4<f32>(T[0], uv);")
    );
    assert_eq!(codes(&body), Vec::<&str>::new());
}

#[test]
fn a_discard_two_helpers_down_is_a_discard() {
    let body = "fn inner() { discard; }
fn outer() { inner(); }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = uv.x;
    if (params.fast == 1u) { outer(); }
    for (var i = 0; i < 32; i++) { c += sin(c + f32(i)); }
    return vec4<f32>(c);
}";
    assert_eq!(end(body, "fast"), Some(End::Both));
}

#[test]
fn a_colour_through_a_pointer_that_also_decides_a_loop_gets_both_notes() {
    let body = "fn rotate(p: ptr<function, vec2<f32>>, a: f32) {
    *p = vec2<f32>(cos(a) * (*p).x - sin(a) * (*p).y, sin(a) * (*p).x + cos(a) * (*p).y);
}
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var p = uv - params.tint.xy;
    rotate(&p, 0.5);
    var c = 0.0;
    for (var i = 0; f32(i) < params.tint.a * 10.0; i++) { c += 0.1; }
    return vec4<f32>(p, c, 1.0);
}";
    // Measured at probes, never refused, with a note for each reason.
    let c = check(&def(ContribKind::Effect), ShaderKind::Filter, body)
        .unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(end(body, "tint"), Some(End::Probe));
    let tint: Vec<&str> = c
        .notes
        .iter()
        .filter(|d| d.code == "cost.probe" && d.message.contains("`tint`"))
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        tint.len() == 2 && tint.iter().any(|m| m.contains("`rotate`")),
        "{tint:?}"
    );
}

#[test]
fn a_colour_passed_on_by_value_is_no_pointer() {
    let body = "fn lin(c: vec3<f32>) -> vec3<f32> { return kl_to_linear(c); }
fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(lin(params.tint.rgb), 1.0); }";
    assert_eq!(codes(body), Vec::<&str>::new());
}

#[test]
fn types_nest_at_most_32_deep_alias_chains_included() {
    let aliases = |n: usize| {
        let mut s = String::from("alias A0 = array<f32, 1>;\n");
        for k in 1..n {
            s.push_str(&format!("alias A{k} = array<A{}, 1>;\n", k - 1));
        }
        s + &effect(&format!(
            "    var x: A{};\n    return vec4<f32>(uv, 0.0, 1.0);",
            n - 1
        ))
    };
    assert_eq!(codes(&aliases(32)), Vec::<&str>::new(), "32 deep");
    assert!(codes(&aliases(33)).contains(&"rule.nesting"), "33 deep");
}

#[test]
fn a_zoom_blur_around_a_point_isnt_refused_for_the_preludes_early_return() {
    let def = ContribDef {
        params: serde_json::from_str(
            r##"[{"key":"center","label":"C","type":"point","default":{"x":0.5,"y":0.5}},
                {"key":"amount","label":"A","type":"float","min":0,"max":1,"default":0.3},
                {"key":"tint","label":"T","type":"color","default":"#FF0000"}]"##,
        )
        .unwrap(),
        ..ContribDef::new("lib.k3f9q2ma/effect.zoom.v1", ContribKind::Effect, "Zoom")
    };
    // `kl_unpremultiply` returns early on a clear texel: a prelude helper's exit counts only its own arms, so 64 taps
    // placed by a point setting decide no work.
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < 64; i++) {
        let p = mix(uv, params.center, f32(i) / 64.0 * params.amount);
        acc += kl_unpremultiply(kl_sample_a(p));
    }
    return kl_premultiply(acc / 64.0);
}";
    let c = check(&def, ShaderKind::Filter, body).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(
        c.facts.costly_params,
        [],
        "where the taps fall decides no work"
    );
    // A colour or point that branches in the loop decides about 44 operations × 64 trips: measured at probes, never
    // refused.
    for (cond, key) in [
        ("params.tint.a > 0.5", "tint"),
        ("params.center.x > 0.5", "center"),
    ] {
        let body = format!(
            "fn effect(uv: vec2<f32>) -> vec4<f32> {{
    var v = vec4<f32>(uv, 0.0, 1.0);
    for (var i = 0; i < 64; i++) {{ if ({cond}) {{ v = v * 0.5 + sin(v) * 0.25 + cos(v) * 0.125; }} v += vec4<f32>(0.01); }}
    return v;
}}"
        );
        let c = check(&def, ShaderKind::Filter, &body).unwrap_or_else(|d| panic!("{cond}: {d:?}"));
        let probed: Vec<_> = c
            .facts
            .costly_params
            .iter()
            .map(|p| (p.key.as_str(), p.costly_end))
            .collect();
        assert_eq!(probed, [(key, End::Probe)], "{cond}");
    }
}
