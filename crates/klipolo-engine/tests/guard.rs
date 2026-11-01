//! Source rules, static cost and diagnostics for WGSL that isn't built in: what can be
//! decided from the text and naga's IR before a GPU sees the shader, in diagnostics an agent can act on.

use klipolo_engine::registry::ShaderKind;
use klipolo_engine::registry::guard::{Severity, check};
use klipolo_model::contrib::{ContribDef, ContribKind};

fn wgsl(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/wgsl/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
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
const WAVES_PARAMS: &str = r##"[{"key":"density","label":"D","type":"float","min":0.5,"max":8,"default":2},
 {"key":"speed","label":"S","type":"float","min":0,"max":4,"default":1},
 {"key":"deepColor","label":"Deep","type":"color","default":"#0B2A4A"},
 {"key":"shallowColor","label":"Shallow","type":"color","default":"#2F8FB8"},
 {"key":"foam","label":"F","type":"float","min":0,"max":1,"default":0.4}]"##;

#[test]
fn a_good_generator_passes_with_its_facts() {
    let c = check(
        &generator(WAVES_PARAMS),
        ShaderKind::Generator,
        &wgsl("waves.wgsl"),
    )
    .unwrap();
    assert!(c.facts.animates, "reads kl.time");
    assert!(!c.facts.reads_layer);
    assert_eq!(c.facts.loops.len(), 1);
    assert_eq!(c.facts.loops[0].max_trips, Some(5));
    assert!(c.facts.ops_per_pixel < 2_000, "{}", c.facts.ops_per_pixel);
}

#[test]
fn a_glsl_ism_names_its_line_and_the_fix() {
    let d = check(
        &generator(WAVES_PARAMS),
        ShaderKind::Generator,
        &wgsl("glsl_mod.wgsl"),
    )
    .unwrap_err();
    let e = d.iter().find(|d| d.severity == Severity::Error).unwrap();
    assert_eq!(
        e.line, 9,
        "lines count from the author's body, not the prelude"
    );
    assert!(e.hint.as_deref().unwrap().contains("x - y * floor(x / y)"));
    assert!(e.source_line.as_deref().unwrap().contains("mod("));
}

#[test]
fn bindings_entry_points_private_vars_and_kl_names_are_refused() {
    for (file, code) in [
        ("binding.wgsl", "rule.binding"),
        ("reserved.wgsl", "rule.reserved"),
        ("private_var.wgsl", "rule.global"),
    ] {
        let d = check(&generator("[]"), ShaderKind::Generator, &wgsl(file)).unwrap_err();
        assert!(d.iter().any(|d| d.code == code), "{file}: {d:?}");
    }
}

#[test]
fn text_caps_hold_before_naga_parses() {
    let huge = "// x\n".repeat(9000);
    assert!(
        check(&generator("[]"), ShaderKind::Generator, &huge)
            .unwrap_err()
            .iter()
            .any(|d| d.code == "scan.size")
    );
    let d = check(&generator("[]"), ShaderKind::Generator, &wgsl("deep.wgsl")).unwrap_err();
    assert!(d.iter().any(|d| d.code == "scan.nesting"));
    let bidi = "fn effect(uv: vec2<f32>) -> vec4<f32> { let a\u{202E} = 1.0; return vec4<f32>(a\u{202E}); }";
    assert!(
        check(&generator("[]"), ShaderKind::Generator, bidi)
            .unwrap_err()
            .iter()
            .any(|d| d.code == "scan.invisible")
    );
    // Trojan Source: the overrides make the commented-out `return` display as code. Comments may hold no
    // direction characters.
    let trojan = "fn effect(uv: vec2<f32>) -> vec4<f32> {\n    /* \u{202E} } \u{2066} return vec4<f32>(1.0); \u{2069} \u{2066} */\n    return vec4<f32>(0.0);\n}";
    let d = check(&generator("[]"), ShaderKind::Generator, trojan).unwrap_err();
    assert!(
        d.iter().all(|d| d.code == "scan.invisible" && d.line == 2),
        "{d:?}"
    );
}

