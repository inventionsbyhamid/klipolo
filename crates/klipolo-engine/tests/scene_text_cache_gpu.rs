//! Scene text kept and stood in for in the preview (lavapipe in CI): text on screen is never pushed out by a build that
//! lands, so five lower thirds, or two heavy texts, settle; a build that lands is kept even while no scene is drawn;
//! while a build is on its way, the stand-in is the latest of the same text, scaled evenly, never squashed, never
//! magnified from a size far off, and a setting's edit keeps the old text showing. Every test takes the GPU turn
//! (`gpu_turn`): they count layouts and texts.

mod support;
use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_engine::headless::Headless;
use klipolo_engine::registry::Registry;
use klipolo_engine::render::scene::{MAX_TEXT_ATLAS_BYTES, text_layout_calls};
use klipolo_model::layout::{Anchor, Layout};
use klipolo_model::project::Project;
use support::*;

/// `json` as `n` layers of `rect` for 30 s, each on its own track.
fn layers_of(json: &str, n: usize, rect: klipolo_model::layout::BoxN) -> (Project, Arc<Registry>) {
    let (mut p, registry) = project_with_scene_in("s", json, 0.0, 30.0, rect);
    let content = scene_item(&mut p, "s").content.clone();
    for k in 1..n {
        let id = format!("s{k}");
        let layer = item(&id, 0.0, 30.0, rect, content.clone());
        p.timeline
            .tracks
            .push(overlay_track(&format!("t{k}"), vec![layer]));
    }
    (p, registry)
}

/// The preview's frame of `p` at `t`, `gap_ms` after the one before.
async fn frame(
    h: &mut Headless,
    p: &Project,
    t: f64,
    size: (u32, u32),
    gap_ms: u64,
) -> image::RgbaImage {
    std::thread::sleep(Duration::from_millis(gap_ms));
    scene_frame(h, &Arc::new(p.clone()), t, size).await.0
}

/// Preview frames of `p` at 2 s until `done` (at most `secs`).
async fn until(
    h: &mut Headless,
    p: &Project,
    size: (u32, u32),
    secs: u64,
    done: impl Fn(&Headless) -> bool,
) {
    let started = Instant::now();
    while !done(h) {
        assert!(
            started.elapsed() < Duration::from_secs(secs),
            "{:?}",
            h.scene_stats()
        );
        frame(h, p, 2.0, size, 15).await;
    }
}

/// The width of the white title's ink, in pixels (0: none).
fn title_width(img: &image::RgbaImage) -> u32 {
    let xs = img
        .enumerate_pixels()
        .filter(|(_, _, p)| p.0[0].min(p.0[1]).min(p.0[2]) > 180)
        .map(|(x, _, _)| x);
    let (lo, hi) = xs.fold((u32::MAX, 0), |(lo, hi), x| (lo.min(x), hi.max(x)));
    if lo > hi { 0 } else { hi - lo + 1 }
}

/// Five lower thirds in one format (one more than the text cache keeps by count) all stay once built, so the
/// preview settles: no build over 80 frames, and five texts kept.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn five_lower_thirds_in_one_format_settle() {
    let _gpu = gpu_turn().await;
    let (p, registry) = layers_of(LOWER_THIRD, 5, bx(0.0, 0.0, 1.0, 1.0));
    let mut h = Headless::preview(registry).await.unwrap();
    until(&mut h, &p, H_SIZE, 30, |h| h.scene_stats().texts >= 5).await;
    for _ in 0..10 {
        frame(&mut h, &p, 2.0, H_SIZE, 15).await;
    }
    let layouts = text_layout_calls();
    for _ in 0..80 {
        frame(&mut h, &p, 2.0, H_SIZE, 15).await;
    }
    assert_eq!(
        text_layout_calls(),
        layouts,
        "no build once settled: {:?}",
        h.scene_stats()
    );
    assert_eq!(h.scene_stats().texts, 5);
}

