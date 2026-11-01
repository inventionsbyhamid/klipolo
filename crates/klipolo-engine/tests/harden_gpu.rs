//! The module a device compiles for WGSL that isn't built in: one loop counter
//! shared by every loop, a sanitising entry, and naga's re-emitted text read back and checked before anything runs.

mod support;
use klipolo_engine::registry::guard::harden::{harden, loop_guards};
use klipolo_engine::registry::guard::{LOOP_TRIPS, check};
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind, ShaderTrust};
use klipolo_model::contrib::{ContribDef, ContribKind, builtin};

fn def() -> ContribDef {
    ContribDef::new(
        "lib.k3f9q2ma/generator.test.v1",
        ContribKind::Generator,
        "Test",
    )
}

#[test]
fn every_loop_gets_the_shared_counter_and_the_entry_sanitises() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
        var x = 0.0;
        for (var i = 0; i < 3; i++) { for (var j = 0; j < 3; j++) { x += 1.0; } }
        loop { x += 1.0; if (x > 100.0) { break; } }
        return vec4<f32>(x / 200.0);
    }";
    let fx = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, body).unwrap();
    // naga's writer bakes the load (`let _eN = kl_trips; if (_eN >= 4096u) { break; } kl_trips = (_eN + 1u);`), so the
    // test reads what it really wrote: the re-parsed module's loops, and the limit's literal once per loop.
    let module = naga::front::wgsl::parse_str(&fx.source).unwrap();
    let g = klipolo_engine::registry::guard::harden::loop_guards(&module);
    assert_eq!((g.loops, g.guarded), (3, 3), "{}", fx.source);
    assert_eq!(fx.source.matches("4096u").count(), 3, "{}", fx.source);
    assert!(fx.source.contains("kl_sanitize("));
    klipolo_engine::registry::validate_wgsl(&fx.source).unwrap();
    // What the registry keeps: the hardened text under its own key, and the facts the guard read.
    assert_eq!(fx.trust, ShaderTrust::Checked);
    assert_eq!(
        fx.key,
        klipolo_engine::registry::fnv1a64(fx.source.as_bytes())
    );
    assert!(!fx.animates);
    assert_eq!(fx.facts.as_ref().unwrap().loops.len(), 3);
}

#[test]
fn protective_or_builtin_ids_are_refused() {
    let p = ContribDef {
        protective: true,
        ..def()
    };
    assert!(
        ShaderEffect::assemble_untrusted(
            &p,
            ShaderKind::Generator,
            "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0); }"
        )
        .is_err()
    );
    let b = ContribDef::new(
        "klipolo.builtin/generator.fake",
        ContribKind::Generator,
        "Fake",
    );
    assert!(
        ShaderEffect::assemble_untrusted(
            &b,
            ShaderKind::Generator,
            "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0); }"
        )
        .is_err()
    );
    // The refusal quotes the id cut at 64 characters, as a malformed id is.
    let long = format!("klipolo.builtin/generator.{}", "x".repeat(500));
    let long = ContribDef::new(long.as_str(), ContribKind::Generator, "Long");
    let refused = ShaderEffect::assemble_untrusted(
        &long,
        ShaderKind::Generator,
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0); }",
    )
    .unwrap_err();
    assert_eq!(refused[0].code, "rule.builtin");
    assert!(
        refused[0].message.chars().count() <= 64 + 24,
        "{}",
        refused[0].message
    );
}

