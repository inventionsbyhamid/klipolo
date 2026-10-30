//! A scene drawn into a target of its own box (lavapipe in CI): a frame held while the scene
//! holds still is dropped when its layer gains an effect or rounded corners (which read the whole layer's target), so
//! the next frame is the one a fresh preview draws; and an animated scene with no text is sized off the frame that
//! needs it (its survey made on the text build thread), so its first frame packs only itself.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::registry::Registry;
use klipolo_model::contrib::builtin;
use klipolo_model::project::Project;
use klipolo_model::timeline::EffectInstance;
use support::*;

const HD: (u32, u32) = (1280, 720);

/// `p` with its scene layer `id` changed by `edit`.
fn edited(
    p: &Project,
    id: &str,
    edit: impl FnOnce(&mut klipolo_model::timeline::Item),
) -> Arc<Project> {
    let mut p = p.clone();
    edit(scene_item(&mut p, id));
    Arc::new(p)
}

/// The frame a fresh preview draws of `p` at `t`.
async fn fresh(p: &Arc<Project>, registry: &Arc<Registry>, t: f64) -> image::RgbaImage {
    let mut h = Headless::new(registry.clone()).await.unwrap();
    scene_frame(&mut h, p, t, HD).await.0
}

/// A lower third held still (its frame drawn into a target of its card's box) gains a blur effect, then rounded
/// corners: each next frame is the one a fresh preview draws, not the held part without them.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_held_part_shows_an_effect_or_corners_added_to_its_layer() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 4.0);
    let mut h = Headless::new(registry.clone()).await.unwrap();
    let plain = Arc::new(p.clone());
    for t in [1.5, 1.6] {
        scene_frame(&mut h, &plain, t, HD).await;
    }
    assert!(
        h.scene_stats().held_frames > 0,
        "held: {:?}",
        h.scene_stats()
    );
    let blurred = edited(&p, "lt", |item| {
        item.effects.push(EffectInstance {
            id: "fx".into(),
            ..EffectInstance::new(builtin::EFFECT_BLUR)
        })
    });
    let rounded = edited(&p, "lt", |item| item.layout.radius = 0.25);
    for (what, edit) in [("a blur", blurred), ("rounded corners", rounded)] {
        let (img, _) = scene_frame(&mut h, &edit, 1.7, HD).await;
        let want = fresh(&edit, &registry, 1.7).await;
        let (within, mean) = klipolo_engine::test_support::compare(&img, &want, 1);
        assert!(
            within >= 0.9999 && mean < 0.01,
            "with {what}: {:.3} % within 1/255 of a fresh frame",
            within * 100.0
        );
        // Back to the plain layer, held again, before the next edit.
        for t in [1.8, 1.9] {
            scene_frame(&mut h, &plain, t, HD).await;
        }
    }
}

/// A dot grid that moves all the time (no text) is surveyed on the text build thread, not on its first frame;
/// its frames take the survey once it lands, and then make no GPU object.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_moving_scene_without_text_is_surveyed_off_its_frame() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("dots", DOT_GRID, 0.0, 4.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &p, 0.1, HD).await;
    let first = h.scene_stats();
    assert_eq!(first.surveys, 0, "none on the first frame: {first:?}");
    let mut adopted = false;
    for i in 0..60 {
        scene_frame(&mut h, &p, 0.15 + f64::from(i) / 60.0, HD).await;
        if h.scene_stats().survey_built_us > 0.0 {
            adopted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(adopted, "its survey lands: {:?}", h.scene_stats());
    for t in [1.2, 1.25, 1.3] {
        scene_frame(&mut h, &p, t, HD).await;
    }
    let before = klipolo_engine::render::effects::gpu_objects_made();
    for i in 0..20 {
        scene_frame(&mut h, &p, 1.35 + f64::from(i) / 30.0, HD).await;
    }
    let made = klipolo_engine::render::effects::gpu_objects_made() - before;
    assert_eq!(made, 0, "20 moving frames made {made} GPU objects");
    assert_eq!(h.scene_stats().surveys, 0, "never on a frame");
}

/// A scene whose box outgrows its first target before its survey lands (here it never does)
/// moves to the layer's whole target, made in the place of the first: the pool never holds both, so its peak is the
/// whole target alone, not the first one beside it for up to 9 frames.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scene_that_outgrows_its_first_target_holds_one() {
    use klipolo_engine::render::effects::{effect_target_peak, reset_effect_target_peak};
    let _gpu = gpu_turn().await;
    let grows = r##"{"scene":1,"duration":4,"nodes":[
      {"type":"rect","id":"r","box":[0.45,0.45,0.1,0.1],"fill":"#E04020",
       "animate":{"scale":[{"t":0,"v":1},{"t":1,"v":1},{"t":1.2,"v":9}]}}]}"##;
    let (p, registry) = project_with_scene("grows", grows, 0.0, 4.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    h.delay_scene_text_builds(std::time::Duration::from_secs(60));
    for t in [0.2, 0.3] {
        scene_frame(&mut h, &p, t, HD).await;
    }
    reset_effect_target_peak();
    scene_frame(&mut h, &p, 1.5, HD).await;
    let whole = u64::from(HD.0) * u64::from(HD.1) * 4;
    let peak = effect_target_peak();
    assert!(
        peak <= whole,
        "the pool held {peak} bytes at the jump, past the whole target's {whole}: {:?}",
        h.scene_stats()
    );
}

/// A moving scene without text whose rect blurs (an offscreen node)
/// asks the build thread for its survey and, that survey held back, is surveyed on its first frame; the build thread's,
/// landing later, isn't adopted again (it could shrink a class grown since, to grow it once more).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_survey_landing_after_the_first_frame_surveyed_isnt_adopted() {
    let _gpu = gpu_turn().await;
    let blurred = r##"{"scene":1,"duration":4,"nodes":[
      {"type":"rect","id":"r","box":[0.1,0.4,0.2,0.2],"fill":"#E04020","blur":6}],
      "timeline":[{"to":"#r","at":0,"dur":4,"x":"400px"}]}"##;
    let (p, registry) = project_with_scene("blurred", blurred, 0.0, 4.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    h.delay_scene_text_builds(std::time::Duration::from_millis(300));
    scene_frame(&mut h, &p, 0.1, HD).await;
    assert_eq!(
        h.scene_stats().surveys,
        1,
        "surveyed on its first frame: {:?}",
        h.scene_stats()
    );
    // Well past the build thread's 300 ms: its survey has landed and been taken.
    for i in 0..40 {
        scene_frame(&mut h, &p, 0.15 + f64::from(i) / 60.0, HD).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let stats = h.scene_stats();
    assert_eq!(stats.survey_built_us, 0.0, "not adopted again: {stats:?}");
    assert_eq!(stats.surveys, 1, "{stats:?}");
}
