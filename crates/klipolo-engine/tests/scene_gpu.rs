//! Scene layers on the GPU (lavapipe in CI): SDF shapes with radius, stroke, gradient, shadow and clip,
//! a scene layer moving as a whole, the still hold that runs no scene pass, warm frames that make no GPU objects, the
//! runtime caps said once, and the lookahead preparing a scene before its first frame. Goldens are compared within
//! 2/255; `UPDATE_GOLDEN=1` writes them.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

const SIZE: (u32, u32) = (640, 360);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn shapes_draw_with_radius_stroke_gradient_shadow_and_clip() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("shapes", SHAPES, 0.0, 2.0);
    let mut h = Headless::new(registry).await.unwrap();
    let (img, warnings) = scene_frame(&mut h, &Arc::new(p), 1.0, SIZE).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    // Each corner its own radius (CSS order: top left 40, top right 4, bottom right 16, bottom left 28 px at 1080p, a
    // third of that here; the card spans 38.4…281.6 × 36…158.4 px). Each probe sits where its own round and the one a
    // top↔bottom or left↔right flip would put there disagree: the top left (13.3 px) leaves the
    // panel where the bottom left's 9.3 would be card, and so on round the card.
    let blue = |p: [u8; 3]| p[2] > 180 && p[0] < 100;
    for (x, y, card) in [
        (41, 39, false),
        (280, 37, true),
        (280, 157, false),
        (42, 155, true),
    ] {
        assert_eq!(
            blue(rgb_at(&img, x, y)),
            card,
            "corner at ({x}, {y}): {:?}",
            rgb_at(&img, x, y)
        );
    }
    // The card's antialiased rim lies between the card and its shadow, never lighter than both (no
    // light seam where the shadow is knocked out of the shape).
    let luma =
        |p: [u8; 3]| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]);
    for (rim, card, shadow) in [
        ((200, 158), (200, 157), (200, 159)),
        ((281, 100), (280, 100), (282, 100)),
    ] {
        let at = |(x, y): (u32, u32)| luma(rgb_at(&img, x, y));
        let most = at(card).max(at(shadow)) + 4.0;
        assert!(
            at(rim) <= most,
            "rim {rim:?}: {} against card {} and shadow {}",
            at(rim),
            at(card),
            at(shadow)
        );
    }
    scene_golden("shapes", &img);
}