/// The sample shaders (a generator with a helper's loop; a filter with nested loops and `continue`) come back
/// from naga's writer whole: every loop guarded, the host's entry points, the output check.
#[test]
fn the_research_shaders_survive_the_round_trip() {
    let wgsl = |name: &str| {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/wgsl/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    };
    let with = |id: &str, kind, params: &str| ContribDef {
        params: serde_json::from_str(params).unwrap(),
        ..ContribDef::new(id, kind, "Test")
    };
    let waves = with(
        "lib.k3f9q2ma/generator.waves.v1",
        ContribKind::Generator,
        r##"[{"key":"density","label":"D","type":"float","min":0.5,"max":8,"default":2},
            {"key":"speed","label":"S","type":"float","min":0,"max":4,"default":1},
            {"key":"deepColor","label":"Deep","type":"color","default":"#0B2A4A"},
            {"key":"shallowColor","label":"Shallow","type":"color","default":"#2F8FB8"},
            {"key":"foam","label":"F","type":"float","min":0,"max":1,"default":0.4}]"##,
    );
    let sparkle = with(
        "lib.k3f9q2ma/effect.sparkle.v1",
        ContribKind::Effect,
        r#"[{"key":"spacing","label":"Spacing","type":"float","min":8,"max":120,"default":36},
            {"key":"size","label":"Size","type":"float","min":0.05,"max":1,"default":0.35},
            {"key":"speed","label":"Speed","type":"float","min":0,"max":4,"default":1},
            {"key":"amount","label":"Amount","type":"float","min":0,"max":2,"default":1}]"#,
    );
    for (def, kind, file, loops) in [
        (waves, ShaderKind::Generator, "waves.wgsl", 1),
        (sparkle, ShaderKind::Filter, "sparkle.wgsl", 2),
    ] {
        let fx = ShaderEffect::assemble_untrusted(&def, kind, &wgsl(file))
            .unwrap_or_else(|d| panic!("{file}: {d:#?}"));
        let m = naga::front::wgsl::parse_str(&fx.source).unwrap();
        let g = loop_guards(&m);
        assert_eq!((g.loops, g.guarded), (loops, loops), "{file}");
        assert!(fx.animates, "{file}");
        klipolo_engine::registry::validate_wgsl(&fx.source).unwrap();
    }
}

/// The walk tells a guarded loop from one that isn't: the text before hardening has its loops and no checks, and a
/// check with the wrong limit, a missing step or another variable doesn't count.
#[test]
fn the_walk_counts_only_the_hosts_own_check() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { var x = 0.0; loop { x += 1.0; if (x > 9.0) { break; } } return vec4<f32>(x); }";
    let checked = check(&def(), ShaderKind::Generator, body).unwrap();
    let before = naga::front::wgsl::parse_str(&checked.source).unwrap();
    assert_eq!(
        (loop_guards(&before).loops, loop_guards(&before).guarded),
        (1, 0)
    );
    let module = |check: &str| {
        format!(
            "var<private> kl_trips: u32;\nvar<private> other: u32;\n@fragment fn main() -> @location(0) vec4<f32> {{ var x = 0.0; loop {{ {check} x += 1.0; if (x > 9.0) {{ break; }} }} return vec4<f32>(x); }}"
        )
    };
    for (check, guarded) in [
        (
            format!("if (kl_trips >= {LOOP_TRIPS}u) {{ break; }} kl_trips = kl_trips + 1u;"),
            1,
        ),
        (
            "if (kl_trips >= 4095u) { break; } kl_trips = kl_trips + 1u;".to_string(),
            0,
        ),
        ("if (kl_trips >= 4096u) { break; }".to_string(), 0),
        (
            "if (other >= 4096u) { break; } other = other + 1u;".to_string(),
            0,
        ),
        (
            "if (kl_trips >= 4096u) { break; } kl_trips = kl_trips + 2u;".to_string(),
            0,
        ),
        (
            "x += 0.5; if (kl_trips >= 4096u) { break; } kl_trips = kl_trips + 1u;".to_string(),
            0,
        ),
    ] {
        let m = naga::front::wgsl::parse_str(&module(&check)).unwrap();
        assert_eq!(loop_guards(&m).guarded, guarded, "{check}");
    }
    // The step's load must be one of this body's own: a load hoisted out of the loop would leave the counter stuck.
    let hoisted = "var<private> kl_trips: u32;\n@fragment fn main() -> @location(0) vec4<f32> { var x = 0.0; let t = kl_trips; loop { if (kl_trips >= 4096u) { break; } kl_trips = t + 1u; x += 1.0; if (x > 9.0) { break; } } return vec4<f32>(x); }";
    let m = naga::front::wgsl::parse_str(hoisted).unwrap();
    assert_eq!(loop_guards(&m).guarded, 0);
}

