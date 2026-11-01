//! Scene text when its build thread can't answer (lavapipe in CI): an export never draws a frame without its text and
//! says nothing (it stops, naming why), and the preview says
//! once that a motion graphic's text couldn't be drawn, also when it shows an earlier text in its place. An export frame
//! also always gets its own text while builds asked ahead of it fill the thread's queue.

mod support;
use std::sync::Arc;
use std::time::Duration;

use klipolo_engine::api::{EXPORT_SCENE_TEXT_MISSING, FRAME_SCENE_TEXT_MISSING};
use klipolo_engine::headless::Headless;
use klipolo_model::params::ParamValue;
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::ItemContent;
use support::*;

/// Frames of `p` at `at` until its text has landed (the preview never waits for it).
async fn until_text_lands(h: &mut Headless, p: &Arc<klipolo_model::project::Project>, at: f64) {
    for _ in 0..200 {
        let (img, _) = scene_frame(h, p, at, H_SIZE).await;
        if ink_bounds_in(&img, lower_third_title_box_px()).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the title never landed");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scene_whose_text_cant_be_built_says_so_once_and_an_export_stops() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 4.0);
    let p = Arc::new(p);
    let says = |notes: &[String]| notes.iter().any(|n| n == FRAME_SCENE_TEXT_MISSING);
    // The preview: said once, never as nothing at all.
    let mut h = Headless::preview(registry.clone()).await.unwrap();
    h.lose_scene_text_thread();
    let (_, first) = scene_frame(&mut h, &p, 1.5, H_SIZE).await;
    let (_, second) = scene_frame(&mut h, &p, 1.6, H_SIZE).await;
    assert!(says(&first), "{first:?}");
    assert!(!says(&second), "once: {second:?}");
    // The preview showing an earlier text in the edited one's place: said too.
    let mut h = Headless::preview(registry.clone()).await.unwrap();
    until_text_lands(&mut h, &p, 1.5).await;
    h.lose_scene_text_thread();
    let mut edited = (*p).clone();
    if let ItemContent::Generator { params, .. } = &mut scene_item(&mut edited, "lt").content {
        params.insert("title".into(), ParamValue::Text("Bo Chen".into()));
    }
    let (img, notes) = scene_frame(&mut h, &Arc::new(edited), 1.5, H_SIZE).await;
    assert!(
        ink_bounds_in(&img, lower_third_title_box_px()).is_some(),
        "the old title stands in"
    );
    assert!(says(&notes), "{notes:?}");
    // The export: the frame isn't drawn without its text; it stops, saying so.
    let stopped = klipolo_engine::test_support::export_frame_notes_without_text_thread(
        registry,
        (*p).clone(),
        secs(1.5),
        (320, 180),
    )
    .await
    .unwrap_err();
    assert!(stopped.contains(EXPORT_SCENE_TEXT_MISSING), "{stopped}");
}

/// Builds the lookahead asked ahead (`MAX_IN_FLIGHT` of them, each slow here) never keep an export frame from its own
/// text: it waits for them to land, then for its own.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_export_frame_waits_out_the_builds_asked_ahead_of_its_own() {
    let _gpu = gpu_turn().await;
    let (mut p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 3.0);
    let later = scene_item(&mut p, "lt").clone();
    for i in 0..klipolo_engine::render::scene::MAX_IN_FLIGHT {
        let mut it = later.clone();
        it.id = format!("later-{i}");
        it.start_us = secs(3.5);
        let track = overlay_track(&format!("later-{i}"), vec![it]);
        p.timeline.tracks.push(track);
    }
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    h.delay_scene_text_builds(Duration::from_millis(600));
    // The lookahead asks for the four layers starting at 3.5 s: the thread's queue is full.
    h.warm(p.clone(), secs(3.5), FormatKind::Landscape);
    assert_eq!(
        h.scene_stats().text_pending,
        klipolo_engine::render::scene::MAX_IN_FLIGHT
    );
    let (img, notes) = scene_frame(&mut h, &p, 1.5, H_SIZE).await;
    assert!(
        ink_bounds_in(&img, lower_third_title_box_px()).is_some(),
        "drawn with its text: {notes:?}"
    );
}
