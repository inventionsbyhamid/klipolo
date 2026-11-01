//! Effect ABI v1.1 in the frame plan: a generator's time follows its piece of the layer and holds on its `fps` steps, a
//! generator whose picture repeats carries a cache key, a half-size one says so, and the definitions that ask for
//! these, or for a downsample, are checked.

mod support;
use klipolo_engine::plan::generator_time;
use klipolo_model::timeline::PieceTime;
use support::*;

use klipolo_engine::plan::{frame_index, frame_plan, frame_start_us};
use klipolo_engine::registry::ShaderEffect;
use klipolo_engine::registry::ShaderKind::{Filter, Generator};
use klipolo_engine::registry::guard::check;
use klipolo_model::contrib::builtin;
use klipolo_model::contrib::{ContribDef, DownsampleSpec, Resolution};
use klipolo_model::ops::Op;
use klipolo_model::params::{ParamKind, ParamSpec, ParamUnit};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::Us;

#[test]
fn a_split_piece_continues_its_generators_time() {
    let whole = PieceTime {
        offset_us: 0,
        span_us: 6_000_000,
    };
    let second = PieceTime {
        offset_us: 2_000_000,
        span_us: 6_000_000,
    };
    assert_eq!(generator_time(whole, 2_500_000, None, true, 30.0).0, 2.5);
    assert_eq!(generator_time(second, 500_000, None, true, 30.0).0, 2.5);
    assert!((generator_time(second, 500_000, None, true, 30.0).1 - 2.5 / 6.0).abs() < 1e-6);
}

#[test]
fn stepped_time_holds_within_a_step_and_static_time_is_zero() {
    let w = PieceTime {
        offset_us: 0,
        span_us: 10_000_000,
    };
    assert_eq!(
        generator_time(w, 1_040_000, Some(12), true, 30.0).0,
        generator_time(w, 1_070_000, Some(12), true, 30.0).0
    );
    assert_ne!(
        generator_time(w, 1_040_000, Some(12), true, 30.0).0,
        generator_time(w, 1_090_000, Some(12), true, 30.0).0
    );
    assert_eq!(
        generator_time(w, 7_000_000, None, false, 30.0),
        (0.0, 0.0, 0)
    );
}

/// Draws its own pattern, moving every frame (it reads `kl.time` and `kl.frame`).
const MOVING: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(fract(kl.time), uv.x, f32(kl.frame % 7u) / 7.0, 1.0);
}";
/// The same picture on every frame.
const STILL: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.5, 1.0); }";
/// A filter that pulses with `kl.time`.
const SPARKLE: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    return kl_sample_a(uv) * (0.5 + 0.5 * sin(kl.time * 6.0));
}";

fn landscape(
    p: Project,
    registry: &std::sync::Arc<klipolo_engine::registry::Registry>,
    t: f64,
) -> klipolo_engine::plan::FramePlan {
    frame_plan(
        &ctx_with(p, registry.clone()),
        secs(t),
        FormatKind::Landscape,
    )
}

/// The second piece of a split generator layer, and the filter on it, see the time, progress and frame the whole layer
/// saw at the same place in the contribution's time.
#[test]
fn a_split_generator_layer_and_its_filters_continue_the_wholes_time() {
    let moving = generator_def("moving", None, None);
    let sparkle = padded_filter_def("sparkle", None);
    let registry = registry_of(&[
        (moving.clone(), Generator, MOVING),
        (sparkle, Filter, SPARKLE),
    ]);
    // The whole layer runs 6 s from 1 s; cut 2 s in, its second piece starts at 3 s, 2 s into the span.
    let whole = generator_piece_project(&moving, 1.0, 6.0, None, vec![fx("sparkle")]);
    let second = generator_piece_project(&moving, 3.0, 4.0, Some((2.0, 6.0)), vec![fx("sparkle")]);
    let w = generator_of(&landscape(whole, &registry, 3.5)).clone();
    let s = generator_of(&landscape(second, &registry, 3.5)).clone();
    assert_eq!((w.source.time_s, s.source.time_s), (2.5, 2.5));
    assert_eq!(s.source.progress, w.source.progress);
    assert!((s.source.progress - 2.5 / 6.0).abs() < 1e-6);
    assert_eq!(s.source.frame, Some(75), "2.5 s on the 30 fps grain clock");
    assert_eq!(w.source.frame, s.source.frame);
    assert_eq!(s.source.duration_s, 6.0, "the whole span");
    assert_eq!((s.effects[0].time_s, s.effects[0].frame), (2.5, Some(75)));
    assert_eq!(s.cache_key, None, "it moves every frame");
}

