//! Scene clips on the GPU (lavapipe in CI): an image is clipped by its own clip and by every clipped node above it (a
//! rounded card, a circle avatar, a wipe); nested clips intersect; a clip closed to nothing draws nothing, not a
//! hairline; a shadow stays outside its box; a stroke's gradient turns by its own angle. Goldens are compared within
//! 2/255; `UPDATE_GOLDEN=1` writes them.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

const SIZE: (u32, u32) = (640, 360);
/// The light panel the scenes draw on (`#D8DCE3`).
const PANEL: [u8; 3] = [0xD8, 0xDC, 0xE3];

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 3)
}

async fn frame_of(
    json: &str,
    setup: impl FnOnce(&mut klipolo_model::project::Project),
) -> image::RgbaImage {
    let (mut p, registry) = project_with_scene("clip", json, 0.0, 2.0);
    setup(&mut p);
    let mut h = Headless::new(registry).await.unwrap();
    let (img, warnings) = scene_frame(&mut h, &Arc::new(p), 1.0, SIZE).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    img
}

/// A photo in a card with rounded corners stays inside the card; an avatar shows only its circle; a wipe shows
/// only its open part.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn images_are_clipped_by_their_own_clip_and_their_cards() {
    let _gpu = gpu_turn().await;
    const CLIPPED: &str = r##"{"scene":1,"duration":2,
      "params":[{"key":"photo","label":"Photo","type":"asset","kind":"image"}],
      "nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#D8DCE3"},
        {"type":"group","id":"card","box":[0.04,0.1,0.28,0.8],"clip":"inset(0 round 40px)","children":[
          {"type":"image","asset":"$photo","box":[-1.5,-0.5,4,2]}]},
        {"type":"image","id":"avatar","asset":"$photo","box":[0.38,0.2,0.2,0.3556],"clip":"circle(40%)"},
        {"type":"image","id":"wipe","asset":"$photo","box":[0.68,0.2,0.28,0.6],"clip":"inset(0 50% 0 0)"}]}"##;
    let dir = tempfile::tempdir().unwrap();
    let img = frame_of(CLIPPED, |p| {
        add_checker_image(p, dir.path(), "photo", "clip", "photo")
    })
    .await;
    // The card's photo is bigger than the card (717 × 358 px about the card's centre): nothing of it outside the card
    // (26…205 × 36…324 px), its corners rounded (13.3 px) too.
    for (x, y) in [(10, 180), (250, 180), (27, 37), (203, 322)] {
        assert!(
            near(rgb_at(&img, x, y), PANEL),
            "card outside at ({x}, {y}): {:?}",
            rgb_at(&img, x, y)
        );
    }
    for (x, y) in [(110, 180), (40, 50), (190, 310)] {
        assert!(
            !near(rgb_at(&img, x, y), PANEL),
            "the card shows its photo at ({x}, {y})"
        );
    }
    // The avatar's photo spans 243…371 × 104…168 px; its circle (r 51 px about 307, 136) cuts its corners.
    assert!(
        near(rgb_at(&img, 247, 108), PANEL),
        "avatar corner: {:?}",
        rgb_at(&img, 247, 108)
    );
    assert!(
        !near(rgb_at(&img, 307, 136), PANEL),
        "the avatar shows its middle"
    );
    // The wipe shows the left half of its photo (435…614 px) only.
    assert!(
        near(rgb_at(&img, 600, 180), PANEL),
        "wiped half: {:?}",
        rgb_at(&img, 600, 180)
    );
    assert!(!near(rgb_at(&img, 450, 180), PANEL), "the open half shows");
    scene_golden("image_clips", &img);
}

/// A strip wiped half open (its group's clip) around a dot with a clip of its own: the dot shows only where both
/// are open.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn nested_clips_intersect() {
    let _gpu = gpu_turn().await;
    const NESTED: &str = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#D8DCE3"},
      {"type":"group","id":"strip","box":[0.1,0.2,0.8,0.6],"clip":"inset(0 50% 0 0)","children":[
        {"type":"rect","box":[0,0,1,1],"fill":"#2D6CDF"},
        {"type":"ellipse","id":"dot","box":[0.3,0.1,0.4,0.8],"fill":"#FF8800","clip":"circle(50%)"}]}]}"##;
    let img = frame_of(NESTED, |_| {}).await;
    // The strip is open up to x = 320 px; the dot spans 218…422 px about (320, 180).
    assert!(
        near(rgb_at(&img, 380, 180), PANEL),
        "past the strip's wipe: {:?}",
        rgb_at(&img, 380, 180)
    );
    assert!(
        near(rgb_at(&img, 300, 180), [0xFF, 0x88, 0x00]),
        "the dot inside both: {:?}",
        rgb_at(&img, 300, 180)
    );
    scene_golden("nested_clips", &img);
}

/// A clip closed to nothing (`inset` with its edges met, `circle(0%)`) draws nothing at all: no hairline, no dot.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_empty_clip_draws_nothing() {
    let _gpu = gpu_turn().await;
    const CLOSED: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0.1,0.1,0.3,0.8],"fill":"#FFFFFF","clip":"inset(0 50% 0 50%)"},
      {"type":"rect","box":[0.45,0.1,0.1,0.8],"fill":"#FFFFFF","clip":"inset(0 100% 0 0)"},
      {"type":"ellipse","box":[0.6,0.1,0.3,0.6],"fill":"#FFFFFF","clip":"circle(0% at 50% 50%)"}]}"##;
    let img = frame_of(CLOSED, |_| {}).await;
    let lit = img
        .pixels()
        .filter(|p| p.0[..3].iter().any(|&c| c > 0))
        .count();
    assert_eq!(lit, 0, "pixels drawn through a closed clip");
    // The lower third before its wipe starts: its card is clipped shut, so the frame is the black background.
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 12.0);
    let mut h = Headless::new(registry).await.unwrap();
    let (img, _) = scene_frame(&mut h, &Arc::new(p), 0.0, SIZE).await;
    let lit = img
        .pixels()
        .filter(|p| p.0[..3].iter().any(|&c| c > 0))
        .count();
    assert_eq!(lit, 0, "the lower third at 0 s draws nothing");
}