/// naga resolves a call to the module's own function before WGSL's, so a body's `fn max(vec3, vec3)` would replace the
/// one the prelude's `kl_to_linear` calls: the host's code calls only the host's functions, and `kl_fs` the entry.
#[test]
fn a_body_function_the_hosts_code_would_call_is_refused() {
    let shadow = "fn max(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {\n    return a;\n}\nfn effect(uv: vec2<f32>) -> vec4<f32> {\n    return vec4<f32>(kl_to_linear(vec3<f32>(-1.0)), 1.0);\n}";
    let d = check(&generator("[]"), ShaderKind::Generator, shadow).unwrap_err();
    assert_eq!((d[0].code, d[0].line), ("rule.shadow", 1), "{d:?}");
    assert!(d[0].message.contains("`max`"), "{}", d[0].message);
    // The body's own helpers, and the entry the host's `kl_fs` calls, are the body's to name.
    let helper = "fn lift(a: vec3<f32>) -> vec3<f32> {\n    return a;\n}\nfn effect(uv: vec2<f32>) -> vec4<f32> {\n    return vec4<f32>(lift(vec3<f32>(uv, 0.0)), 1.0);\n}";
    check(&generator("[]"), ShaderKind::Generator, helper).unwrap();
    // A constant of a built-in's name breaks the prelude's call to it: the refusal says why.
    let constant =
        "const pow = 2.0;\nfn effect(uv: vec2<f32>) -> vec4<f32> {\n    return vec4<f32>(pow);\n}";
    let d = check(&generator("[]"), ShaderKind::Generator, constant).unwrap_err();
    assert_eq!(d[0].code, "rule.shadow", "{d:?}");
}

#[test]
fn a_shader_too_costly_to_run_is_refused_before_the_gpu_with_advice() {
    let d = check(
        &generator("[]"),
        ShaderKind::Generator,
        &wgsl("raymarch.wgsl"),
    )
    .unwrap_err();
    let e = d.iter().find(|d| d.code == "cost.static").unwrap();
    assert!(e.message.contains("operations per pixel"));
    assert!(e.hint.as_deref().unwrap().contains("constant bound"));
}

#[test]
fn animation_is_read_by_member_name_not_position() {
    // `frame_direct.wgsl` reads `kl.frame` itself (member 3 at the base); `seed_only.wgsl` reads only `kl.seed` (member 4).
    let direct = check(
        &generator("[]"),
        ShaderKind::Generator,
        &wgsl("frame_direct.wgsl"),
    )
    .unwrap();
    assert!(direct.facts.animates, "kl.frame changes every frame");
    let seed = check(
        &generator("[]"),
        ShaderKind::Generator,
        &wgsl("seed_only.wgsl"),
    )
    .unwrap();
    assert!(!seed.facts.animates, "kl.seed is fixed per layer");
    // Copying the whole `kl` struct counts as reading everything in it.
    let whole = "fn effect(uv: vec2<f32>) -> vec4<f32> { let f = kl; return vec4<f32>(uv, f.px * 0.0, 1.0); }";
    assert!(
        check(&generator("[]"), ShaderKind::Generator, whole)
            .unwrap()
            .facts
            .animates
    );
}

