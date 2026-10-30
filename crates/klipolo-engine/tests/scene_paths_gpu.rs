//! Scene paths on the GPU (lavapipe in CI): a draw-on trim, dashes along a line and round a ring (an odd
//! list doubled), a dot riding a motion path and a bar turning with it, a polyline chart from a table, a path too
//! detailed to draw left out and said once, the same path in 16:9 and 9:16, and an animated trim or motion that makes
//! no mesh and no GPU object once warm. Goldens are compared within 2/255; `UPDATE_GOLDEN=1` writes them.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_model::project::FormatKind;
use support::*;

const SIZE: (u32, u32) = (640, 360);

fn luma(p: [u8; 3]) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

async fn frame_at(json: &str, t: f64) -> (image::RgbaImage, Vec<String>) {
    let (p, registry) = project_with_scene("paths", json, 0.0, 2.0);
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &Arc::new(p), t, SIZE).await
}

/// A path line drawn on by `trim.end` (0 → 1 over 2 s) and a `line` node trimmed to half: at 1 s each shows its left
/// half, its start whole (an open path's start is its cap, not a cut).
const TRIM: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"type":"path","id":"draw","box":[0.1,0.3,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"#FFFFFF",
   "strokeWidth":12,"strokeScale":false,"animate":{"trim.end":[{"t":0,"v":0},{"t":2,"v":1,"ease":"none"}]}},
  {"type":"line","id":"rule","box":[0.1,0.6,0.8,0],"stroke":"#FFFFFF","strokeWidth":12,"trim":{"end":0.5}}]}"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn trim_draws_half_a_line_at_half_progress() {
    let _gpu = gpu_turn().await;
    let (img, warnings) = frame_at(TRIM, 1.0).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    // Both lines run from x 64 to 576 (the path at y 126, the line node at y 216): half is x 320.
    for y in [126, 216] {
        for x in [65, 70, 200, 310] {
            assert!(
                luma(rgb_at(&img, x, y)) > 200.0,
                "lit at ({x}, {y}): {:?}",
                rgb_at(&img, x, y)
            );
        }
        for x in [330, 450, 570] {
            assert!(
                luma(rgb_at(&img, x, y)) < 30.0,
                "trimmed at ({x}, {y}): {:?}",
                rgb_at(&img, x, y)
            );
        }
    }
    scene_golden("trim", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn dashes_follow_the_path() {
    let _gpu = gpu_turn().await;
    // Dashes of 60 and 30 1080p pixels: 20 on and 10 off here, from x 64. The odd list [60, 30, 15] repeats as SVG's
    // does: 20 on, 10 off, 5 on, 20 off, 10 on, 5 off. The ring's dashes follow its curve.
    const DASHES: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"path","id":"even","box":[0.1,0.2,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"#FFFFFF",
       "strokeWidth":12,"strokeScale":false,"dash":[60,30]},
      {"type":"path","id":"odd","box":[0.1,0.4,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"#FFFFFF",
       "strokeWidth":12,"strokeScale":false,"dash":[60,30,15]},
      {"type":"ellipse","id":"ring","box":[0.4,0.6,0.2,0.35],"fill":"none","stroke":"#FFB000","strokeWidth":12,
       "dash":[30,30]}]}"##;
    let (img, warnings) = frame_at(DASHES, 1.0).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    let lit = |x: u32, y: u32| luma(rgb_at(&img, 64 + x, y)) > 150.0;
    for (x, on) in [(10, true), (25, false), (40, true), (55, false), (70, true)] {
        assert_eq!(lit(x, 90), on, "the even dashes at x {}", 64 + x);
    }
    for (x, on) in [
        (10, true),
        (25, false),
        (32, true),
        (45, false),
        (60, true),
        (67, false),
        (80, true),
    ] {
        assert_eq!(lit(x, 162), on, "the odd dashes at x {}", 64 + x);
    }
    scene_golden("dashes", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_dot_rides_the_motion_path_and_turns_with_it() {
    let _gpu = gpu_turn().await;
    // A quarter circle in a 320 × 288 px box at (160, 36): its 100-unit view fits as 288 px, so its centre is at
    // (176, 36) and halfway along it (1 s of 2) lies at 45°, (379.6, 239.6), heading down and left (135°). The bar
    // turns with the path (`autoRotate`); the dot is drawn over it.
    const RIDE: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0,0,1,1],"fill":"#202830"},
      {"type":"path","id":"arc","box":[0.25,0.1,0.5,0.8],"d":"M100 0 A100 100 0 0 1 0 100","fill":"none",
       "stroke":"#556070","strokeWidth":6,"strokeScale":false},
      {"type":"rect","id":"bar","box":[0,0,0.075,0.0167],"fill":"#FFFFFF",
       "motionPath":{"path":"#arc","autoRotate":true},
       "animate":{"motionPath.progress":[{"t":0,"v":0},{"t":2,"v":1,"ease":"none"}]}},
      {"type":"ellipse","id":"dot","box":[0,0,0.03,0.0533],"fill":"#FFB000","motionPath":{"path":"#arc"},
       "animate":{"motionPath.progress":[{"t":0,"v":0},{"t":2,"v":1,"ease":"none"}]}}]}"##;
    let (img, warnings) = frame_at(RIDE, 1.0).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    let centre = rgb_at(&img, 379, 239);
    assert!(
        centre[0] > 230 && centre[1] > 150 && centre[2] < 40,
        "the dot's centre is on the path: {centre:?}"
    );
    // 16 px from the centre along 135° the bar shows; along 45° (straight out from the arc) nothing does.
    assert!(
        luma(rgb_at(&img, 368, 250)) > 200.0,
        "the bar along the path: {:?}",
        rgb_at(&img, 368, 250)
    );
    assert!(
        luma(rgb_at(&img, 390, 250)) < 60.0,
        "nothing across it: {:?}",
        rgb_at(&img, 390, 250)
    );
    scene_golden("motion", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_polyline_from_rows_rises_with_its_values() {
    let _gpu = gpu_turn().await;
    // Rows 1, 3, 2, 5 in a 512 × 144 px box at (64, 36): from the bottom left to the top right, and smoothed below.
    const CHART: &str = r##"{"scene":1,"duration":2,
      "params":[{"key":"data","label":"Data","type":"table","columns":["v"],"maxRows":8,"default":[
        {"label":"a","values":[1]},{"label":"b","values":[3]},{"label":"c","values":[2]},{"label":"d","values":[5]}]}],
      "nodes":[{"type":"polyline","id":"chart","box":[0.1,0.1,0.8,0.4],"points":"$data","stroke":"#7FD1FF",
        "strokeWidth":9,"cap":"round","join":"round"},
        {"type":"polyline","id":"smooth","box":[0.1,0.55,0.8,0.4],"points":"$data","smooth":true,
         "stroke":"#FFB000","strokeWidth":9}]}"##;
    let (img, warnings) = frame_at(CHART, 1.0).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    // The mean height of the line's ink in a column of the chart's top half.
    let ink_y = |x: u32| {
        let ys: Vec<f64> = (30..186)
            .filter(|&y| luma(rgb_at(&img, x, y)) > 60.0)
            .map(f64::from)
            .collect();
        assert!(!ys.is_empty(), "the line crosses x {x}");
        ys.iter().sum::<f64>() / ys.len() as f64
    };
    assert!(
        (ink_y(66) - 180.0).abs() < 4.0,
        "starts at the bottom: {}",
        ink_y(66)
    );
    assert!(
        (ink_y(574) - 36.0).abs() < 4.0,
        "ends at the top: {}",
        ink_y(574)
    );
    assert!(
        (ink_y(149) - 144.0).abs() < 4.0,
        "rises from 1 to 3: {}",
        ink_y(149)
    );
    assert!(
        (ink_y(320) - 126.0).abs() < 4.0,
        "falls from 3 to 2: {}",
        ink_y(320)
    );
    scene_golden("polyline", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_too_detailed_path_is_left_out_and_said_once() {
    let _gpu = gpu_turn().await;
    // 800 deep curves: past 20,000 points once flattened at their drawn size. The rect beside it still draws.
    let mut d = String::from("M0 0");
    for _ in 0..400 {
        d.push_str(" C0 1000 1000 1000 1000 0 C1000 -1000 0 -1000 0 0");
    }
    let json = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{{"type":"path","box":[0,0,0.5,1],"d":"{d}","fill":"none",
          "stroke":"#FFFFFF","strokeWidth":4,"strokeScale":false}},
          {{"type":"rect","box":[0.6,0.3,0.3,0.4],"fill":"#2D6CDF"}}]}}"##
    );
    let (p, registry) = project_with_scene("detailed", &json, 0.0, 2.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let (img, first) = scene_frame(&mut h, &p, 0.5, SIZE).await;
    let (_, second) = scene_frame(&mut h, &p, 1.5, SIZE).await;
    let code = klipolo_engine::api::FRAME_SCENE_TOO_DETAILED;
    assert_eq!(first.iter().filter(|w| *w == code).count(), 1, "{first:?}");
    assert!(!second.iter().any(|w| w == code), "said once: {second:?}");
    let white = img
        .pixels()
        .filter(|p| p.0[..3].iter().all(|&c| c > 200))
        .count();
    assert_eq!(white, 0, "the detailed path draws nothing");
    assert!(
        luma(rgb_at(&img, 480, 180)) > 60.0,
        "the rect beside it draws"
    );
}