/// An `image` node draws the picture its image setting names, contained in its box and turned with it, under the shape
/// painted after it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_image_node_draws_its_picture_in_its_box() {
    let _gpu = gpu_turn().await;
    const PICTURE: &str = r##"{"scene":1,"duration":2,
      "params":[{"key":"logo","label":"Logo","type":"asset","kind":"image"}],
      "nodes":[{"type":"image","asset":"$logo","box":[0.1,0.1,0.5,0.8],"rotate":10},
        {"type":"rect","box":[0.45,0.4,0.4,0.3],"fill":"rgba(255, 255, 255, 0.8)","radius":16}]}"##;
    let dir = tempfile::tempdir().unwrap();
    let (mut p, registry) = project_with_scene("picture", PICTURE, 0.0, 2.0);
    add_checker_image(&mut p, dir.path(), "logo", "picture", "logo");
    let mut h = Headless::new(registry).await.unwrap();
    let (img, warnings) = scene_frame(&mut h, &Arc::new(p), 1.0, SIZE).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    scene_golden("image", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scene_layer_moves_as_a_whole_with_layer_motion() {
    let _gpu = gpu_turn().await;
    let (mut p, registry) =
        project_with_scene_in("shapes", SHAPES, 0.0, 2.0, bx(0.25, 0.25, 0.5, 0.5));
    let mut enter = preset("slide");
    enter.params.insert(
        "from".into(),
        klipolo_model::params::ParamValue::Text("left".into()),
    );
    scene_item(&mut p, "shapes").layout.motion = Some(klipolo_model::motion::Motion {
        enter: Some(enter),
        ..Default::default()
    });
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let (early, _) = scene_frame(&mut h, &p, 0.1, SIZE).await;
    let (rest, _) = scene_frame(&mut h, &p, 1.0, SIZE).await;
    scene_golden("slide", &early);
    assert!(
        ink_x(&early) < ink_x(&rest) - 40.0,
        "at 0.1 s the shapes sit left of where they rest: {} against {}",
        ink_x(&early),
        ink_x(&rest)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_hold_reuses_the_last_target() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 12.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let mut passes = Vec::new();
    let mut frames = Vec::new();
    for s in [1.5, 2.0, 2.5] {
        let (img, _) = scene_frame(&mut h, &p, s, SIZE).await;
        passes.push(h.passes_last_frame());
        frames.push(img);
    }
    assert!(
        passes[1] < passes[0] && passes[2] == passes[1],
        "no scene pass after the first: {passes:?}"
    );
    assert_eq!(frames[0], frames[2], "the held frame is the drawn one");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_warm_scene_frame_makes_no_gpu_objects() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 12.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    // Through the entrance (every frame moves) and into the hold, twice: the second time is warm.
    let times: Vec<f64> = (0..30).map(|i| 0.05 + f64::from(i) / 24.0).collect();
    for &s in &times {
        scene_frame(&mut h, &p, s, SIZE).await;
    }
    let before = klipolo_engine::render::effects::gpu_objects_made();
    for &s in &times {
        scene_frame(&mut h, &p, s, SIZE).await;
    }
    assert_eq!(
        klipolo_engine::render::effects::gpu_objects_made(),
        before,
        "30 warm frames make no GPU objects"
    );
}

/// The scene runtime's caps: 1,500 repeated rects (1,000 drawn), a burst of
/// 100,000 particles (none past 500 kept) and eight chained tweens over every rect (past the track cap, so the scene's
/// still). It draws within its budget, and each degrade is said once.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scene_past_every_cap_it_owns_draws_within_budget_and_says_so_once() {
    let _gpu = gpu_turn().await;
    let json = past_every_cap();
    let registry = unchecked_scene_registry(&[("caps", &json)]);
    let (p, _) = project_with_scene("caps", SHAPES, 0.0, 4.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    klipolo_engine::render::scene::reset_scene_bytes_peak();
    let started = std::time::Instant::now();
    let (_, first) = scene_frame(&mut h, &p, 0.5, SIZE).await;
    let took = started.elapsed();
    let (_, second) = scene_frame(&mut h, &p, 1.5, SIZE).await;
    for code in [
        klipolo_engine::api::FRAME_SCENE_REDUCED,
        klipolo_engine::api::FRAME_SCENE_STILL,
    ] {
        assert_eq!(
            first.iter().filter(|w| *w == code).count(),
            1,
            "{code}: {first:?}"
        );
        assert!(!second.iter().any(|w| w == code), "said once: {second:?}");
    }
    // The bound counts the instance's CPU side too (its nodes, frame, packed shapes and their uploaded
    // copy), not only its GPU buffers (363,632 bytes for these 1,001 nodes).
    let bytes = klipolo_engine::render::scene::scene_bytes_peak();
    assert!(bytes > 1 << 20 && bytes < 8 << 20, "{bytes} bytes");
    // The drawer's own work for the first frame (the build included) stays well inside a frame's budget
    // even in a debug build on a software GPU.
    let cpu_ms = h.scene_stats().cpu_us_last / 1000.0;
    assert!(cpu_ms < 500.0, "{cpu_ms} ms");
    println!("past every cap: first frame {took:?}, {bytes} scene bytes");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scene_layer_about_to_appear_is_prepared_in_the_lookahead() {
    let _gpu = gpu_turn().await;
    // The layer starts at 3 s; the lookahead warms its start a second ahead, then 60 frames play before it shows (30 at
    // 30 fps would do; the prepared instance must outlive them).
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 3.0, 5.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &p, 1.9, SIZE).await;
    h.warm(
        p.clone(),
        secs(3.0),
        klipolo_model::project::FormatKind::Landscape,
    );
    let stats = h.scene_stats();
    assert_eq!(stats.builds, 1, "prepared ahead: {stats:?}");
    for i in 0..60 {
        scene_frame(&mut h, &p, 2.0 + f64::from(i) / 60.0, SIZE).await;
    }
    scene_frame(&mut h, &p, 3.0, SIZE).await;
    let stats = h.scene_stats();
    assert_eq!(
        stats.draw_builds, 0,
        "nothing built on the render thread: {stats:?}"
    );
    assert_eq!(stats.builds, 1, "{stats:?}");
}

/// A scene the lookahead prepared but that never shows (playback stopped or sought away before it) is
/// let go after a bounded wait, not kept until the project closes.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scene_prepared_but_never_shown_is_let_go() {
    let _gpu = gpu_turn().await;
    let (mut p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 30.0);
    // A second scene layer far ahead (60 s): the lookahead warms its start, and it never shows here.
    let mut later = scene_item(&mut p, "lt").clone();
    later.id = "later".into();
    later.start_us = secs(60.0);
    p.timeline.tracks.push(overlay_track("later", vec![later]));
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let small = (160, 90);
    scene_frame(&mut h, &p, 1.5, small).await;
    h.warm(
        p.clone(),
        secs(60.0),
        klipolo_model::project::FormatKind::Landscape,
    );
    assert_eq!(
        h.scene_stats().cached,
        2,
        "prepared ahead: {:?}",
        h.scene_stats()
    );
    for i in 0..600 {
        scene_frame(&mut h, &p, 1.5 + f64::from(i) / 60.0, small).await;
    }
    assert_eq!(
        h.scene_stats().cached,
        1,
        "only the scene on screen is kept: {:?}",
        h.scene_stats()
    );
}

/// A layer drawn at another size (a gizmo resize, a window or format change) keeps its instance; only the
/// target's size is written again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_resize_keeps_the_scene_instance() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 12.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    for size in [(640, 360), (800, 450), (1024, 576), (640, 360)] {
        scene_frame(&mut h, &p, 0.5, size).await;
    }
    let stats = h.scene_stats();
    assert_eq!(stats.builds, 1, "one instance across sizes: {stats:?}");
}

