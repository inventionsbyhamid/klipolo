//! The static cap holds against crafted bodies, and `check` stays bounded on any 32 KiB.

use std::time::{Duration, Instant};

use klipolo_engine::registry::guard::{self, Diagnostic, check};
use klipolo_engine::registry::{ShaderEffect, ShaderKind};
use klipolo_model::contrib::{ContribDef, ContribKind};

fn generator() -> ContribDef {
    ContribDef::new(
        "lib.k3f9q2ma/generator.test.v1",
        ContribKind::Generator,
        "Test",
    )
}

fn codes(body: &str) -> Vec<&'static str> {
    match check(&generator(), ShaderKind::Generator, body) {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(|d| d.code).collect(),
    }
}

fn effect(lines: &str) -> String {
    format!("fn effect(uv: vec2<f32>) -> vec4<f32> {{\n{lines}\n}}\n")
}

/// `first`, then `link` `n` times, broken into lines of at most 380 characters (with room for an indent before).
fn chain(first: &str, link: &str, n: usize) -> String {
    let mut out = String::from(first);
    let mut line = first.len();
    for _ in 0..n {
        if line + link.len() > 380 {
            out.push('\n');
            line = 0;
        }
        out.push_str(link);
        line += link.len();
    }
    out
}

/// What the main process does with a body the worker passed: assembled and hardened, the hardened
/// text verified (`compilable`), then parsed, validated and written as Metal's shading language, as wgpu does on the
/// Mac. naga's writer wraps every `Binary` in parentheses and spells types out, so that text nests deeper than the
/// body. The MSL's length, or why it failed.
fn hardened_msl(body: &str) -> Result<usize, String> {
    use naga::back::msl;
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    let fx = ShaderEffect::assemble_untrusted(&generator(), ShaderKind::Generator, body)
        .map_err(|d| format!("{d:?}"))?;
    fx.compilable()?;
    let module = naga::front::wgsl::parse_str(&fx.source).map_err(|e| e.to_string())?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .map_err(|e| e.to_string())?;
    let (text, _) = msl::write_string(
        &module,
        &info,
        &msl::Options::default(),
        &msl::PipelineOptions::default(),
    )
    .map_err(|e| e.to_string())?;
    Ok(text.len())
}

/// Runs `f` on a thread with the main process's 2 MiB blocking-pool stack.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn a_test_or_a_step_hoisted_out_of_the_loop_is_no_bound() {
    let trips = |lines: &str| {
        let c = check(&generator(), ShaderKind::Generator, &effect(lines)).unwrap();
        c.facts.loops[0].max_trips
    };
    let hoisted_load = "var i = 0; let first = i; var c = 0.0;
    for (; first < 1; i++) { c += 0.1; }
    return vec4<f32>(c);";
    assert_eq!(trips(hoisted_load), None, "`first` never changes");
    let hoisted_test = "var i = 0; let go = i < 5; var c = 0.0;
    for (; go; i++) { c += 0.1; }
    return vec4<f32>(c);";
    assert_eq!(trips(hoisted_test), None);
    let hoisted_step = "var i = 0; let s = i; var c = 0.0;
    for (; i < 4; i = s + 1) { c += 0.1; }
    return vec4<f32>(c);";
    assert_eq!(trips(hoisted_step), None, "`i` sticks at 1");
    // A crafted attack: about a thousand loop-free operations a step, once read as one step.
    let attack = "fn w0(x: f32) -> f32 { return sin(x) * 1.01 + cos(x) * 0.99 + x * 0.5 + 0.25; }
fn w1(x: f32) -> f32 { return w0(w0(w0(w0(w0(w0(w0(w0(w0(w0(x)))))))))); }
fn w2(x: f32) -> f32 { return w1(w1(w1(w1(w1(w1(w1(w1(w1(w1(x)))))))))); }
fn effect(uv: vec2<f32>) -> vec4<f32> {
    var i = 0;
    let first = i;
    var acc = uv.x;
    for (; first < 1; i++) {
        acc += w2(acc);
    }
    return vec4<f32>(acc);
}";
    assert_eq!(codes(attack), ["cost.static"]);
}