/// Two heavy texts on screen (120 three-letter words split by word at size 200 at 1080p, about 11 MB each: words share
/// no cell, where repeated letters do) both stay, past the format's 16 MiB while both show,
/// and settle.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn two_heavy_texts_both_stay() {
    let _gpu = gpu_turn().await;
    let heavy = format!(
        r#"{{"scene":1,"duration":30,"nodes":[{{"id":"t","type":"text","text":"{}","split":"words","box":[0,0,1,1],
          "size":200}}]}}"#,
        "ABC DEF GHI JKL MNO PQR STU VWX ".repeat(15).trim_end()
    );
    let (p, registry) = layers_of(&heavy, 2, bx(0.0, 0.0, 1.0, 1.0));
    let mut h = Headless::preview(registry).await.unwrap();
    const FULL: (u32, u32) = (1920, 1080);
    until(&mut h, &p, FULL, 120, |h| h.scene_stats().texts >= 2).await;
    for _ in 0..4 {
        frame(&mut h, &p, 2.0, FULL, 60).await;
    }
    let layouts = text_layout_calls();
    for _ in 0..24 {
        frame(&mut h, &p, 2.0, FULL, 60).await;
    }
    let s = h.scene_stats();
    assert_eq!(text_layout_calls(), layouts, "no build once settled: {s:?}");
    assert_eq!(s.texts, 2);
    assert!(
        s.text_bytes > MAX_TEXT_ATLAS_BYTES,
        "both heavy: {} bytes",
        s.text_bytes
    );
}

/// A build that lands is collected at the next frame's start, though that frame draws no scene (the layer has
/// ended): it is kept, counted, never left waiting in the channel.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_landed_build_is_kept_while_no_scene_draws() {
    let _gpu = gpu_turn().await;
    let (p, registry) = layers_of(LOWER_THIRD, 1, bx(0.0, 0.0, 1.0, 1.0));
    let mut h = Headless::preview(registry).await.unwrap();
    frame(&mut h, &p, 2.0, H_SIZE, 0).await;
    let started = Instant::now();
    while h.scene_stats().texts == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the landed build is kept"
        );
        frame(&mut h, &p, 40.0, H_SIZE, 15).await;
    }
}

/// The layer drawn twice as wide shows its title at once, from the text built for the old width, scaled
/// evenly (not stretched across) until its own lands; and a title setting's edit shows the old title meanwhile.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_stand_in_keeps_its_shape_and_an_edit_keeps_the_old_title() {
    let _gpu = gpu_turn().await;
    let (p, registry) = layers_of(LOWER_THIRD, 1, bx(0.0, 0.0, 0.5, 1.0));
    let mut h = Headless::preview(registry).await.unwrap();
    until(&mut h, &p, H_SIZE, 30, |h| h.scene_stats().texts >= 1).await;
    let narrow = title_width(&frame(&mut h, &p, 2.0, H_SIZE, 0).await);
    assert!(narrow > 10, "the title shows: {narrow}");
    let mut wide = p.clone();
    scene_item(&mut wide, "s").layout = Layout::new(Anchor::TopLeft, bx(0.0, 0.0, 1.0, 1.0));
    let stand_in = title_width(&frame(&mut h, &wide, 2.0, H_SIZE, 0).await);
    assert!(
        stand_in > 10 && f64::from(stand_in) < 1.2 * f64::from(narrow),
        "an even stand-in: {narrow} px, then {stand_in} px"
    );
    let mut edited = p.clone();
    if let klipolo_model::timeline::ItemContent::Generator { params, .. } =
        &mut scene_item(&mut edited, "s").content
    {
        params.insert(
            "title".into(),
            klipolo_model::params::ParamValue::Text("Ana Leeson".into()),
        );
    }
    let while_building = title_width(&frame(&mut h, &edited, 2.0, H_SIZE, 0).await);
    assert!(
        while_building > 10,
        "the old title shows while the new one builds"
    );
}

