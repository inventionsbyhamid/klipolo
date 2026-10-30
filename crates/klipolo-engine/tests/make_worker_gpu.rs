//! The render worker's GPU work: compile with its thumbnail and notes, frames as bytes with their
//! statistics, project frames from the app's stills clamped to their envelopes, text in the app's fonts, and the draft
//! layers of filters and transitions. Measurements use `FakeMeter`; the drawing is real (lavapipe in CI). The
//! measurement itself is tested in `make_measure_gpu.rs`.

#[path = "make_worker_gpu/audit.rs"]
mod audit;
mod support;
#[path = "make_worker_gpu/verify.rs"]
mod verify;
use klipolo_engine::make::measure::FakeMeter;
use support::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn compile_measures_frames_and_stats_for_a_good_generator() {
    let mut w = InProcessWorker::new(FakeMeter::new(2.0)).await;
    let reply = w.compile(draft_def(), WAVES_BODY).await;
    let c = reply.compiled().unwrap();
    assert!(c.ok);
    let cost = c.cost.as_ref().unwrap();
    assert!(cost.preview_ms.p95 > 0.0 && cost.export_ms.p1080 >= cost.preview_ms.p95);
    assert!(
        image::load_from_memory(&reply.thumbnail().unwrap().decode().unwrap()).is_ok(),
        "bytes on its own line, not a path"
    );
    let frames = w.frames_alone(c, &[0.0, 1.0, 2.0, 3.0], 512).await;
    assert_eq!(frames.len(), 4);
    assert!(
        frames.iter().all(|f| f.stats.luma.max > f.stats.luma.min),
        "not flat"
    );
    assert!(frames[1].stats.change_from_prev.unwrap() > 0.0, "it moves");
    assert!(
        frames
            .iter()
            .all(|f| (f.stats.alpha_coverage - 1.0).abs() < 1e-6)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn nan_pixels_are_counted_and_black_or_static_output_is_said_in_numbers() {
    let mut w = InProcessWorker::new(FakeMeter::new(2.0)).await;
    let nan = w
        .compile(
            draft_def(),
            "fn effect(uv: vec2<f32>) -> vec4<f32> { let z = uv.x - uv.x; return vec4<f32>(z / z); }",
        )
        .await;
    let frames = w.frames_alone(nan.compiled().unwrap(), &[0.0], 256).await;
    assert!(frames[0].stats.nan_fixed > 0);
    let still = w
        .compile(
            draft_def(),
            "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }",
        )
        .await;
    let f = w
        .frames_alone(still.compiled().unwrap(), &[0.0, 1.0], 256)
        .await;
    assert_eq!(f[0].stats.luma.max, 0.0);
    assert_eq!(f[1].stats.change_from_prev, Some(0.0));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn project_frames_come_from_the_apps_stills_and_clamp_to_the_envelope() {
    let mut w = InProcessWorker::new(FakeMeter::new(2.0)).await;
    let frames = w
        .frames_project_with_stills(video_project(), &[0.5], solid_still([255, 0, 0]))
        .await;
    let red = pixel_of(&frames[0], (10, 10));
    assert!(
        near(red, [255, 0, 0], 4),
        "the still the app decoded, not a file it opened: {red:?}"
    );
    // A checked generator whose envelope bounds `steps` at 22, placed with 60, draws as it does at 22.
    let clamped = w
        .frames_project_with_contrib(rays_project(60.0), rays_contrib_with_envelope(22.0), &[0.5])
        .await;
    let at_22 = w
        .frames_project_with_contrib(rays_project(22.0), rays_contrib_with_envelope(22.0), &[0.5])
        .await;
    let at_60 = w
        .frames_project_with_contrib(rays_project(60.0), rays_contrib_with_envelope(64.0), &[0.5])
        .await;
    assert_eq!(clamped[0].jpeg_decoded(), at_22[0].jpeg_decoded());
    assert_ne!(
        clamped[0].jpeg_decoded(),
        at_60[0].jpeg_decoded(),
        "60 draws otherwise"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_worker_draws_text_in_the_bundled_faces() {
    let mut w = InProcessWorker::new(FakeMeter::new(2.0)).await;
    let frames = w
        .frames_project(text_project("Montserrat", 700, "Hello"), &[0.5], 512)
        .await;
    assert!(
        w.notes()
            .iter()
            .all(|n| !n.starts_with("frame.fontMissing")),
        "configured before the first frame: {:?}",
        w.notes()
    );
    assert!(frames[0].stats.luma.max > 0.5);
}

/// The draft layers of the other kinds: a filter on the 50 %-grey-and-white test image, a transition between two test
/// stills (no decoder), each compiled, measured and thumbnailed; the filter's frame shows the image.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn filters_and_transitions_are_drawn_on_their_test_images() {
    use klipolo_model::contrib::{ContribDef, ContribKind};
    let mut w = InProcessWorker::new(FakeMeter::new(1.0)).await;
    let filter = ContribDef::new("lib.k3f9q2ma/effect.pass.v1", ContribKind::Effect, "Pass");
    let f = w
        .compile(
            filter,
            "fn effect(uv: vec2<f32>) -> vec4<f32> { return kl_sample_a(uv); }",
        )
        .await;
    let c = f.compiled().unwrap();
    assert!(c.ok && c.cost.is_some(), "{:?}", c.diagnostics);
    assert!(image::load_from_memory(&f.thumbnail().unwrap().decode().unwrap()).is_ok());
    let frames = w.frames_alone(c, &[1.0], 512).await;
    assert!(
        frames[0].stats.luma.max > 0.9,
        "the test image's white squares"
    );
    let fade = ContribDef::new(
        "lib.k3f9q2ma/transition.fade.v1",
        ContribKind::Transition,
        "Fade",
    );
    let t = w
        .compile(
            fade,
            "fn transition(uv: vec2<f32>) -> vec4<f32> { return mix(kl_sample_a(uv), kl_sample_b(uv), kl.progress); }",
        )
        .await;
    let c = t.compiled().unwrap();
    assert!(c.ok && c.cost.is_some(), "{:?}", c.diagnostics);
    assert!(image::load_from_memory(&t.thumbnail().unwrap().decode().unwrap()).is_ok());
}

/// A Frames request is stepped first (16², 64², then its size), so a draft too slow to draw is
/// refused before any big frame, whatever it compiled to.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn frames_of_a_draft_too_slow_to_draw_are_refused_before_a_big_frame() {
    let mut w = InProcessWorker::new(FakeMeter::new(400.0)).await;
    let c = w.compile(draft_def(), WAVES_BODY).await;
    let lines = w
        .frames_alone_lines(c.compiled().unwrap(), &[0.0, 1.0], 1024)
        .await;
    assert!(
        lines.iter().all(|l| !l.contains("\"op\":\"frame\"")),
        "no frame drawn: {lines:?}"
    );
    assert!(lines.last().unwrap().contains("\"failed\""), "{lines:?}");
}

/// Two drafts never share a pipeline, even when their 64-bit keys collide (forged here), and a
/// registry swap lets the last draft's pipelines and kept pictures go.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn two_drafts_never_share_a_pipeline_even_with_one_key() {
    use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
    use klipolo_model::contrib::{ContribDef, ContribKind};
    let mut h = headless_alone().await;
    let red = ContribDef::new(
        "lib.k3f9q2ma/generator.red.v1",
        ContribKind::Generator,
        "Red",
    );
    let green = ContribDef::new(
        "lib.k3f9q2ma/generator.green.v1",
        ContribKind::Generator,
        "Green",
    );
    let a = ShaderEffect::assemble_untrusted(
        &red,
        ShaderKind::Generator,
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }",
    )
    .unwrap();
    let mut b = ShaderEffect::assemble_untrusted(
        &green,
        ShaderKind::Generator,
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0, 1.0, 0.0, 1.0); }",
    )
    .unwrap();
    b.key = a.key;
    h.set_registry(Registry::with_checked([(red.clone(), a)]));
    let first = render_pixel(&mut h, &red).await;
    h.set_registry(Registry::with_checked([(green.clone(), b)]));
    let second = render_pixel(&mut h, &green).await;
    assert_eq!(first, [255, 0, 0]);
    assert_eq!(second, [0, 255, 0], "the second draft drew its own text");
}

