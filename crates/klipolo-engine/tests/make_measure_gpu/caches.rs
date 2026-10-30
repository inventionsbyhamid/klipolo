//! Fenced timing takes a slope between a single frame and a batch of its
//! copies, so every copy must pay what the single frame pays. Scenes keep results between frames: a still scene holds
//! its last picture (the hold), and a title keeps its texture once built. A timed frame never shows a held
//! scene picture (`EffectStage::fresh`, as for a stepped generator's kept picture), so a still scene's single frame and
//! every copy draw its passes; a title is built by its first frame and drawn from its texture by every frame after,
//! The timed single and each copy alike. And every copy draws with the uniforms the first wrote
//! identical-uniform assumption, pinned for the merged effect stage).

use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::make::draft::DraftLayer;
use klipolo_engine::registry::Registry;
use klipolo_model::contrib::{ContribDef, ContribKind, builtin};
use klipolo_model::project::FormatKind;

use crate::support::*;

/// The copies a batch here draws.
const COPIES: u32 = 6;
const SIZE: (u32, u32) = (640, 360);

/// A still scene as the worker measures a scene draft (`DraftLayer::of_scene`, its measured copy, on the worker's
/// device): a plain frame drawn again while it holds still shows its held picture (so the hold is live here), but a
/// timed frame and every copy of a batch draw the scene's own passes, as the frame that first drew it did, so a batch's
/// cost a frame isn't below its single frame's. Plain frames hold again after.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_still_scenes_timed_frames_and_batch_copies_each_draw_its_passes() {
    let registry = scene_registry(&[("shapes", SHAPES)]);
    let def = ContribDef::new(scene_id("shapes").as_str(), ContribKind::Scene, "Shapes");
    let scene = registry.scene(&def.id).expect("compiled").clone();
    let layer = DraftLayer::of_scene(
        &def,
        scene,
        registry,
        Default::default(),
        FormatKind::Landscape,
    );
    let mut h = headless_alone().await;
    layer.prepare(&mut h, true, false);
    let p = Arc::new(layer.project());
    let format = layer.format;
    let (a, b) = (secs(0.5), secs(1.0));
    h.render(p.clone(), a, format, SIZE).await.unwrap();
    let drawn = h.passes_last_frame();
    let held = h.scene_stats().held_frames;
    h.render(p.clone(), b, format, SIZE).await.unwrap();
    assert_eq!(
        h.scene_stats().held_frames,
        held + 1,
        "a plain frame while it holds still shows its held picture"
    );
    assert!(
        h.passes_last_frame() < drawn,
        "the held picture draws no pass of the scene's: {} of {drawn}",
        h.passes_last_frame()
    );
    let held = h.scene_stats().held_frames;
    h.render_timed(p.clone(), b, format, SIZE).await.unwrap();
    assert_eq!(
        h.passes_last_frame(),
        drawn,
        "a timed frame draws the scene's passes"
    );
    let batch = h
        .render_batch(p.clone(), b, format, SIZE, COPIES)
        .await
        .unwrap();
    assert_eq!(
        batch.passes,
        vec![drawn; COPIES as usize],
        "every copy draws the scene's passes, none shows a held picture"
    );
    assert_eq!(
        h.scene_stats().held_frames,
        held,
        "no timed frame shows a held picture"
    );
    h.render(p.clone(), a, format, SIZE).await.unwrap();
    h.render(p, b, format, SIZE).await.unwrap();
    assert_eq!(
        h.scene_stats().held_frames,
        held + 1,
        "plain frames hold again after"
    );
}

/// A title under a filter (a text layer with the built-in vignette, so its frame has passes of its own): its first
/// frame builds its texture, and after it neither the timed single frame nor any copy of a batch builds it again; each
/// copy draws the passes the single frame draws, so a batch's cost a frame isn't below its single frame's.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_titles_timed_frames_and_batch_copies_each_draw_what_the_single_frame_draws() {
    let mut p = text_project("Inter", 700, "A title");
    p.timeline.tracks[0].items[0].effects = vec![fx(builtin::EFFECT_VIGNETTE)];
    let p = Arc::new(p);
    let mut h = Headless::new(Registry::builtin()).await.unwrap();
    let (at, format) = (secs(1.0), FormatKind::Landscape);
    h.render(p.clone(), at, format, SIZE).await.unwrap();
    let built = h.text_stats().0.built_here;
    assert!(built > 0, "its first frame builds it");
    let plain = h.passes_last_frame();
    h.render_timed(p.clone(), at, format, SIZE).await.unwrap();
    let single = h.passes_last_frame();
    assert_eq!(
        single, plain,
        "the timed frame draws what a frame of it draws"
    );
    assert!(single > 1, "the vignette's passes: {single}");
    let batch = h
        .render_batch(p.clone(), at, format, SIZE, COPIES)
        .await
        .unwrap();
    assert_eq!(
        batch.passes,
        vec![single; COPIES as usize],
        "each copy draws what the single frame draws"
    );
    assert_eq!(
        h.text_stats().0.built_here,
        built,
        "built once, by its first frame: neither the timed frame nor a copy builds it"
    );
}

/// A generator whose picture moves with `kl.time`.
const MOVING: &str =
    "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, fract(kl.time), 1.0); }";

/// Fenced timing's identical-uniform assumption, pinned for the effect stage: every copy of a batch draws with the
/// same uniforms. The stage writes a pass's uniforms (its `KlFrame` and settings) only when they changed, and a batch's
/// writes all land before its one submission, so the first copy writes what changed since the frame before and no copy
/// after it writes any (were one to, every copy would draw with the last copy's). A generator whose `kl.time` moves and
/// the sample sparkle filter (padded, over the test image), each batched at a time the frame before wasn't.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn every_copy_of_a_batch_draws_with_the_uniforms_the_first_wrote() {
    let mut h = headless_alone().await;
    for layer in [
        draft(draft_def_with("[]"), MOVING),
        draft(sparkle_def(), SPARKLE_BODY),
    ] {
        layer.prepare(&mut h, true, false);
        let p = Arc::new(layer.project());
        let times = layer.times(2);
        let single = h
            .render_timed(p.clone(), times[0], layer.format, SIZE)
            .await
            .unwrap();
        assert_eq!(single.uniform_writes.len(), 1, "{single:?}");
        let batch = h
            .render_batch(p, times[1], layer.format, SIZE, COPIES)
            .await
            .unwrap();
        let id = &layer.def.id;
        assert!(
            batch.uniform_writes[0] > 0,
            "{id:?}: the first copy writes what changed since the frame before: {:?}",
            batch.uniform_writes
        );
        assert_eq!(
            batch.uniform_writes[1..],
            [0; COPIES as usize - 1],
            "{id:?}: no copy after the first writes a uniform"
        );
    }
}