/// A static generator keeps one cache key whatever the time, a stepped one keeps one per step, one that moves every
/// frame has none; `resolution: half` draws at half size and `auto` at full size until the render worker resolves it.
#[test]
fn static_and_stepped_generators_carry_a_cache_key_and_half_size_says_so() {
    let still = generator_def("still", Some(Resolution::Half), None);
    let stepped = generator_def("stepped", Some(Resolution::Auto), Some(12));
    let moving = generator_def("moving", None, None);
    let registry = registry_of(&[
        (still.clone(), Generator, STILL),
        (stepped.clone(), Generator, MOVING),
        (moving.clone(), Generator, MOVING),
    ]);
    let at = |def: &ContribDef, seconds: f64, t: f64| {
        let p = generator_piece_project(def, 0.0, seconds, None, Vec::new());
        generator_of(&landscape(p, &registry, t)).clone()
    };
    let (a, b) = (at(&still, 10.0, 1.0), at(&still, 10.0, 7.0));
    assert!(a.cache_key.is_some());
    assert_eq!(a.cache_key, b.cache_key);
    assert_eq!(
        (a.source.time_s, a.source.progress, a.source.frame),
        (0.0, 0.0, Some(0)),
        "it reads no time"
    );
    assert!(a.half);
    // Its layer's length is `kl.duration`: trimmed, it draws again.
    assert_ne!(at(&still, 8.0, 1.0).cache_key, a.cache_key);

    let step = |t: f64| at(&stepped, 10.0, t);
    assert!(step(1.04).cache_key.is_some());
    assert_eq!(step(1.04).cache_key, step(1.07).cache_key);
    assert_ne!(step(1.07).cache_key, step(1.09).cache_key);
    assert_eq!(
        (step(1.07).source.time_s, step(1.07).source.step_fps),
        (1.0, 12.0)
    );
    assert_eq!(step(1.07).source.fps, 30.0, "kl.fps is the project's");
    assert!(!step(1.04).half, "auto is drawn at full size");

    assert_eq!(at(&moving, 10.0, 1.04).cache_key, None);
    assert_eq!(at(&moving, 10.0, 1.04).source.step_fps, 0.0, "smooth");
}

/// The fields v1.1 adds to `KlFrame` (`fps`, `duration`, `resolution`, `step_fps`) hold for a layer's whole length, so
/// reading them alone draws the same every frame; stepped and piece time reach a shader only as `kl.time`,
/// `kl.progress` and `kl.frame`, which the guard's `animates` sees.
#[test]
fn the_new_kl_fields_hold_over_a_layer_and_time_stays_visible_to_the_guard() {
    let def = generator_def("g", None, Some(12));
    let animates = |body: &str| check(&def, Generator, body).unwrap().facts.animates;
    let constant = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(kl.duration / 10.0, kl.fps / 60.0 + kl.step_fps / 120.0, kl.resolution, 1.0);
}";
    assert!(!animates(constant));
    for time in ["kl.time", "kl.progress", "f32(kl.frame)"] {
        let body = format!(
            "fn effect(uv: vec2<f32>) -> vec4<f32> {{\n    return vec4<f32>(fract({time}), 0.0, 0.0, 1.0);\n}}"
        );
        assert!(animates(&body), "{time}");
    }
    // Planned: the constant reader is cached like any static generator.
    let registry = registry_of(&[(def.clone(), Generator, constant)]);
    let at = |t: f64| {
        let p = generator_piece_project(&def, 0.0, 10.0, None, Vec::new());
        generator_of(&landscape(p, &registry, t)).cache_key
    };
    assert!(at(1.0).is_some());
    assert_eq!(at(1.0), at(6.5));
}

