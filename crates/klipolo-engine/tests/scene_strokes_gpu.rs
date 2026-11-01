//! Scene strokes on the GPU (lavapipe in CI): a translucent stroke blends once where its own joins and crossings
//! overlap; trim and dash cuts take the line's cap, so a round-capped zero-length dash is a dot and a draw-on ends
//! without a jump; a scaling layer motion makes no mesh per frame; a fill lyon can't make within the cap is said, not
//! dropped; a dash gap narrower than the caps is covered once (the stencils' own tests are in `scene_stencils_gpu.rs`).
//! Goldens are compared within 2/255; `UPDATE_GOLDEN=1` writes them.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

const SIZE: (u32, u32) = (640, 360);

fn luma(p: [u8; 3]) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

async fn frame_at(json: &str, t: f64) -> (image::RgbaImage, Vec<String>) {
    let (p, registry) = project_with_scene("strokes", json, 0.0, 2.0);
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &Arc::new(p), t, SIZE).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_translucent_stroke_blends_once_where_it_overlaps() {
    let _gpu = gpu_turn().await;
    // 20 px half-white strokes: a V with a 61° corner at (192, 288), and a bow tie crossing itself at (496, 180).
    const OVERLAPS: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"polyline","points":[[0.1,0.2],[0.3,0.8],[0.5,0.2]],"stroke":"rgba(255, 255, 255, 0.5)","strokeWidth":60},
      {"type":"polyline","points":[[0.6,0.2],[0.95,0.8],[0.95,0.2],[0.6,0.8]],"stroke":"rgba(255, 255, 255, 0.5)",
       "strokeWidth":60}]}"##;
    let (img, warnings) = frame_at(OVERLAPS, 1.0).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    let at = |x, y| luma(rgb_at(&img, x, y));
    let straight = at(128, 180);
    assert!(
        (straight - 127.5).abs() <= 3.0,
        "a single coverage: {straight}"
    );
    for (x, y, what) in [
        (192, 280, "inside the corner"),
        (496, 180, "the crossing"),
        (440, 126, "the bow tie's arm"),
    ] {
        assert!(
            (at(x, y) - straight).abs() <= 2.0,
            "{what} at ({x}, {y}): {} against {straight}",
            at(x, y)
        );
    }
    scene_golden("overlaps", &img);
}

/// Round caps at dash ends make `[0, 30]` dots (radius 3 px every 10 px), a trim's cut round past it, and square
/// caps lengthen each dash by its half width.
const CAPS: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"type":"path","box":[0.1,0.2,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"#FFFFFF","strokeWidth":18,
   "strokeScale":false,"cap":"round","dash":[0,30]},
  {"type":"path","box":[0.1,0.4,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"#FFFFFF","strokeWidth":18,
   "strokeScale":false,"cap":"round","trim":{"end":0.5}},
  {"type":"path","box":[0.1,0.6,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"#FFFFFF","strokeWidth":18,
   "strokeScale":false,"cap":"square","dash":[60,60]},
  {"type":"ellipse","box":[0.4,0.75,0.2,0.22],"fill":"none","stroke":"#FFB000","strokeWidth":18,"cap":"round",
   "dash":[0,45]}]}"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn trim_and_dash_cuts_take_the_lines_cap() {
    let _gpu = gpu_turn().await;
    let (img, warnings) = frame_at(CAPS, 1.0).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    let lit = |x: u32, y: u32| luma(rgb_at(&img, x, y)) > 150.0;
    // Dots at x 64, 74, 84… on y 90, dark between.
    for i in 1..6 {
        assert!(lit(64 + 10 * i, 90), "a dot at x {}", 64 + 10 * i);
        assert!(!lit(69 + 10 * i, 90), "a gap at x {}", 69 + 10 * i);
    }
    // The trim cuts at x 320 on y 162: half its width past it the round end still shows, past that nothing.
    assert!(lit(321, 162), "the round end past the cut");
    assert!(!lit(325, 162), "nothing beyond it");
    // Square-capped 20 px dashes from x 64 on y 234 reach 3 px further: x 86 shows, x 89 doesn't.
    assert!(lit(86, 234) && !lit(89, 234), "square ends");
    scene_golden("caps", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_round_capped_draw_on_ends_without_a_jump() {
    let _gpu = gpu_turn().await;
    // Drawn on over 2 s: at 1.998 s (0.999) its tip is already the round end the finished line has at 2 s.
    const DRAW_ON: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"path","box":[0.1,0.4,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"#FFFFFF","strokeWidth":18,
       "strokeScale":false,"cap":"round","animate":{"trim.end":[{"t":0,"v":0},{"t":2,"v":1,"ease":"none"}]}}]}"##;
    let (p, registry) = project_with_scene("drawOn", DRAW_ON, 0.0, 3.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let (almost, _) = scene_frame(&mut h, &p, 1.998, SIZE).await;
    let (done, _) = scene_frame(&mut h, &p, 2.0, SIZE).await;
    assert!(
        luma(rgb_at(&almost, 577, 162)) > 150.0,
        "the tip's round end past x 576"
    );
    // The two frames differ only at the tip (its last half pixel of travel): nowhere left of x 572.
    for (x, y, a) in almost.enumerate_pixels() {
        let b = done.get_pixel(x, y);
        let diff = (luma([a[0], a[1], a[2]]) - luma([b[0], b[1], b[2]])).abs();
        if diff > 8.0 && x < 572 {
            let row = |img: &image::RgbaImage| {
                (x.saturating_sub(4)..x + 5)
                    .map(|xx| format!("{:.0}", luma(rgb_at(img, xx, y))))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            panic!(
                "({x}, {y}) changes by {diff} in the last frame: [{}] then [{}]",
                row(&almost),
                row(&done)
            );
        }
    }
}

/// A scene layer that pops in and pulses (its layer motion scales every frame) keeps its meshes: a handful made as
/// the scale crosses its buckets, next to none on a second pass, and its instance built once.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scaling_layer_motion_makes_no_mesh_per_frame() {
    let _gpu = gpu_turn().await;
    const SHAPES: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0.1,0.1,0.3,0.3],"fill":"none","stroke":"#FFFFFF","strokeWidth":6,"radius":24,
       "trim":{"end":0.7}},
      {"type":"path","box":[0.5,0.1,0.4,0.3],"d":"M0 50 C30 0 70 100 100 50","fill":"none","stroke":"#FFB000",
       "strokeWidth":4,"cap":"round"},
      {"type":"polyline","box":[0.1,0.6,0.8,0.3],"points":[[0,1],[0.3,0.2],[0.6,0.7],[1,0]],"smooth":true,
       "stroke":"#7FD1FF","strokeWidth":6}]}"##;
    let (mut p, registry) =
        project_with_scene_in("pulse", SHAPES, 0.0, 4.0, bx(0.2, 0.2, 0.6, 0.6));
    scene_item(&mut p, "pulse").layout.motion = Some(klipolo_model::motion::Motion {
        enter: Some(preset("pop")),
        looping: Some(preset("pulse")),
        ..Default::default()
    });
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let times: Vec<f64> = (0..30).map(|i| 0.02 + f64::from(i) / 15.0).collect();
    scene_frame(&mut h, &p, times[0], SIZE).await;
    let first = h.scene_stats().meshes_made;
    for &t in &times {
        scene_frame(&mut h, &p, t, SIZE).await;
    }
    let after_one = h.scene_stats().meshes_made;
    assert!(
        after_one - first <= 8,
        "meshes made over 30 scaling frames: {}",
        after_one - first
    );
    for &t in &times {
        scene_frame(&mut h, &p, t, SIZE).await;
    }
    let stats = h.scene_stats();
    assert!(
        stats.meshes_made - after_one <= 2,
        "meshes made on the second pass: {}",
        stats.meshes_made - after_one
    );
    assert_eq!(stats.builds, 1, "the instance is built once");
}