/// A path's own units fit its box (`xMidYMid meet`), so in 16:9 and 9:16 the same scene draws the same
/// square path, the same size, in the middle of its box: the crops about each box's centre match.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_path_draws_the_same_in_16_9_and_9_16() {
    let _gpu = gpu_turn().await;
    const BADGE: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"path","box":[0.25,0.25,0.5,0.5],"d":"M50 0 L100 50 L50 100 L0 50 Z M50 25 L75 50 L50 75 L25 50 Z",
       "fillRule":"evenodd","fill":"#FFB000","stroke":"#FFFFFF","strokeWidth":4,"join":"round"}]}"##;
    let (mut p, registry) = project_with_scene("badge", BADGE, 0.0, 2.0);
    // The layer fills the frame in 9:16 too (its own rect there, as a person would place it).
    let full = bx(0.0, 0.0, 1.0, 1.0);
    scene_item(&mut p, "badge")
        .layout
        .overrides
        .insert(FormatKind::Portrait, full);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let (wide, _) = scene_frame_in(&mut h, &p, 1.0, (640, 360), FormatKind::Landscape).await;
    let (tall, _) = scene_frame_in(&mut h, &p, 1.0, (360, 640), FormatKind::Portrait).await;
    // The path fills 180 × 180 px in both: from (230, 90) in the wide frame and (90, 230) in the tall one.
    let crop = |img: &image::RgbaImage, x: u32, y: u32| {
        image::imageops::crop_imm(img, x - 4, y - 4, 188, 188).to_image()
    };
    let (a, b) = (crop(&wide, 230, 90), crop(&tall, 90, 230));
    let (within, _) = klipolo_engine::test_support::compare(&a, &b, 2);
    assert!(within >= 0.999, "{:.3} % alike", within * 100.0);
    // Its hole is even-odd's: the middle is clear, the band around it filled.
    assert!(luma(rgb_at(&wide, 320, 180)) < 30.0, "the hole");
    assert!(luma(rgb_at(&wide, 320, 110)) > 100.0, "the band");
    scene_golden("formats", &wide);
}

/// A trim and a motion animating every frame make no mesh and no GPU object once warm, and the meshes'
/// bytes stay inside their bound.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_animated_trim_or_motion_makes_no_mesh_once_warm() {
    let _gpu = gpu_turn().await;
    let (p, registry) = project_with_scene("trim", TRIM, 0.0, 2.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let times: Vec<f64> = (0..30).map(|i| 0.05 + f64::from(i) / 16.0).collect();
    scene_frame(&mut h, &p, times[0], SIZE).await;
    let made = h.scene_stats().meshes_made;
    for &t in &times {
        scene_frame(&mut h, &p, t, SIZE).await;
    }
    // The second time through is warm.
    let objects = klipolo_engine::render::effects::gpu_objects_made();
    for &t in &times {
        scene_frame(&mut h, &p, t, SIZE).await;
    }
    let stats = h.scene_stats();
    assert_eq!(stats.meshes_made, made, "no mesh made while the trim moves");
    assert_eq!(
        klipolo_engine::render::effects::gpu_objects_made(),
        objects,
        "no GPU object either"
    );
    assert!(stats.meshes >= 2 && stats.mesh_bytes <= klipolo_engine::render::scene::MAX_MESH_BYTES);
}