#[test]
fn float_counters_are_no_bound() {
    let trips = |head: &str| {
        let body = effect(&format!(
            "var c = 0.0;\n    for ({head}) {{ c += 0.1; }}\n    return vec4<f32>(c);"
        ));
        check(&generator(), ShaderKind::Generator, &body)
            .unwrap()
            .facts
            .loops[0]
            .max_trips
    };
    assert_eq!(
        trips("var x = 8388608.0; x < 8388611.0; x += 0.5"),
        None,
        "2^23 + 0.5 rounds back to 2^23"
    );
    assert_eq!(trips("var t = 0.0; t < 1.0; t += 0.25"), None);
}

#[test]
fn big_arrays_are_refused_and_copies_cost_their_elements() {
    let big = effect(
        "var big: array<vec4<f32>, 1000000>;
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < 64; i++) { let copy = big; acc += copy[i]; }
    return acc;",
    );
    assert!(codes(&big).contains(&"rule.size"), "{:?}", codes(&big));
    let wide = "struct Half { a: array<f32, 600> }
struct Both { a: Half, b: Half }
fn effect(uv: vec2<f32>) -> vec4<f32> { var w: Both; return vec4<f32>(w.a.a[0]); }";
    assert!(
        codes(wide).contains(&"rule.size"),
        "1,200 values in one struct"
    );
    let fits = effect(
        "var a: array<f32, 1000>;
    var acc = 0.0;
    for (var i = 0; i < 64; i++) { let copy = a; acc += copy[i]; }
    return vec4<f32>(acc);",
    );
    let c = check(&generator(), ShaderKind::Generator, &fits).unwrap();
    assert!(
        c.facts.ops_per_pixel > 64 * 1000,
        "each copy moves 1,000 values: {}",
        c.facts.ops_per_pixel
    );
}