/// `fps` (1–60) and `resolution` belong to generators; anything else is refused with its own code.
#[test]
fn fps_and_resolution_are_a_generators_and_fps_is_one_to_sixty() {
    let code = |def: &ContribDef, kind, body: &str| {
        ShaderEffect::assemble_untrusted(def, kind, body)
            .err()
            .map(|d| d[0].code)
    };
    let pass = "fn effect(uv: vec2<f32>) -> vec4<f32> { return kl_sample_a(uv); }";
    for fps in [0, 61] {
        let def = generator_def("g", None, Some(fps));
        assert_eq!(code(&def, Generator, STILL), Some("rule.fps"), "{fps}");
    }
    for fps in [1, 60] {
        assert_eq!(
            code(&generator_def("g", None, Some(fps)), Generator, STILL),
            None
        );
    }
    let stepped_filter = ContribDef {
        fps: Some(12),
        ..padded_filter_def("f", None)
    };
    assert_eq!(code(&stepped_filter, Filter, pass), Some("rule.fps"));
    let half_filter = ContribDef {
        resolution: Some(Resolution::Half),
        ..padded_filter_def("f", None)
    };
    assert_eq!(code(&half_filter, Filter, pass), Some("rule.resolution"));
    let half = generator_def("g", Some(Resolution::Half), Some(24));
    let shader = ShaderEffect::assemble_untrusted(&half, Generator, STILL).unwrap();
    assert_eq!(
        (shader.resolution, shader.fps),
        (Resolution::Half, Some(24))
    );
}

/// A filter that isn't built in may run at half size from a px setting of at least 4 px; any other rule is refused,
/// saying why.
#[test]
fn a_non_built_in_filter_downsamples_only_from_a_px_setting_of_4_px_or_more() {
    let soft = |unit: Option<ParamUnit>, at_least_px: f32| ContribDef {
        params: vec![ParamSpec {
            key: "radius".into(),
            label: "Radius".into(),
            visible_when: None,
            kind: ParamKind::Float {
                min: 0.0,
                max: 64.0,
                default: 12.0,
                step: None,
                unit,
            },
        }],
        downsample: Some(DownsampleSpec {
            param: "radius".into(),
            at_least_px,
        }),
        ..padded_filter_def("soft", None)
    };
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return kl_sample_a(uv); }";
    let shader = ShaderEffect::assemble_untrusted(&soft(Some(ParamUnit::Px), 8.0), Filter, body)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let rule = shader.downsample.expect("a px setting from 8 px");
    assert_eq!((rule.param_offset, rule.at_least_px), (0, 8.0));
    for (unit, at_least_px, says) in [
        (None, 8.0, "px"),
        (Some(ParamUnit::Percent), 8.0, "px"),
        (Some(ParamUnit::Px), 2.0, "4 px"),
        (Some(ParamUnit::Px), f32::NAN, "4 px"),
    ] {
        let d =
            ShaderEffect::assemble_untrusted(&soft(unit, at_least_px), Filter, body).unwrap_err();
        assert_eq!(d[0].code, "rule.downsample", "{d:?}");
        assert!(d[0].message.contains(says), "{}", d[0].message);
    }
}

/// `p` planned at exactly `at_us`.
fn plan_at(
    p: &Project,
    registry: &std::sync::Arc<klipolo_engine::registry::Registry>,
    at_us: Us,
) -> klipolo_engine::plan::GeneratorLayer {
    let plan = frame_plan(
        &ctx_with(p.clone(), registry.clone()),
        at_us,
        FormatKind::Landscape,
    );
    generator_of(&plan).clone()
}