/// Fail closed: a module that loses the host's entry points or its output check after hardening is refused,
/// never compiled.
#[test]
fn a_module_that_loses_the_hosts_entry_points_or_output_check_is_refused() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";
    let mut no_vs = check(&def(), ShaderKind::Generator, body).unwrap();
    no_vs.module.entry_points.retain(|ep| ep.name != "kl_vs");
    let d = harden(&no_vs).unwrap_err();
    assert_eq!(d[0].code, "harden.roundtrip", "{d:?}");
    assert!(d[0].message.contains("entry points"), "{}", d[0].message);

    let mut renamed = check(&def(), ShaderKind::Generator, body).unwrap();
    for (_, f) in renamed.module.functions.iter_mut() {
        if f.name.as_deref() == Some("kl_sanitize") {
            f.name = Some("other_name".into());
        }
    }
    let d = harden(&renamed).unwrap_err();
    assert_eq!(d[0].code, "harden.roundtrip", "{d:?}");
}

/// naga resolves a call to the module's own function before WGSL's, so a body's `min` would replace the one the output
/// check uses: refused (the guard's `host` rule), whether or not its signature fits.
#[test]
fn a_function_that_would_replace_one_the_output_check_uses_is_refused() {
    // It fits: `rgb ≤ a` would no longer hold.
    let fits = "fn min(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> { return a; }
fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0, 1.0, 1.0, 0.25); }";
    let d = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, fits).unwrap_err();
    assert_eq!((d[0].code, d[0].line), ("rule.shadow", 1), "{d:?}");
    assert!(d[0].message.contains("`min`"), "{}", d[0].message);
    // It doesn't fit the output check's call.
    let misfits = "fn min(a: f32, b: f32) -> f32 { return a; }
fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(min(uv.x, 0.5)); }";
    let d = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, misfits).unwrap_err();
    assert_eq!(d[0].code, "rule.shadow", "{d:?}");
    // A name the host doesn't call is the author's to take.
    let mine = "fn mix3(a: f32) -> f32 { return a; }
fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(mix3(uv.x)); }";
    ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, mine).unwrap();
}

/// A body that ends in an attribute can't hand it to the host's `kl_fs`: the body is closed with `;`.
#[test]
fn a_dangling_attribute_cant_reach_the_hosts_entry() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }\n@diagnostic(off, derivative_uniformity)";
    assert!(ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, body).is_err());
}

/// naga's writer drops a trailing `_` and non-ASCII letters, so `fn max_` or `fn maäx` would come back as `fn max` and
/// take every built-in `max(…)` call that fits, the prelude's included, in the compiled text only.
#[test]
fn names_naga_would_rewrite_are_refused() {
    // `_naga…` is the WGSL writer's reserved prefix: it would come back as `gen__naga…`.
    for name in ["max_", "maäx", "x__y", "_nagafoo"] {
        let body = format!(
            "fn {name}(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {{ return a; }}\nfn effect(uv: vec2<f32>) -> vec4<f32> {{ return vec4<f32>(kl_to_linear(vec3<f32>(uv, 0.5)), 1.0); }}"
        );
        let d = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, &body).unwrap_err();
        assert_eq!((d[0].code, d[0].line), ("rule.name", 1), "{name}: {d:?}");
        assert!(d[0].message.contains(name), "{}", d[0].message);
    }
    // Locals too; a phony `_` and a leading underscore are names the writer keeps.
    let local = "fn effect(uv: vec2<f32>) -> vec4<f32> {\n    let größe = 1.0;\n    return vec4<f32>(größe);\n}";
    let d = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, local).unwrap_err();
    assert_eq!((d[0].code, d[0].line), ("rule.name", 2), "{d:?}");
    let kept = "fn effect(uv: vec2<f32>) -> vec4<f32> { let _half = 0.5; _ = uv; return vec4<f32>(_half); }";
    ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, kept).unwrap();
}

/// Fail closed whatever the name rule misses: a function renamed into a built-in in the checked IR comes back as
/// `fn max` and captures the prelude's `max` calls, which the round trip counts.
#[test]
fn the_round_trip_refuses_a_call_a_renamed_function_captured() {
    let body = "fn mymax(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> { return a; }\nfn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(kl_to_linear(vec3<f32>(uv, 0.5)), 1.0); }";
    let mut checked = check(&def(), ShaderKind::Generator, body).unwrap();
    for (_, f) in checked.module.functions.iter_mut() {
        if f.name.as_deref() == Some("mymax") {
            f.name = Some("max_".into());
        }
    }
    let d = harden(&checked).unwrap_err();
    assert_eq!(d[0].code, "harden.roundtrip", "{d:?}");
    assert!(d[0].message.contains("built-in"), "{}", d[0].message);
}

