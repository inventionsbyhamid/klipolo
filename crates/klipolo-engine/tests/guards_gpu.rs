//! The main app's guards on the GPU. In the app's preview
//! (`RenderMode::Interactive`) a checked shader compiles off the drawing thread and draws only once the app said its
//! crash-guard mark is on disk; it is pre-compiled, with its half-size step, as soon as it is placed; the preview keeps
//! every pipeline its project uses; a device loss names what drew; the passes are sampled and slowed when over budget.
//! Everywhere else (`Contained`: the export and the render worker) checked shaders draw on frame 0, and an export draws
//! a slow pass in tiles with the same result. Drawn for real (lavapipe in CI).

mod support;
use support::*;

use klipolo_engine::api::EngineEvent;
use klipolo_engine::headless::Headless;
use klipolo_engine::make::budget::{Cost, CostClass};
use klipolo_engine::make::measure::FakeMeter;
use klipolo_engine::registry::pack::Verdict;
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
use klipolo_engine::render::effects::sampling::Degrade;
use klipolo_engine::render::mode::RenderMode;
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind};
use klipolo_model::project::FormatKind;
use std::time::Duration;

/// A "halo" filter: white wherever the layer is transparent within 12 px of opaque pixels (as `padding_gpu.rs`'s).
const HALO: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let here = kl_sample_a(uv);
    if (here.a > 0.0) { return here; }
    let r = 12.0 * kl.px / kl.size;
    let near = max(max(kl_sample_a(uv + vec2<f32>(r.x, 0.0)).a, kl_sample_a(uv - vec2<f32>(r.x, 0.0)).a),
                   max(kl_sample_a(uv + vec2<f32>(0.0, r.y)).a, kl_sample_a(uv - vec2<f32>(0.0, r.y)).a));
    return vec4<f32>(near);
}";

/// 5 px left of the red image's box: white only where the halo drew.
const OUTSIDE: (u32, u32) = (395, 350);

/// A branch the measurement never took (`detail` is 0 by default): measured cheap, statically heavy (some 60,000
/// operations a pixel, about 600 ms at 1080p by the static prediction).
const BRANCH_GATED_HEAVY: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var c = vec4<f32>(uv, 0.0, 1.0);
    if (params.detail > 0.5) {
        for (var i = 0; i < 4000; i++) {
            c = vec4<f32>(c.rgb * 0.999 + vec3<f32>(sin(f32(i) * uv.x) * 0.001), 1.0);
        }
    }
    return clamp(c, vec4<f32>(0.0), vec4<f32>(1.0));
}";

fn first_draw(e: &EngineEvent) -> bool {
    matches!(e, EngineEvent::FirstDraw { .. })
}

fn drew_safely(e: &EngineEvent) -> bool {
    matches!(e, EngineEvent::DrewSafely { .. })
}