#[test]
fn a_setting_that_bounds_a_loop_is_named_with_its_costly_end() {
    // `loop_param.wgsl`: `for (var i = 0; i < i32(params.steps); i++)` with steps 1–64.
    let def =
        generator(r#"[{"key":"steps","label":"Steps","type":"int","min":1,"max":64,"default":8}]"#);
    let c = check(&def, ShaderKind::Generator, &wgsl("loop_param.wgsl")).unwrap();
    assert_eq!(c.facts.loops[0].bound_param.as_deref(), Some("steps"));
    assert_eq!(c.facts.costly_params[0].key, "steps");
    assert_eq!(
        c.facts.costly_params[0].costly_end,
        klipolo_engine::registry::guard::End::Max
    );
}

#[test]
fn a_setting_that_gates_a_loop_through_a_branch_is_costly_too() {
    // `if (params.detail > 0.5) { for (var i = 0; i < 48; i++) { … kl_sample_a … } }`, and a bool `fancy` gating a
    // second loop, and `switch (params.mode)` whose case 2 holds a loop.
    let def = generator(
        r#"[{"key":"detail","label":"D","type":"float","min":0,"max":1,"default":0},
                           {"key":"fancy","label":"F","type":"bool","default":false},
                           {"key":"mode","label":"M","type":"choice","options":["a","b","c"],"default":"a"}]"#,
    );
    let c = check(&def, ShaderKind::Generator, &wgsl("branch_gated.wgsl")).unwrap();
    let end = |k: &str| {
        c.facts
            .costly_params
            .iter()
            .find(|p| p.key == k)
            .map(|p| p.costly_end)
    };
    use klipolo_engine::registry::guard::End;
    assert_eq!(
        (end("detail"), end("fancy"), end("mode")),
        (Some(End::Max), Some(End::Max), Some(End::Both))
    );
}

#[test]
fn the_entry_function_must_have_the_kinds_signature() {
    let body = "fn effect(uv: vec2<f32>, t: f32) -> vec4<f32> { return vec4<f32>(uv, t, 1.0); }";
    let d = check(&generator("[]"), ShaderKind::Generator, body).unwrap_err();
    assert!(d.iter().any(|d| d.code == "rule.entry"));
}

// ---- More cases: the sample sparkle filter, the assembly, bounds and settings that reach a loop indirectly.

#[test]
fn the_research_sparkle_filter_passes_and_its_padding_setting_is_costly() {
    use klipolo_engine::registry::guard::End;
    use klipolo_model::contrib::PaddingSpec;
    let def = ContribDef {
        params: serde_json::from_str(
            r#"[{"key":"spacing","label":"Spacing","type":"float","min":8,"max":120,"default":36},
                {"key":"size","label":"Size","type":"float","min":0.05,"max":1,"default":0.35},
                {"key":"speed","label":"Speed","type":"float","min":0,"max":4,"default":1},
                {"key":"amount","label":"Amount","type":"float","min":0,"max":2,"default":1}]"#,
        )
        .unwrap(),
        padding: Some(PaddingSpec {
            px: 4.0,
            param: Some("spacing".into()),
            per_unit: 0.5,
            max_px: 64.0,
        }),
        ..ContribDef::new(
            "lib.k3f9q2ma/effect.sparkle.v1",
            ContribKind::Effect,
            "Sparkle",
        )
    };
    let c = check(&def, ShaderKind::Filter, &wgsl("sparkle.wgsl")).unwrap();
    assert!(c.facts.reads_layer && c.facts.animates);
    let trips: Vec<_> = c
        .facts
        .loops
        .iter()
        .map(|l| (l.line, l.max_trips))
        .collect();
    assert_eq!(
        trips,
        [(15, Some(3)), (16, Some(3))],
        "both loops run -1..=1"
    );
    assert_eq!(
        c.facts.samples_per_pixel, 10,
        "the layer once, then once per cell of the 3×3"
    );
    let costly: Vec<_> = c
        .facts
        .costly_params
        .iter()
        .map(|p| (p.key.as_str(), p.costly_end))
        .collect();
    // The padding's setting (Max), which also moves where a glint skips its work with `continue` (Both).
    assert_eq!(
        costly,
        [("spacing", End::Both)],
        "only the padding's setting"
    );
}