/// Wherever on the project's frame grid a stepped generator starts, at 24, 30 and 60 fps, its steps at half the
/// project's rate hold exactly two frames each and its `kl.frame` rises with each step; an unstepped one's `kl.frame`
/// counts its frames on the 30 fps grain clock exactly. Local time is a difference of two rounded frame starts, so it
/// can fall 1 µs short of a boundary they share.
#[test]
fn steps_and_frames_hold_evenly_wherever_the_layer_starts() {
    for (rate, step) in [(24u32, 12u32), (30, 15), (60, 30)] {
        let stepped = generator_def("stepped", None, Some(step));
        let smooth = generator_def("smooth", None, None);
        let registry = registry_of(&[
            (stepped.clone(), Generator, MOVING),
            (smooth.clone(), Generator, MOVING),
        ]);
        for start in 0..=5i64 {
            let layer = |def: &ContribDef| {
                let mut p = generator_piece_project(def, 0.0, 4.0, None, Vec::new());
                p.settings.fps = rate;
                p.timeline.tracks[0].items[0].start_us = frame_start_us(start, rate);
                p
            };
            let (stepped_p, smooth_p) = (layer(&stepped), layer(&smooth));
            let frames = start..start + 48;
            let held: Vec<(f32, Option<u32>)> = frames
                .clone()
                .map(|n| {
                    let g = plan_at(&stepped_p, &registry, frame_start_us(n, rate));
                    (g.source.time_s, g.source.frame)
                })
                .collect();
            // Run lengths of equal time, the last (cut short by the window) left out.
            let mut runs = vec![1];
            for w in held.windows(2) {
                if w[1].0 == w[0].0 {
                    *runs.last_mut().unwrap() += 1;
                } else {
                    assert!(
                        w[1].1 > w[0].1,
                        "{rate} fps from frame {start}: kl.frame {held:?}"
                    );
                    runs.push(1);
                }
            }
            runs.pop();
            assert!(
                runs.iter().all(|&r| r == 2),
                "{rate} fps, {step} fps steps, from frame {start}: holds {runs:?}"
            );
            // An unstepped generator's `kl.frame` is exactly its frames since the layer began on the 30 fps grain
            // clock, `(n − start) · 30 / rate`, at every rate, so dropping the frame's slack fails here at 24 and 60
            // fps too.
            let smooth: Vec<Option<u32>> = frames
                .clone()
                .map(|n| {
                    plan_at(&smooth_p, &registry, frame_start_us(n, rate))
                        .source
                        .frame
                })
                .collect();
            let grain: Vec<Option<u32>> = frames
                .map(|n| Some(((n - start) * 30 / i64::from(rate)) as u32))
                .collect();
            assert_eq!(
                smooth, grain,
                "{rate} fps from frame {start}: kl.frame against the grain clock"
            );
        }
    }
}

/// A split keeps the seed. Both pieces of a generator layer split by the editor (`Op::SplitItem`, which gives the
/// second piece an id of its own) seed their passes, the generator's and its filters', as the whole layer did, so
/// `kl.seed` and `kl_rand` patterns continue across the cut.
#[test]
fn a_split_keeps_the_whole_layers_seed() {
    let moving = generator_def("moving", None, Some(12));
    let sparkle = padded_filter_def("sparkle", None);
    let registry = registry_of(&[
        (moving.clone(), Generator, MOVING),
        (sparkle, Filter, SPARKLE),
    ]);
    let whole = generator_piece_project(&moving, 0.0, 6.0, None, vec![fx("sparkle")]);
    let split = split_at(&whole, "gen", 2.0);
    let ids: Vec<&str> = split.timeline.tracks[0]
        .items
        .iter()
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "a split gives the second piece an id of its own"
    );
    for t in [0.5, 2.5, 4.0] {
        let (w, s) = (
            plan_at(&whole, &registry, secs(t)),
            plan_at(&split, &registry, secs(t)),
        );
        assert_eq!(s.source.seed, w.source.seed, "at {t} s");
        assert_eq!(s.effects[0].seed, w.effects[0].seed, "its filter at {t} s");
        assert_eq!(
            (s.source.time_s, s.source.frame),
            (w.source.time_s, w.source.frame)
        );
    }
}

