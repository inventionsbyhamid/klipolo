//! The scene pass's stencils on the GPU (lavapipe in CI): six moving scene layers of six sizes keep a stencil each, and
//! the stencils, under their own bound, never push scene instances out of theirs, counted at the Mac's 1 byte a pixel
//! as well as the emulated 4. Only a pass that draws a translucent stroke has a stencil: these layers each draw one.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

const SIZE: (u32, u32) = (640, 360);

/// A rect and a translucent stroke that move all the time (so no frame is held, and every pass has a stencil).
const MOVING: &str = r##"{"scene":1,"duration":4,"nodes":[
  {"type":"rect","class":"r","box":[0.1,0.1,0.5,0.5],"fill":"#FFFFFF"},
  {"type":"path","class":"r","box":[0.1,0.1,0.5,0.5],"d":"M0 0 L10 10 L0 10 L10 0","fill":"none",
   "stroke":"rgba(255, 59, 48, 0.5)","strokeWidth":6}],
  "timeline":[{"to":".r","at":0,"dur":1,"x":"12px","yoyo":true,"repeat":-1,"ease":"sine.inOut"}]}"##;

/// Six moving scene layers of six sizes on screen at once keep a stencil each: a second pass over the same
/// frames makes no GPU object (with four kept, each frame made two again).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn six_scene_layers_of_six_sizes_keep_their_stencils() {
    let _gpu = gpu_turn().await;
    let (mut p, registry) =
        project_with_scene_in("moving", MOVING, 0.0, 4.0, bx(0.0, 0.0, 0.2, 0.2));
    let content = scene_item(&mut p, "moving").content.clone();
    for i in 1..6 {
        let at = f64::from(i) * 0.1;
        let rect = bx(at, at, 0.2 + f64::from(i) * 0.05, 0.2 + f64::from(i) * 0.03);
        let layer = item(&format!("moving{i}"), 0.0, 4.0, rect, content.clone());
        p.timeline
            .tracks
            .push(overlay_track(&format!("titles{i}"), vec![layer]));
    }
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let times: Vec<f64> = (0..10).map(|i| 0.05 + f64::from(i) / 12.0).collect();
    for &t in &times {
        scene_frame(&mut h, &p, t, SIZE).await;
    }
    let before = klipolo_engine::render::effects::gpu_objects_made();
    for &t in &times {
        scene_frame(&mut h, &p, t, SIZE).await;
    }
    assert_eq!(
        klipolo_engine::render::effects::gpu_objects_made() - before,
        0,
        "GPU objects made over 10 warm frames of six scene layers"
    );
    // Their bytes are kept in the scene budget.
    let counted = h.scene_stats().stencil_bytes;
    assert!(
        counted > 0 && klipolo_engine::render::scene::scene_bytes_peak() >= counted,
        "stencil bytes {counted}"
    );
}

/// Stencils made while this lives are counted at the bytes a pixel it was given; the platform's count again after,
/// even when a test fails.
struct Counted;

impl Counted {
    fn at(bytes: u64) -> Self {
        klipolo_engine::render::scene::count_stencil_pixels_at(Some(bytes));
        Self
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        klipolo_engine::render::scene::count_stencil_pixels_at(None);
    }
}

/// The stencils have a bound of their own and leave the scene instances' budget alone. Five
/// nearly full-frame moving layers of five sizes hold 38 MB of stencils, past the instances' whole budget. A layer
/// built on a frame while another waits, prepared, drops neither it nor any layer on screen.
///
/// Run with stencils counted at `bytes` a pixel, at a `full` frame size whose stencils hold the same
/// bytes either way, so the stencils press on every platform: 4 at 1080p where Stencil8 is emulated (DX12, Vulkan
/// without S8, lavapipe), 1 at 4K where it is native (Metal: the Mac).
async fn stencils_press_without_pushing_instances_out(bytes: u64, full: (u32, u32)) {
    let _gpu = gpu_turn().await;
    let _counted = Counted::at(bytes);
    // Each a little smaller than the one before, so each has a stencil of its own.
    let sized = |s: f64| bx(0.0, 0.0, s, s);
    let (mut p, registry) = project_with_scene_in("moving", MOVING, 0.0, 10.0, sized(1.0));
    let content = scene_item(&mut p, "moving").content.clone();
    // Four more on screen throughout, one shown at 2.5 s for 0.4 s (built on its frame), one from 3 s (prepared).
    for (id, start, duration, s) in [
        ("m1", 0.0, 10.0, 0.98),
        ("m2", 0.0, 10.0, 0.96),
        ("m3", 0.0, 10.0, 0.94),
        ("m4", 0.0, 10.0, 0.92),
        ("late", 2.5, 0.4, 0.9),
        ("prepared", 3.0, 5.0, 0.88),
    ] {
        let layer = item(id, start, duration, sized(s), content.clone());
        p.timeline
            .tracks
            .push(overlay_track(&format!("t-{id}"), vec![layer]));
    }
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &p, 1.9, full).await;
    h.warm(
        p.clone(),
        secs(3.0),
        klipolo_model::project::FormatKind::Landscape,
    );
    let before = h.scene_stats();
    for t in [2.0, 2.5, 2.7, 3.0, 3.1] {
        scene_frame(&mut h, &p, t, full).await;
    }
    let stats = h.scene_stats();
    assert!(
        stats.stencil_bytes > klipolo_engine::render::scene::MAX_SCENE_BYTES,
        "the stencils press at {bytes} bytes a pixel: {stats:?}"
    );
    assert_eq!(
        stats.draw_builds - before.draw_builds,
        1,
        "only the late layer is built on a frame at {bytes} bytes a pixel: {stats:?}"
    );
    assert_eq!(stats.builds, 7, "{bytes} bytes a pixel: {stats:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn stencils_never_push_scene_instances_out() {
    stencils_press_without_pushing_instances_out(4, (1920, 1080)).await;
}

/// The Mac's count, wherever the test runs.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn stencils_never_push_scene_instances_out_at_one_byte_a_pixel() {
    stencils_press_without_pushing_instances_out(1, (3840, 2160)).await;
}