fn halo_id() -> ContribId {
    ContribId::new("com.test.fx", "effect.halo")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_checked_shader_compiles_off_the_drawing_thread_and_waits_for_its_guard_mark() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await; // the app's preview mode
    let first = h.render_at(secs(0.1)).await; // the pipeline isn't ready: drawn without the effect
    assert_eq!(pixel_at(&first, OUTSIDE), [0, 0, 0]);
    h.acknowledge_marks(); // its compile mark: only now does the compile thread get it
    h.wait_for_pipelines().await;
    let unmarked = h.render_at(secs(0.2)).await; // ready, but the draw mark isn't acknowledged: still without it
    assert_eq!(pixel_at(&unmarked, OUTSIDE), [0, 0, 0]);
    assert_eq!(
        h.count(first_draw),
        2,
        "the compile's mark, then the draw's"
    );
    h.acknowledge_marks();
    let later = h.render_at(secs(0.3)).await;
    assert_eq!(pixel_at(&later, OUTSIDE), [255, 255, 255]);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn sixty_frames_that_include_it_say_it_drew_safely_once() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await;
    h.render_at(0).await;
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    h.render_at(0).await;
    h.acknowledge_marks();
    let compiled = h.count(drew_safely); // the compile's mark closed when it returned
    // 60 frames a second, inside the image's 2 s, small so that 60 of them take well under the 2 s bound.
    let small = (320, 180);
    for n in 1..=59 {
        h.render_sized(secs(f64::from(n) / 60.0), small).await;
    }
    assert_eq!(h.count(drew_safely), compiled, "59 frames drew it");
    // The 60 count from when the GPU finished its first drawing frame: with at most two frames in
    // flight, the 60th of those comes by the 63rd frame.
    for n in 60..=63 {
        h.render_sized(secs(f64::from(n) / 60.0), small).await;
    }
    assert_eq!(
        h.count(drew_safely),
        compiled + 1,
        "the 60th since then did"
    );
    for n in 64..=70 {
        h.render_sized(secs(f64::from(n) / 60.0), small).await;
    }
    assert_eq!(h.count(drew_safely), compiled + 1, "said once");
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, EngineEvent::DrewSafely { contrib, .. } if *contrib == halo_id()))
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn placing_a_checked_effect_compiles_it_and_its_half_size_step_before_a_frame_needs_them() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await;
    assert!(!h.h.half_size_step_ready());
    h.h.prepare_registry(); // the feed brought the registry that places it (the session's actor does this)
    let halo_first = h
        .events()
        .iter()
        .filter(|e| matches!(e, EngineEvent::FirstDraw { contrib, .. } if *contrib == halo_id()))
        .count();
    assert_eq!(
        halo_first, 1,
        "its compile mark, before any frame asked for it"
    );
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    assert!(
        h.h.checked_pipelines_kept() >= 1,
        "compiled before any frame"
    );
    assert!(
        h.h.half_size_step_ready(),
        "the half-size step's pipeline too"
    );
    h.render_at(secs(0.1)).await; // its draw mark is asked for now
    h.acknowledge_marks();
    let drawn = h.render_at(secs(0.2)).await;
    assert_eq!(
        pixel_at(&drawn, OUTSIDE),
        [255, 255, 255],
        "nothing left to compile"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_preview_keeps_every_pipeline_its_project_uses_and_lets_go_of_the_rest() {
    // The pipeline cache's cap (`MAX_PIPELINES`) is 32 pipelines that aren't built in: a project using 40 keeps all 40
    // in the preview (none is compiled again when the playhead comes back to it), and a registry without them lets them
    // go.
    let (p, registry) = project_with_many_checked_filters(40);
    let mut h = Headless::with_mode(registry, RenderMode::Interactive)
        .await
        .unwrap();
    h.prepare_registry();
    let asked = acknowledge(&mut h, 0);
    assert_eq!(
        asked, 41,
        "the white generator's and the 40 filters' compile marks"
    );
    h.wait_for_pipelines().await;
    let p = std::sync::Arc::new(p);
    h.render(p.clone(), secs(0.5), FormatKind::Landscape, (640, 360))
        .await
        .unwrap();
    assert_eq!(h.checked_pipelines_kept(), 41);
    h.set_registry(Registry::builtin());
    h.prepare_registry();
    assert_eq!(h.checked_pipelines_kept(), 0);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_device_loss_names_the_checked_effects_drawn_in_the_last_60_frames() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await;
    h.render_at(0).await;
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    h.render_at(secs(0.05)).await;
    h.acknowledge_marks();
    let drawn = h.render_at(secs(0.1)).await;
    assert_eq!(pixel_at(&drawn, OUTSIDE), [255, 255, 255]);
    let asked = h.count(first_draw);
    h.h.simulate_device_loss();
    h.render_at(secs(0.2)).await; // the device and the compositor are made again
    let lost: Vec<&Vec<ContribId>> = h
        .events()
        .iter()
        .filter_map(|e| match e {
            EngineEvent::GpuLost { contribs } => Some(contribs),
            _ => None,
        })
        .collect();
    assert_eq!(lost, [&vec![halo_id()]]);
    // On the new device it asks for a new mark before it compiles.
    assert!(h.count(first_draw) > asked);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_export_draws_a_slow_pass_in_tiles_with_the_same_result() {
    let whole = export_frame_with_checked_generator(WAVES_BODY, Cost::with_export_ms(50.0)).await;
    let tiled = export_frame_with_checked_generator(WAVES_BODY, Cost::with_export_ms(900.0)).await; // → 5 tiles
    assert_eq!(whole.submissions, 1);
    assert_eq!(tiled.submissions, 5);
    assert!(whole.rgba == tiled.rgba, "the same image");
    // Measured cheap, but statically heavy (a branch the measurement never took): tiled by the static prediction.
    let gated =
        export_frame_with_checked_generator(BRANCH_GATED_HEAVY, Cost::with_export_ms(5.0)).await;
    assert!(gated.submissions > 1, "{}", gated.submissions);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_apps_preview_stays_interactive_also_after_a_device_loss() {
    // The engine session the app builds, through every place that makes its compositor.
    let mut backend = app_preview_backend().await; // what `commands/engine.rs` builds
    assert_eq!(backend.mode(), RenderMode::Interactive);
    assert_eq!(
        backend.warm_compositor_mode().await,
        Some(RenderMode::Interactive)
    );
    backend.simulate_device_loss(); // ensure_device rebuilds the device and compositor
    render_blank_frame(&mut backend).await;
    assert_eq!(backend.compositor_mode(), Some(RenderMode::Interactive));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_export_worker_and_the_render_worker_draw_checked_effects_on_frame_0() {
    // Contained mode: no async compile, no handshake.
    let mut job = export_job_with_checked_filter(HALO).await;
    let first = job.render_frame(0).await;
    assert_eq!(pixel_at(&first, OUTSIDE), [255, 255, 255]);
    let mut w = InProcessWorker::new(FakeMeter::new(2.0)).await;
    // The worker's test image is grey and white; this filter paints it black.
    let black = ContribDef::new("lib.k3f9q2ma/effect.black.v1", ContribKind::Effect, "Black");
    let compiled = w
        .compile(
            black,
            "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }",
        )
        .await;
    let frames = w
        .frames_alone(compiled.compiled().unwrap(), &[0.0], 256)
        .await;
    assert!(
        frames[0].stats.luma.max < 0.1,
        "the worker's frames include the effect: {:?}",
        frames[0].stats.luma
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_sampler_times_checked_passes_on_the_gpu_and_slows_one_down() {
    // A full-frame generator verified Light (0.5 ms) is far over twice that on lavapipe at 1080p. Sampled every frame
    // here, on a clock that moves a second a frame, it goes to half size, then to a still (a generator).
    let def = draft_def();
    let mut cost = Cost::with_export_ms(1.0);
    cost.class = CostClass::Light;
    cost.budget_ms = 0.5;
    let shader = ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, WAVES_BODY)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let shader = shader.verified(&Verdict::of(cost).unwrap());
    let registry = Registry::builtin_with_test_shader(&def, shader);
    let mut h = Headless::with_mode(registry, RenderMode::Interactive)
        .await
        .unwrap();
    assert!(h.device().has_timestamps(), "lavapipe times passes");
    h.sample_with_clock(1, 1.0);
    let p = std::sync::Arc::new(generator_piece_project(&def, 0.0, 30.0, None, Vec::new()));
    h.prepare_registry();
    let done = acknowledge(&mut h, 0); // the compile mark
    h.wait_for_pipelines().await;
    h.render(p.clone(), 0, FormatKind::Landscape, (1920, 1080))
        .await
        .unwrap();
    acknowledge(&mut h, done); // the draw mark
    let mut notes = Vec::new();
    for n in 0..24 {
        let f = h
            .render(
                p.clone(),
                secs(f64::from(n) / 30.0),
                FormatKind::Landscape,
                (1920, 1080),
            )
            .await
            .unwrap();
        notes.extend(f.warnings);
    }
    let steps: Vec<Degrade> = h
        .events()
        .iter()
        .filter_map(|e| match e {
            EngineEvent::EffectDegraded {
                contrib,
                to,
                ms,
                budget_ms,
            } => {
                assert_eq!(*contrib, def.id);
                assert!(*ms > 1.0 && *budget_ms == 0.5, "{ms} against {budget_ms}");
                Some(*to)
            }
            _ => None,
        })
        .collect();
    assert_eq!(steps, [Degrade::Half, Degrade::Still]);
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.starts_with(klipolo_engine::api::FRAME_AGENT_EFFECT_SLOW))
            .count(),
        1,
        "said once: {notes:?}"
    );
}

/// Placing and compiling an effect leave no mark open; the first frame that draws it asks for one,
/// which closes after 60 frames that drew it (a static generator's cache hits count) or 2 s after its first draw.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_mark_opens_only_when_a_frame_draws_and_closes_after_60_frames_or_2_s() {
    let mut h = headless_with_static_checked_generator(RenderMode::Interactive).await;
    h.h.prepare_registry();
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    h.acknowledge_marks();
    assert_eq!(
        h.h.open_marks(),
        0,
        "placing and compiling it leave no mark open"
    );
    h.render_at(secs(0.1)).await; // the first frame that needs it asks for its mark
    h.acknowledge_marks();
    let before = std::time::Instant::now();
    // 60 frames from when the GPU finished the first: at most two more than 60 in flight.
    for n in 0..63 {
        // One picture, from the static cache; small, so these frames take well under the 2 s bound.
        h.render_sized(secs(0.2 + f64::from(n) / 100.0), (320, 180))
            .await;
    }
    assert!(
        before.elapsed() < Duration::from_secs(2),
        "the frames closed it, not the time"
    );
    assert_eq!(
        h.h.open_marks(),
        0,
        "60 frames drew it, cache hits included"
    );
    // A mark drawn once (a still, paused) closes 2 s after that first draw, with no more frames.
    let mut h = headless_with_static_checked_generator(RenderMode::Interactive).await;
    h.h.prepare_registry();
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    h.render_at(secs(0.1)).await;
    h.acknowledge_marks();
    h.render_at(secs(0.1)).await;
    assert_eq!(h.h.open_marks(), 1, "drawn once");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    h.h.guard_tick();
    assert_eq!(h.h.open_marks(), 0, "2 s after its first draw");
}

/// The compile thread parses and compiles a checked shader only once its mark is on disk, and that
/// mark closes when the compile returns.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_checked_shader_is_compiled_only_after_its_mark_is_acknowledged() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await;
    h.render_at(secs(0.1)).await;
    h.wait_for_pipelines().await;
    assert_eq!(
        h.h.pipelines_pending() + h.h.checked_pipelines_kept(),
        0,
        "nothing compiled before its mark"
    );
    assert_eq!(h.count(first_draw), 1, "its mark asked for");
    assert!(
        !h.h.builtins_ready(),
        "the shader presets wait behind the first agent compile"
    );
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    assert_eq!(
        h.h.checked_pipelines_kept(),
        1,
        "compiled once acknowledged"
    );
    assert!(h.h.builtins_ready(), "and the presets after it");
    assert_eq!(
        h.h.open_marks(),
        0,
        "the compile's mark closed when it returned"
    );
}

/// In the app's preview the built-ins are queued on the compile thread ahead of
/// any agent compile, so placing a checked effect compiles nothing on the drawing thread, and no frame waits on the
/// scope lock behind an agent's compile.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_built_ins_compile_on_the_compile_thread_before_any_agent_compile() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await;
    let before = h.h.drawing_thread_compiles();
    h.h.prepare_registry();
    assert_eq!(
        h.h.drawing_thread_compiles(),
        before,
        "placing it compiled nothing on the drawing thread"
    );
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    assert!(
        h.h.builtins_ready(),
        "the built-ins came from the compile thread"
    );
    h.render_at(secs(0.1)).await;
    assert_eq!(
        h.h.drawing_thread_compiles(),
        before,
        "nor did a frame after it"
    );
}