/// A generator's `kl.frame`, and its filters', count on the 30 fps grain clock every other layer's grain counts on,
/// whatever the project's frame rate, so grain refreshes at 30 Hz on every layer.
#[test]
fn generator_frames_count_on_the_grain_clock_at_any_project_rate() {
    let moving = generator_def("moving", None, None);
    let sparkle = padded_filter_def("sparkle", None);
    let registry = registry_of(&[
        (moving.clone(), Generator, MOVING),
        (sparkle, Filter, SPARKLE),
    ]);
    for rate in [24u32, 25, 30, 50, 60] {
        let mut p = generator_piece_project(&moving, 0.0, 6.0, None, vec![fx("sparkle")]);
        p.settings.fps = rate;
        for n in 0..120 {
            let at = frame_start_us(n, rate);
            let g = plan_at(&p, &registry, at);
            let grain = u32::try_from(frame_index(at, 30)).unwrap();
            assert_eq!(g.source.frame, Some(grain), "{rate} fps, frame {n}");
            assert_eq!(
                g.effects[0].frame,
                Some(grain),
                "its filter, {rate} fps, frame {n}"
            );
        }
    }
}

/// `kl.fps` is the project's frame rate on every pass (a clip's effects, an overlay's, a generator's
/// own pass and its filters, a transition), and a generator's step rate is `kl.step_fps` (0 when its time runs
/// smoothly).
#[test]
fn every_pass_reads_the_projects_fps_and_a_generator_its_step_rate() {
    let stepped = generator_def("stepped", None, Some(12));
    let sparkle = padded_filter_def("sparkle", None);
    let registry = registry_of(&[
        (stepped.clone(), Generator, MOVING),
        (sparkle, Filter, SPARKLE),
    ]);
    for rate in [24u32, 60] {
        let mut p = generator_piece_project(&stepped, 0.0, 6.0, None, vec![fx("sparkle")]);
        p.settings.fps = rate;
        let g = plan_at(&p, &registry, secs(1.0));
        let fps = rate as f32;
        assert_eq!((g.source.fps, g.source.step_fps), (fps, 12.0), "{rate}");
        assert_eq!(
            (g.effects[0].fps, g.effects[0].step_fps),
            (fps, 0.0),
            "{rate}"
        );

        // The main footage's clip effects, and a transition across the cut at 20 s (window 19.5–20.5 s).
        let mut p = project();
        p.settings.fps = rate;
        p.timeline.main[0].effects = vec![fx(builtin::EFFECT_VIGNETTE)];
        p.timeline.main[1].source_in_us = secs(2.0);
        let p = klipolo_model::apply_ops(
            &p,
            &[Op::SetTransition {
                clip_id: "a".into(),
                transition: Some(klipolo_model::timeline::Transition {
                    effect: builtin::TRANSITION_DISSOLVE.into(),
                    duration_us: secs(1.0),
                    params: Default::default(),
                }),
            }],
        )
        .unwrap();
        let c = ctx(p);
        let clip = frame_plan(&c, secs(1.0), FormatKind::Landscape);
        assert_eq!(videos(&clip)[0].effects[0].fps, fps, "{rate}");
        let cut = frame_plan(&c, secs(19.75), FormatKind::Landscape);
        let Some(klipolo_engine::plan::Layer::Transition(t)) = cut.layers.first() else {
            panic!("a transition at 19.75 s: {:?}", cut.layers);
        };
        assert_eq!(
            (t.pass.fps, t.pass.step_fps, t.pass.duration_s),
            (fps, 0.0, 1.0),
            "{rate}"
        );
    }
}