/// A fill whose crossings take it past 20,000 vertices once lyon has made it is left out and said, not dropped
/// silently while its stroke draws.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_fill_too_detailed_for_lyon_is_said() {
    let _gpu = gpu_turn().await;
    // A star polygon of 301 points, each joined to the one 150 on: about 45,000 crossings.
    let mut d = String::new();
    for j in 0..=301usize {
        let a = (j * 150 % 301) as f64 * std::f64::consts::TAU / 301.0;
        let (x, y) = (50.0 + 50.0 * a.cos(), 50.0 + 50.0 * a.sin());
        d.push_str(&format!("{}{x:.3} {y:.3} ", if j == 0 { "M" } else { "L" }));
    }
    let json = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{{"type":"path","box":[0.2,0.1,0.6,0.8],"d":"{d}Z","fill":"#FFFFFF",
          "stroke":"#FF0000","strokeWidth":1}}]}}"##
    );
    let (_, warnings) = frame_at(&json, 1.0).await;
    let code = klipolo_engine::api::FRAME_SCENE_TOO_DETAILED;
    assert_eq!(
        warnings.iter().filter(|w| *w == code).count(),
        1,
        "{warnings:?}"
    );
}

/// A dash gap narrower than the caps is covered once. Half-white 16 px lines of 4 px dashes and 2 px gaps under
/// round and square caps read one coverage all along (each pixel takes the nearer cut's cap, one fragment a pixel), as
/// SVG's union of the dashes' outlines does: no gap blended twice, no seam.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_narrow_dash_gap_under_wide_caps_is_covered_once() {
    let _gpu = gpu_turn().await;
    const GAPS: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"path","box":[0.1,0.4,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"rgba(255, 255, 255, 0.5)",
       "strokeWidth":48,"strokeScale":false,"cap":"round","dash":[12,6]},
      {"type":"path","box":[0.1,0.6,0.8,0.1],"d":"M0 0 L100 0","fill":"none","stroke":"rgba(255, 255, 255, 0.5)",
       "strokeWidth":48,"strokeScale":false,"cap":"square","dash":[12,6]}]}"##;
    let (img, warnings) = frame_at(GAPS, 1.0).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    for (centre, cap) in [(162, "round"), (234, "square")] {
        for y in centre - 4..=centre + 4 {
            for x in 100..540 {
                let v = luma(rgb_at(&img, x, y));
                assert!(
                    (v - 127.5).abs() <= 3.0,
                    "{cap} caps at ({x}, {y}): {v}, not one coverage"
                );
            }
        }
    }
}