/// naga's writer drops `@diagnostic(…)` filters, so the round trip would refuse what `check` let through: refused up
/// front, with the reason.
#[test]
fn diagnostic_filters_are_refused() {
    let body = "@diagnostic(off, derivative_uniformity)\nfn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";
    let d = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, body).unwrap_err();
    assert_eq!((d[0].code, d[0].line), ("rule.diagnostic", 1), "{d:?}");
}

/// naga 25's uniformity analysis treats `break` and `continue` as no disruption, so the counter's `break` leaves an
/// implicit-derivative sample in a loop valid. This pins it: a naga that adopts WGSL's full analysis turns it red.
#[test]
fn the_counter_keeps_implicit_derivatives_valid_on_naga_25() {
    let filter = ContribDef::new("lib.k3f9q2ma/effect.taps.v1", ContribKind::Effect, "Taps");
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = vec4<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        if (kl.progress > 0.9) { continue; }
        c += textureSample(kl_a, kl_samp, uv + vec2<f32>(f32(i), 0.0) * kl_texel()) * 0.25;
        if (kl.progress > 0.5) { break; }
    }
    return c;
}";
    let fx = ShaderEffect::assemble_untrusted(&filter, ShaderKind::Filter, body)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert!(fx.source.contains("textureSample("), "{}", fx.source);
}

/// A module that stops validating once hardened is refused where the author can find it, and without the uniformity
/// hint unless that is the cause.
#[test]
fn a_shader_that_stops_validating_is_refused_at_its_line() {
    use naga::{Scalar, TypeInner};
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {\n    let k = 1.0;\n    return vec4<f32>(uv, 0.0, k);\n}";
    let mut checked = check(&def(), ShaderKind::Generator, body).unwrap();
    let f32_ty = checked
        .module
        .types
        .iter()
        .find(|(_, t)| t.inner == TypeInner::Scalar(Scalar::F32))
        .map(|(h, _)| h)
        .unwrap();
    for (_, f) in checked.module.functions.iter_mut() {
        if f.name.as_deref() == Some("effect") {
            f.result.as_mut().unwrap().ty = f32_ty;
        }
    }
    let d = harden(&checked).unwrap_err();
    assert_eq!((d[0].code, d[0].line), ("harden.validate", 3), "{d:?}");
    assert!(d[0].hint.is_none(), "{d:?}");
    assert!(
        !d[0].message.contains("// ----"),
        "no assembled source: {}",
        d[0].message
    );
}

/// The output check holds only hardened text for anything that isn't built in: `compilable` is what the effect
/// stage's compile asks first.
#[test]
fn only_hardened_text_compiles_for_a_shader_that_isnt_built_in() {
    let registry = Registry::builtin();
    let gradient = registry
        .shader(&builtin::GENERATOR_GRADIENT.into())
        .unwrap();
    gradient.compilable().unwrap();
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { var x = 0.0; loop { x += 1.0; if (x > 9.0) { break; } } return vec4<f32>(x); }";
    let mut fx = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, body).unwrap();
    fx.compilable().unwrap();
    // Trusted by mistake: assembled as written, under an id that isn't built in.
    let plain = ShaderEffect::assemble(&def(), ShaderKind::Generator, body).unwrap();
    assert!(plain.compilable().is_err());
    // Marked checked, but its text isn't the hardened one.
    fx.source = plain.source.clone();
    assert!(
        fx.compilable().unwrap_err().contains("loop"),
        "{:?}",
        fx.compilable()
    );
}