#[test]
fn the_checked_source_is_what_the_registry_assembles() {
    use klipolo_engine::registry::{SANITIZE, ShaderEffect};
    let (def, body) = (generator(WAVES_PARAMS), wgsl("waves.wgsl"));
    let c = check(&def, ShaderKind::Generator, &body).unwrap();
    // `assemble`'s text, with the output check after `Params`, the body closed by `;` and the entry returning through
    // the output check.
    let assembled = ShaderEffect::assemble(&def, ShaderKind::Generator, &body).unwrap();
    let want = assembled
        .source
        .replacen("// ---- ", &format!("{SANITIZE}\n// ---- "), 1)
        .replacen("\n\n@fragment\n", "\n;\n\n@fragment\n", 1)
        .replacen(
            "return effect(in.uv);",
            "return kl_sanitize(effect(in.uv));",
            1,
        );
    assert_eq!(c.source, want);
    assert_eq!(
        c.source.lines().nth(c.body_line as usize - 1),
        body.lines().next()
    );
    let expected = c.facts.ops_per_pixel as f64 * 1920.0 * 1080.0 / 2e8;
    assert!((c.facts.static_ms(1920 * 1080) - expected).abs() < 1e-9);
    // Diagnostics reach agents as JSON.
    let d = check(&def, ShaderKind::Generator, &wgsl("glsl_mod.wgsl")).unwrap_err();
    let json = serde_json::to_value(&d[0]).unwrap();
    assert_eq!(
        (json["severity"].as_str(), json["line"].as_u64()),
        (Some("error"), Some(9))
    );
    assert!(json["sourceLine"].as_str().unwrap().contains("mod("));
}

#[test]
fn loop_bounds_are_read_only_when_the_loop_provably_ends() {
    let trips = |body: &str| {
        let f = format!(
            "fn effect(uv: vec2<f32>) -> vec4<f32> {{ var c = 0.0; {body} return vec4<f32>(c); }}"
        );
        check(&generator("[]"), ShaderKind::Generator, &f)
            .unwrap()
            .facts
            .loops[0]
            .max_trips
    };
    assert_eq!(
        trips("for (var i = 10; i > 0; i -= 2) { c += 0.1; }"),
        Some(5)
    );
    assert_eq!(
        trips("for (var i = 0u; i <= 4u; i++) { c += 0.1; }"),
        Some(5)
    );
    assert_eq!(
        trips("var i = 0; loop { if (i >= 3) { break; } c += 0.1; continuing { i += 1; } }"),
        Some(3)
    );
    assert_eq!(
        trips("for (var i = 5u; i >= 0u; i--) { c += 0.1; }"),
        None,
        "a u32 never goes below 0"
    );
    assert_eq!(
        trips("for (var i = 0; i < 8; i++) { i += 1; c += 0.1; }"),
        None,
        "the body moves the counter"
    );
    assert_eq!(
        trips("for (var i = 0; i < 8; i--) { c += 0.1; }"),
        None,
        "counts away from its bound"
    );
    assert_eq!(
        trips("for (var x = 1e8; x < 2e8; x += 1.0) { c += 0.1; }"),
        None,
        "1 is lost at 1e8 in f32"
    );
}

#[test]
fn a_setting_reaching_a_loop_through_a_helper_or_a_variable_is_costly_too() {
    use klipolo_engine::registry::guard::End;
    let def = generator(
        r#"[{"key":"octaves","label":"O","type":"int","min":1,"max":8,"default":4},
            {"key":"hq","label":"Q","type":"bool","default":false},
            {"key":"far","label":"F","type":"float","min":1,"max":100,"default":10}]"#,
    );
    let body = "fn fbm(p: vec2<f32>, n: i32) -> f32 {
    var s = 0.0;
    for (var i = 0; i < n; i++) { s += sin(p.x * f32(i)); }
    return s;
}
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var steps = 8;
    if (params.hq == 1u) { steps = 64; }
    var c = fbm(uv, i32(params.octaves));
    var t = 0.0;
    for (var i = 0; i < steps; i++) {
        if (t > params.far) { break; }
        t += 0.5;
        c += 0.01;
    }
    return vec4<f32>(c);
}";
    let c = check(&def, ShaderKind::Generator, body).unwrap();
    let end = |k: &str| {
        c.facts
            .costly_params
            .iter()
            .find(|p| p.key == k)
            .map(|p| p.costly_end)
    };
    assert_eq!(end("octaves"), Some(End::Max), "through fbm's argument");
    assert_eq!(
        end("hq"),
        Some(End::Both),
        "through a variable it sets: the costly arm isn't a simple comparison"
    );
    // A `break` other than the loop's own test is an early exit: both ends.
    assert_eq!(end("far"), Some(End::Both));
}