/// An export draws a scene past its caps as the preview does, reduced, and reports it with the
/// frame's time instead of stopping.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_export_draws_a_reduced_scene_and_reports_it() {
    let _gpu = gpu_turn().await;
    let registry = unchecked_scene_registry(&[("caps", &past_every_cap())]);
    let (p, _) = project_with_scene("caps", SHAPES, 0.0, 4.0);
    let notes =
        klipolo_engine::test_support::export_frame_notes(registry, p, secs(0.5), (320, 180))
            .await
            .expect("the frame is drawn, not refused");
    let codes: Vec<&str> = notes.iter().map(|(code, _)| code.as_str()).collect();
    assert_eq!(
        codes,
        [
            klipolo_engine::api::EXPORT_SCENE_REDUCED,
            klipolo_engine::api::EXPORT_SCENE_STILL
        ],
        "{notes:?}"
    );
    assert!(notes.iter().all(|(_, at)| *at == secs(0.5)), "{notes:?}");
}

/// Performance numbers: a typical title and a 1,000-dot grid, each frame
/// moving, at 1280 × 720 on this GPU: the scene drawer's own CPU time and the whole frame's (with its read back).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn scene_frame_costs() {
    let _gpu = gpu_turn().await;
    for (name, json, from) in [("title", LOWER_THIRD, 0.05), ("grid", DOT_GRID, 0.5)] {
        let (p, registry) = project_with_scene(name, json, 0.0, 8.0);
        let p = Arc::new(p);
        let mut h = Headless::new(registry).await.unwrap();
        let times: Vec<f64> = (0..20).map(|i| from + f64::from(i) / 60.0).collect();
        for &s in &times {
            scene_frame(&mut h, &p, s, (1280, 720)).await;
        }
        let (mut cpu, started) = (0.0, std::time::Instant::now());
        for &s in &times {
            scene_frame(&mut h, &p, s, (1280, 720)).await;
            cpu += h.scene_stats().cpu_us_last;
        }
        let frame_ms = started.elapsed().as_secs_f64() * 1000.0 / times.len() as f64;
        let n = times.len() as f64;
        println!(
            "{name}: scene drawer {:.0} µs a frame on the CPU, whole frame {frame_ms:.1} ms (with read back)",
            cpu / n
        );
        assert!(cpu > 0.0);
    }
}

/// 1,500 repeated rects (1,000 drawn), a burst of 100,000 particles (none past 500 kept) and eight chained tweens over
/// every rect (past the track cap, so the scene's still).
fn past_every_cap() -> String {
    let tweens: Vec<String> = (0..8)
        .map(|k| {
            format!(
                r#"{{"to":".c","at":0,"dur":1,"x":{}}}"#,
                0.05 * f64::from(k + 1)
            )
        })
        .collect();
    format!(
        r##"{{"scene":1,"duration":4,"nodes":[
          {{"type":"repeat","times":1500,"gap":[0.0005,0.0005],"children":[{{"type":"rect","class":"c","box":[0,0,0.02,0.02],"fill":"#FFFFFF"}}]}},
          {{"type":"emitter","box":[0,0,1,1],"burst":100000,"life":3,"child":{{"type":"ellipse","box":[0,0,0.01,0.01],"fill":"#FF0000"}}}}],
          "timeline":[{}]}}"##,
        tweens.join(",")
    )
}