/// Text built for a layer a tenth of the frame's size never stands in, magnified tenfold, for the full-size
/// layer: its first frame shows no title until its own text lands, then the title.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn text_built_far_smaller_never_stands_in() {
    let _gpu = gpu_turn().await;
    let (tiny, registry) = layers_of(LOWER_THIRD, 1, bx(0.0, 0.0, 0.1, 0.1));
    let mut h = Headless::preview(registry).await.unwrap();
    until(&mut h, &tiny, H_SIZE, 30, |h| h.scene_stats().texts >= 1).await;
    let mut full = tiny.clone();
    scene_item(&mut full, "s").layout = Layout::new(Anchor::TopLeft, bx(0.0, 0.0, 1.0, 1.0));
    let first = frame(&mut h, &full, 2.0, H_SIZE, 0).await;
    assert_eq!(
        ink_bounds_in(&first, lower_third_title_box_px()),
        None,
        "no magnified stand-in"
    );
    let started = Instant::now();
    while ink_bounds_in(
        &frame(&mut h, &full, 2.0, H_SIZE, 15).await,
        lower_third_title_box_px(),
    )
    .is_none()
    {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the title shows once built"
        );
    }
}

/// The lookahead never prepares a layer that starts scaled to nothing (under half a pixel): no instance built
/// for a 1 × 1 target, which would then stand in, magnified, for the first frames; it does prepare one already showing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_lookahead_skips_a_layer_scaled_to_nothing() {
    let _gpu = gpu_turn().await;
    let (mut p, registry) =
        project_with_scene_in("s", LOWER_THIRD, 1.0, 4.0, bx(0.0, 0.0, 1.0, 1.0));
    let key = |t: f64, v: f64| klipolo_model::motion::Key {
        t_us: klipolo_model::time::secs_to_us(t),
        v,
        ease: None,
    };
    scene_item(&mut p, "s").layout.motion = Some(klipolo_model::motion::Motion {
        keys: Some(klipolo_model::motion::MotionKeys {
            scale: vec![key(0.0, 0.0), key(2.0, 1.0)],
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut h = Headless::preview(registry).await.unwrap();
    // Before the layer starts: the frame's size is known.
    frame(&mut h, &p, 0.5, H_SIZE, 0).await;
    let p = Arc::new(p);
    let landscape = klipolo_model::project::FormatKind::Landscape;
    h.warm(p.clone(), klipolo_model::time::secs_to_us(1.0), landscape);
    assert_eq!(h.scene_stats().builds, 0, "nothing prepared at scale 0");
    h.warm(p, klipolo_model::time::secs_to_us(2.0), landscape);
    assert_eq!(h.scene_stats().builds, 1, "prepared once it shows");
}

/// The wait/block split. The lookahead asks for a layer's text without waiting for it (it never blocks), and an
/// export's asks with its fonts waiting for this computer's; the preview's never wait.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_lookahead_asks_an_exports_text_with_its_fonts_waiting() {
    let _gpu = gpu_turn().await;
    let landscape = klipolo_model::project::FormatKind::Landscape;
    for export in [true, false] {
        let (p, registry) =
            project_with_scene_in("s", LOWER_THIRD, 1.0, 4.0, bx(0.0, 0.0, 1.0, 1.0));
        let mut h = if export {
            Headless::new(registry).await.unwrap()
        } else {
            Headless::preview(registry).await.unwrap()
        };
        // Before the layer starts: the frame's size is known; then the lookahead for its start.
        frame(&mut h, &p, 0.5, H_SIZE, 0).await;
        h.warm(Arc::new(p), klipolo_model::time::secs_to_us(2.0), landscape);
        let s = h.scene_stats();
        let waiting = if export { 1 } else { 0 };
        assert_eq!(
            (s.text_pending, s.text_pending_waiting),
            (1, waiting),
            "asked, not waited for (export {export}): {s:?}"
        );
    }
}