#[test]
fn a_fan_out_of_structs_is_checked_at_once() {
    // 2^29 values in 30 lines (2 GiB of bools: under naga's 4 GiB).
    let mut body = String::from("struct S0 { a: bool }\n");
    for k in 0..29 {
        body.push_str(&format!("struct S{} {{ a: S{k}, b: S{k} }}\n", k + 1));
    }
    body.push_str("const c = S29();\nfn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }\n");
    let started = Instant::now();
    assert!(codes(&body).contains(&"rule.size"));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn long_chains_are_refused_before_naga_even_on_a_small_stack() {
    let binary = effect(&format!(
        "let a = uv.x;\n    let b = {};\n    return vec4<f32>(b);",
        chain("a", "+a", 16_000)
    ));
    let selectors = effect(&format!(
        "let b = {}.x;\n    return vec4<f32>(b);",
        chain("uv", ".xy", 10_000)
    ));
    let elses = effect(&format!(
        "let a = uv.x;\n    var b = 0.0;\n    {}\n    return vec4<f32>(b);",
        chain(
            "if (a > 0.5) { b = 1.0; }",
            " else if (a > 0.5) { b = 1.0; }",
            900
        )
    ));
    for (name, body, code) in [
        ("binary", binary, "scan.chain"),
        ("selectors", selectors, "scan.chain"),
        ("else if", elses, "scan.nesting"),
    ] {
        assert!(
            body.len() <= guard::MAX_SOURCE_BYTES,
            "{name}: {}",
            body.len()
        );
        let found = on_small_stack(move || codes(&body));
        assert!(found.contains(&code), "{name}: {found:?}");
    }
}

#[test]
fn chains_just_under_the_caps_check_on_a_small_stack() {
    let binary = effect(&format!(
        "let a = uv.x;\n    let b = {};\n    return vec4<f32>(b);",
        chain("a", "+a", 250)
    ));
    let selectors = effect(&format!(
        "let b = {}.x;\n    return vec4<f32>(b);",
        chain("uv", ".xy", 250)
    ));
    let elses = effect(&format!(
        "let a = uv.x;\n    var b = 0.0;\n    {}\n    return vec4<f32>(b);",
        chain(
            "if (a > 0.5) { b = 1.0; }",
            " else if (a > 0.5) { b = 1.0; }",
            29
        )
    ));
    // A type nested 32 deep, the cap (`rule.nesting` counts `alias` chains), which the hardened
    // text spells out whole.
    let mut deep = String::from("alias A0 = array<f32, 1>;\n");
    for k in 1..32 {
        deep.push_str(&format!("alias A{k} = array<A{}, 1>;\n", k - 1));
    }
    deep.push_str(&effect(
        "    var x: A31;\n    return vec4<f32>(uv, 0.0, 1.0);",
    ));
    for (name, body) in [
        ("binary", binary),
        ("selectors", selectors),
        ("else if", elses),
        ("deep type", deep),
    ] {
        let (found, msl) = on_small_stack(move || (codes(&body), hardened_msl(&body)));
        assert!(found.is_empty(), "{name}: {found:?}");
        if let Err(why) = msl {
            panic!("{name}: the hardened text didn't make it to MSL: {why}");
        }
    }
}

#[test]
fn every_cap_class_refuses() {
    let functions: String = (0..65)
        .map(|k| format!("fn g{k}() {{}}\n"))
        .collect::<String>()
        + "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";
    let line = "c+=c*c*c*c*c*c*c*c*c*c;c+=c*c*c*c*c*c*c*c*c*c;c+=c*c*c*c*c*c*c*c*c*c;\n";
    let expressions = effect(&format!(
        "var c = uv.x;\n{}return vec4<f32>(c);",
        line.repeat(380)
    ));
    let tables = "const t = array<f32, 600>(); const u = array<f32, 600>();
fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(t[0] + u[0]); }";
    let cases = [
        (functions, "rule.size"),
        (expressions, "rule.size"),
        (tables.to_string(), "rule.size"),
        ("override k: f32 = 1.0;\nfn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(k); }".to_string(), "rule.override"),
        ("struct A { n: atomic<u32> }\nfn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }".to_string(), "rule.atomic"),
        (effect("let a = 1.0;\0\n    return vec4<f32>(a);"), "scan.nul"),
        ("//\n".repeat(401), "scan.lines"),
        (format!("// {}\n", "x".repeat(400)), "scan.line"),
    ];
    for (body, code) in cases {
        assert!(
            body.len() <= guard::MAX_SOURCE_BYTES,
            "{code}: {}",
            body.len()
        );
        let found = codes(&body);
        assert!(found.contains(&code), "{code}: {found:?}");
    }
}

#[test]
fn diagnostics_escape_what_the_agent_wrote() {
    let body = effect(
        "let x = mod(1.0, 2.0); // \u{1b}[31m red \u{202E}reversed\n    return vec4<f32>(x);",
    );
    let d = check(&generator(), ShaderKind::Generator, &body).unwrap_err();
    let line = d[0].source_line.as_deref().unwrap();
    assert!(
        line.contains("\\u{001B}") && line.contains("\\u{202E}"),
        "{line}"
    );
    assert!(!line.contains('\u{1b}') && !line.contains('\u{202E}'));
    let bad = ContribDef::new(
        "lib.k3f9q2ma/generator.x\n@group(0)\u{202E}",
        ContribKind::Generator,
        "Bad",
    );
    let d = check(
        &bad,
        ShaderKind::Generator,
        &effect("return vec4<f32>(uv, 0.0, 1.0);"),
    )
    .unwrap_err();
    let message: &Diagnostic = &d[0];
    assert_eq!(message.code, "rule.params");
    assert!(
        !message.message.contains('\n') && !message.message.contains('\u{202E}'),
        "{}",
        message.message
    );
}

#[test]
fn the_scan_builds_no_more_than_it_reports() {
    let body = effect(&format!(
        "let a = 1.0;\n{}\n    return vec4<f32>(a);",
        chain("", "\u{200B}", 10_000)
    ));
    assert!(body.len() <= guard::MAX_SOURCE_BYTES);
    let started = Instant::now();
    let found = guard::scan(&body);
    assert!(found.len() <= 8 && found.iter().all(|d| d.code == "scan.invisible"));
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "{:?}",
        started.elapsed()
    );
}