/// A shader preset first needed while an agent's compile runs (the presets wait behind it on the compile thread) is
/// left out of that frame, as a checked shader is, rather than compiled on the drawing thread behind the agent's
/// compile (the device's scope lock); once handed back it draws, compiled once.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_preset_first_needed_during_an_agent_compile_never_compiles_on_the_drawing_thread() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await;
    h.render_at(secs(0.1)).await; // the halo's compile mark asked for; the other built-ins to the compile thread
    h.wait_for_pipelines().await;
    let dir = tempfile::tempdir().unwrap();
    let glowing = std::sync::Arc::new(image_project(
        dir.path(),
        [255, 0, 0],
        (400, 300, 100),
        vec![fx("halo"), fx("klipolo.builtin/effect.glow")],
    ));
    let before = h.h.drawing_thread_compiles();
    h.acknowledge_marks(); // the halo's compile goes to the compile thread, the presets behind it
    h.h.render(
        glowing.clone(),
        secs(0.1),
        FormatKind::Landscape,
        (640, 360),
    )
    .await
    .unwrap();
    assert_eq!(
        h.h.drawing_thread_compiles(),
        before,
        "the glow waited for the compile thread"
    );
    h.wait_for_pipelines().await;
    h.h.render(glowing, secs(0.1), FormatKind::Landscape, (640, 360))
        .await
        .unwrap();
    assert_eq!(
        h.h.drawing_thread_compiles(),
        before,
        "and drew from its pipeline once handed back"
    );
}

/// A preview frame registers a work-done callback for the crash guard (a boxed closure, a poll)
/// only while a draw mark waits for the GPU to finish its first drawing frame; in steady state it adds none.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_frame_with_no_mark_waiting_for_the_gpu_registers_no_callback() {
    let mut h = headless_with_checked_filter(HALO, RenderMode::Interactive).await;
    for n in 0..5 {
        h.render_at(secs(f64::from(n) / 10.0)).await; // only a compile mark asked: nothing drawn yet
    }
    assert_eq!(h.h.done_callbacks(), 0, "no mark waits for the GPU");
    h.acknowledge_marks();
    h.wait_for_pipelines().await;
    h.render_at(secs(0.6)).await; // the draw mark is asked for
    h.acknowledge_marks();
    h.render_at(secs(0.7)).await; // its first drawing frame: it waits for the GPU
    assert!(
        h.h.done_callbacks() >= 1,
        "the frame it waits for is signalled"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    h.h.guard_tick(); // the GPU finished it: its 2 s run, it waits for no frame any more
    let steady = h.h.done_callbacks();
    for n in 8..12 {
        h.render_at(secs(f64::from(n) / 10.0)).await;
    }
    assert_eq!(h.h.done_callbacks(), steady, "steady state: none");
}