/// More of what comes back whole: a loop inside a `continuing` block, and a transition.
#[test]
fn a_continuing_loop_and_a_transition_survive_the_round_trip() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var x = 0.0;
    var n = 0;
    loop {
        x += 1.0;
        continuing {
            for (var k = 0; k < 2; k++) { x += 0.5; }
            n += 1;
            break if n >= 3;
        }
    }
    return vec4<f32>(x / 10.0);
}";
    let fx = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, body)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let g = loop_guards(&naga::front::wgsl::parse_str(&fx.source).unwrap());
    assert_eq!((g.loops, g.guarded), (2, 2), "{}", fx.source);
    let fade = ContribDef::new(
        "lib.k3f9q2ma/transition.fade.v1",
        ContribKind::Transition,
        "Fade",
    );
    let body = "fn transition(uv: vec2<f32>) -> vec4<f32> {
    var c = vec4<f32>(0.0);
    for (var i = 0; i < 2; i++) { c += mix(kl_sample_a(uv), kl_sample_b(uv), kl.progress) * 0.5; }
    return c;
}";
    let fx = ShaderEffect::assemble_untrusted(&fade, ShaderKind::Transition, body)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let g = loop_guards(&naga::front::wgsl::parse_str(&fx.source).unwrap());
    assert_eq!((g.loops, g.guarded), (1, 1), "{}", fx.source);
    assert!(fx.animates && fx.source.contains("transition("));
}

/// Built-ins keep `assemble`: trusted, no facts, and whether they animate read from their IR at registry build.
#[test]
fn built_ins_keep_their_path_and_know_whether_they_animate() {
    let registry = Registry::builtin();
    let gradient = registry
        .shader(&builtin::GENERATOR_GRADIENT.into())
        .unwrap();
    let dissolve = registry
        .shader(&builtin::TRANSITION_DISSOLVE.into())
        .unwrap();
    assert_eq!(gradient.trust, ShaderTrust::Builtin);
    assert!(gradient.facts.is_none());
    assert!(!gradient.animates, "reads only kl.size");
    assert!(dissolve.animates, "reads kl.progress");
    assert!(!gradient.source.contains("kl_sanitize") && !gradient.source.contains("kl_trips"));
}

/// The effect stage's compile asks `compilable()` first, not naga's validation alone: a shader marked checked whose text
/// skipped hardening is never drawn, and the frame says so. With `validate_wgsl` in its place
/// the unhardened text, which validates, would draw white.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_checked_shader_whose_text_isnt_hardened_isnt_drawn() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
    let mut fx = ShaderEffect::assemble_untrusted(&def(), ShaderKind::Generator, body).unwrap();
    let plain = ShaderEffect::assemble(&def(), ShaderKind::Generator, body).unwrap();
    fx.source = plain.source.clone();
    assert_eq!(fx.trust, ShaderTrust::Checked);
    let (px, warnings) = support::make::render_generator_shader(&def(), fx, (16, 16)).await;
    assert!(
        warnings.iter().any(|w| w.contains("couldn't be drawn")),
        "{warnings:?}"
    );
    assert_eq!(&px[..4], &[0, 0, 0, 255], "only the black background");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_endless_loop_ends_and_non_finite_colours_draw_as_black() {
    // `loop {}` with no break: without the counter this never returns.
    let endless = "fn effect(uv: vec2<f32>) -> vec4<f32> { var x = 0.0; loop { x += 1.0; } return vec4<f32>(x); }";
    let nan = "fn effect(uv: vec2<f32>) -> vec4<f32> { let z = uv.x - uv.x; return vec4<f32>(z / z, 1.0 / z, -1.0, 2.0); }";
    let over = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0, 1.0, 1.0, 0.25); }";
    // The generator fills the frame over the project's black background: NaN, +inf and −1 → 0, alpha 2 → 1, so
    // opaque black; the premultiplied (1, 1, 1, 0.25) becomes (0.25, 0.25, 0.25, 0.25) and composites to 64 over black.
    for (body, want) in [(nan, [0u8, 0, 0, 255]), (over, [64, 64, 64, 255])] {
        let px = support::make::render_generator_rgba(&def(), body, (16, 16)).await;
        assert_eq!(&px[..4], &want, "{body}");
    }
    let started = std::time::Instant::now();
    let px = support::make::render_generator_rgba(&def(), endless, (16, 16)).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    // 4,096 iterations ran and came back through the output check: x = 4096 clamps to opaque white.
    assert_eq!(&px[..4], &[255, 255, 255, 255]);
}