/// A translucent card's shadow stays outside it, as CSS's box-shadow does: the card shows what is under it, not
/// its own shadow.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_shadow_does_not_show_through_a_translucent_fill() {
    let _gpu = gpu_turn().await;
    const GLASS: &str = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#FFFFFF"},
      {"type":"rect","id":"glass","box":[0.25,0.25,0.5,0.5],"fill":"rgba(255, 255, 255, 0.3)","radius":12,
       "shadow":{"x":0,"y":0,"blur":40,"spread":20,"color":"rgba(0, 0, 0, 0.8)"}}]}"##;
    let img = frame_of(GLASS, |_| {}).await;
    let inside = rgb_at(&img, 320, 180);
    assert!(
        inside.iter().all(|&c| c >= 250),
        "the card's middle is the white under it: {inside:?}"
    );
    let outside = rgb_at(&img, 156, 180);
    assert!(
        outside.iter().all(|&c| c < 200),
        "the shadow shows outside it: {outside:?}"
    );
}

fn luma(p: [u8; 3]) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

/// A shape that isn't wholly opaque shows no dark band of its own shadow just inside its edge: a
/// half-transparent card, an opaque card faded to half inside the scene, and an unfilled thin outline. Each pixel half a
/// pixel inside the edge is within a few levels of the pixels further in, and the shadow still shows outside.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_shape_that_is_not_opaque_shows_no_dark_rim_of_its_shadow() {
    let _gpu = gpu_turn().await;
    // Edges on whole pixels at 640 × 360: glass 32…288, fade 352…608 (both 90…270), outline 192…448 × 290…340 px.
    const RIMS: &str = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#E6E6E6"},
      {"type":"rect","id":"glass","box":[0.05,0.25,0.4,0.5],"fill":"rgba(255, 255, 255, 0.5)",
       "shadow":{"x":0,"y":0,"blur":40,"spread":20,"color":"rgba(0, 0, 0, 0.8)"}},
      {"type":"rect","id":"fade","box":[0.55,0.25,0.4,0.5],"fill":"#FFFFFF","opacity":0.5,
       "shadow":{"x":12,"y":12,"blur":20,"color":"rgba(0, 0, 0, 0.6)"}},
      {"type":"rect","id":"outline","box":[0.3,0.8056,0.4,0.1389],"fill":"rgba(0, 0, 0, 0)","stroke":"#FFFFFF",
       "strokeWidth":3,"shadow":{"x":0,"y":0,"blur":30,"spread":10,"color":"rgba(0, 0, 0, 0.6)"}}]}"##;
    let img = frame_of(RIMS, |_| {}).await;
    let at = |x: u32, y: u32| luma(rgb_at(&img, x, y));
    let mut wrong = Vec::new();
    // Half a pixel inside each edge against further in: the glass's left edge (the panel through it, 242.5), the faded
    // card's shadowed right edge, and the outline's inner half (its 1 px stroke straddles x = 192), not darker.
    for (name, rim, inside, most) in [
        ("glass", (32, 180), (40, 180), 8.0),
        ("fade", (607, 180), (603, 180), 6.0),
        ("outline", (192, 315), (196, 315), 6.0),
    ] {
        let (r, i) = (at(rim.0, rim.1), at(inside.0, inside.1));
        let dark = if name == "outline" {
            i - r
        } else {
            (r - i).abs()
        };
        if dark > most {
            wrong.push(format!("{name} rim {r:.1} against inside {i:.1}"));
        }
    }
    for (x, y, name) in [
        (24, 180, "glass"),
        (610, 200, "fade"),
        (186, 315, "outline"),
    ] {
        if at(x, y) >= 230.0 - 20.0 {
            wrong.push(format!(
                "{name}'s shadow doesn't show outside it: {}",
                at(x, y)
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A stroke's gradient turns by its own angle (here 90°, so it runs down the box, not across), whatever the
/// fill's.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_stroke_gradient_turns_by_its_own_angle() {
    let _gpu = gpu_turn().await;
    const STROKED: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0.2,0.1,0.6,0.8],"fill":"#202020","strokeWidth":40,
       "stroke":{"linear":[0,0,1,0],"stops":[[0,"#FF0000"],[1,"#0000FF"]],"angle":90}}]}"##;
    let img = frame_of(STROKED, |_| {}).await;
    // The stroke's middle on the left (x 128 px) and on the right (x 512 px) at the same height: the same colour.
    let (left, right) = (rgb_at(&img, 130, 180), rgb_at(&img, 509, 180));
    assert!(
        left.iter().zip(right).all(|(a, b)| a.abs_diff(b) <= 24),
        "across the box the gradient is the same: {left:?} against {right:?}"
    );
    let (top, bottom) = (rgb_at(&img, 320, 38), rgb_at(&img, 320, 322));
    assert!(
        top[0].abs_diff(bottom[0]) > 120,
        "down the box it runs: {top:?} against {bottom:?}"
    );
}