/// An image of the person's outside `in/` that the app didn't copy is never opened: it draws as
/// missing, and the note says so.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_image_the_app_did_not_copy_is_never_opened() {
    let mut w = InProcessWorker::new(FakeMeter::new(2.0)).await;
    let outside = tempfile::tempdir().unwrap();
    let p = image_project(outside.path(), [255, 0, 0], (0, 0, 400), Vec::new());
    let frames = w.frames_project(p, &[0.5], 512).await;
    let px = pixel_of(&frames[0], (20, 20));
    assert!(
        !near(px, [255, 0, 0], 60),
        "the red file was opened: {px:?}"
    );
    assert!(
        w.notes()
            .iter()
            .any(|n| n.starts_with("frame.mediaMissing")),
        "{:?}",
        w.notes()
    );
}

/// A worker's pipelines don't pile up across drafts: each registry swap lets the last draft's go.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn pipelines_do_not_pile_up_across_drafts() {
    use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
    use klipolo_model::contrib::{ContribDef, ContribKind};
    let mut h = headless_alone().await;
    let mut kept = Vec::new();
    for i in 0..40 {
        let id = format!("lib.k3f9q2ma/generator.pile{i}.v1");
        let def = ContribDef::new(id.as_str(), ContribKind::Generator, "Pile");
        let body = format!(
            "fn effect(uv: vec2<f32>) -> vec4<f32> {{ return vec4<f32>({i}.0 / 40.0, 0.0, 0.0, 1.0); }}"
        );
        let shader = ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, &body).unwrap();
        h.set_registry(Registry::with_checked([(def.clone(), shader)]));
        render_pixel(&mut h, &def).await;
        kept.push(h.effect_pipelines());
    }
    assert_eq!(kept[39], kept[0], "{kept:?}");
}

/// The first pixel of `def`'s generator over the canvas at 1 s, 64 × 36.
async fn render_pixel(
    h: &mut klipolo_engine::headless::Headless,
    def: &klipolo_model::contrib::ContribDef,
) -> [u8; 3] {
    let p = std::sync::Arc::new(generator_piece_project(def, 0.0, 2.0, None, Vec::new()));
    let f = h
        .render(
            p,
            1_000_000,
            klipolo_model::project::FormatKind::Landscape,
            (64, 36),
        )
        .await
        .unwrap();
    [f.bytes[0], f.bytes[1], f.bytes[2]]
}
