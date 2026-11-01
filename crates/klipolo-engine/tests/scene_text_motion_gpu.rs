//! Scene text under layer motion and off the render thread (lavapipe in CI): a scaling layer lays its text out once and
//! the title scales with its card; a layer scaled from nothing or far up stays bounded; a wrapped title split by lines
//! fades in line by line, its units taken by the instance that draws it; text is built on its own thread, the preview
//! drawing without it until it lands and an export waiting. Every test takes the GPU turn (`gpu_turn`): they count GPU
//! objects, layouts and builds, which the others would make.

mod support;
use support::*;

/// The lower third as a layer of `seconds` with `motion`, full frame.
fn lower_third_moving(
    motion: klipolo_model::motion::Motion,
    seconds: f64,
) -> (
    std::sync::Arc<klipolo_model::project::Project>,
    std::sync::Arc<klipolo_engine::registry::Registry>,
) {
    let (mut p, registry) =
        project_with_scene_in("lt", LOWER_THIRD, 0.0, seconds, bx(0.0, 0.0, 1.0, 1.0));
    scene_item(&mut p, "lt").layout.motion = Some(motion);
    (std::sync::Arc::new(p), registry)
}

/// The width of the pixels of `img` that `ink` picks, if any.
fn ink_width(img: &image::RgbaImage, ink: impl Fn([u8; 4]) -> bool) -> u32 {
    let xs = img
        .enumerate_pixels()
        .filter(|(_, _, p)| ink(p.0))
        .map(|(x, _, _)| x);
    let (lo, hi) = xs.fold((u32::MAX, 0), |(lo, hi), x| (lo.min(x), hi.max(x)));
    if lo > hi { 0 } else { hi - lo + 1 }
}

/// A scene layer whose layer motion scales (a zoom in over 6 s, then a pulse) lays its text out once,
/// for the largest scale its motion reaches: a second pass over the same frames lays out nothing and makes no GPU
/// object, and the title scales with its card: the white title's width keeps its share of the orange tab and rule's
/// while the card grows by more than a tenth.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scaling_layer_lays_its_text_out_once() {
    let _gpu = gpu_turn().await;
    let mut zoom = preset("zoom");
    zoom.duration_us = klipolo_model::time::secs_to_us(6.0);
    let motion = klipolo_model::motion::Motion {
        enter: Some(zoom),
        looping: Some(preset("pulse")),
        ..Default::default()
    };
    let (p, registry) = lower_third_moving(motion, 12.0);
    let mut h = klipolo_engine::headless::Headless::new(registry)
        .await
        .unwrap();
    const SIZE: (u32, u32) = (1280, 720);
    let times: Vec<f64> = (0..30).map(|i| 0.05 + f64::from(i) / 5.0).collect();
    for &t in &times {
        scene_frame(&mut h, &p, t, SIZE).await;
    }
    let layouts = klipolo_engine::render::scene::text_layout_calls();
    let mut widths = Vec::new();
    let (mut objects, mut before) = (0, h.scene_stats());
    for (i, &t) in times.iter().enumerate() {
        let (img, _) = scene_frame(&mut h, &p, t, SIZE).await;
        if t > 1.0 {
            // At rest in the scene (its lines are in by 0.94 s): the title white, the tab and rule orange.
            let title = ink_width(&img, |p| p[0].min(p[1]).min(p[2]) > 180);
            let card = ink_width(&img, |p| p[0] > 180 && p[2] < 90);
            widths.push((t, f64::from(title), f64::from(card)));
        }
        if i == 0 {
            // Counted from the second frame: the first redraws after the last pass's 24 held frames, and the effect
            // stage let that target go after 8 unused ones (a still layer makes the same two).
            (objects, before) = (
                klipolo_engine::render::effects::gpu_objects_made(),
                h.scene_stats(),
            );
        }
    }
    assert_eq!(
        klipolo_engine::render::scene::text_layout_calls(),
        layouts,
        "no layout per frame"
    );
    assert_eq!(
        klipolo_engine::render::effects::gpu_objects_made(),
        objects,
        "no GPU objects per frame: {before:?} then {:?}",
        h.scene_stats()
    );
    let spread = |v: &dyn Fn(&(f64, f64, f64)) -> f64| {
        let (lo, hi) = widths
            .iter()
            .map(v)
            .fold((f64::MAX, 0.0f64), |(lo, hi), x| (lo.min(x), hi.max(x)));
        hi / lo.max(1.0)
    };
    assert!(
        spread(&|w| w.2) > 1.1,
        "the card grows by more than a tenth: {widths:?}"
    );
    assert!(
        spread(&|w| w.1 / w.2.max(1.0)) < 1.05,
        "the title keeps its share of the card: {widths:?}"
    );
}

/// A layer keyed from scale 10⁻⁶ up to 1, and one keyed from 1 up to 10⁶: no frame lays its text out
/// past 2,048 px an em or holds an atlas past its budget, and none panics; while it is under half a pixel, nothing is
/// built.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_layer_scaled_from_nothing_or_far_up_is_bounded() {
    let _gpu = gpu_turn().await;
    let key = |t: f64, v: f64| klipolo_model::motion::Key {
        t_us: klipolo_model::time::secs_to_us(t),
        v,
        ease: None,
    };
    for scale in [
        vec![key(0.0, 1e-6), key(2.0, 1.0)],
        vec![key(0.0, 1.0), key(2.0, 1e6)],
    ] {
        let from_nothing = scale[0].v < 1.0;
        let motion = klipolo_model::motion::Motion {
            keys: Some(klipolo_model::motion::MotionKeys {
                scale,
                ..Default::default()
            }),
            ..Default::default()
        };
        let (p, registry) = lower_third_moving(motion, 4.0);
        let mut h = klipolo_engine::headless::Headless::new(registry)
            .await
            .unwrap();
        let started = std::time::Instant::now();
        for t in [0.0, 1e-4, 0.01, 0.5, 1.5, 2.0, 3.0] {
            scene_frame(&mut h, &p, t, H_SIZE).await;
            let s = h.scene_stats();
            assert!(
                s.text_bytes <= klipolo_engine::render::scene::MAX_TEXT_ATLAS_BYTES,
                "{} bytes",
                s.text_bytes
            );
            if from_nothing && t < 1e-3 {
                // Under half a pixel: nothing shows, so nothing is built or laid out.
                assert_eq!(
                    s.builds, 0,
                    "a layer scaled to nothing builds nothing at {t}"
                );
            }
        }
        assert!(started.elapsed().as_secs() < 60, "{:?}", started.elapsed());
    }
}

/// The units a split text laid out reach the instance that draws it (its text is built on another
/// thread, from a copy of the instance): a title split by lines that wraps into four fades in line by line, so at
/// first none of its lines shows, though the model's estimate gave it one unit (the rest would stand unmoved).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_wrapped_title_fades_in_line_by_line() {
    let _gpu = gpu_turn().await;
    const WRAPPED: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"id":"t","type":"text","text":"one two three four five six","split":"lines","box":[0.05,0.05,0.12,0.9],
       "size":60,"weight":700,"fill":"#FFFFFF"}],
      "timeline":[{"from":"#t","dur":0.5,"opacity":0,"stagger":0.1}]}"##;
    let mut h = scene_headless(WRAPPED).await;
    let bright = |img: &image::RgbaImage| img.pixels().filter(|p| p.0[0] > 128).count();
    let (early, late) = (
        bright(&h.render_at(0.02).await),
        bright(&h.render_at(1.5).await),
    );
    assert!(
        early < 10 && late > 200,
        "no line shows at first, every line at the end: {early} then {late} pixels"
    );
}

/// Scene text is built on its own thread, never the render thread. The preview's first frame of the
/// lower third draws without its title (never a stall), and the title shows once its build lands; the export's way
/// (`Headless`) waits for the build on the same path.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn scene_text_is_built_off_the_render_thread() {
    let _gpu = gpu_turn().await;
    let elsewhere = klipolo_engine::render::scene::text_builds_on_other_threads();
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 4.0);
    let p = std::sync::Arc::new(p);
    let mut h = klipolo_engine::headless::Headless::preview(registry.clone())
        .await
        .unwrap();
    let (first, _) = scene_frame(&mut h, &p, 2.0, H_SIZE).await;
    let box_px = lower_third_title_box_px();
    let shown = |img: &image::RgbaImage| ink_bounds_in(img, box_px).is_some();
    let started = std::time::Instant::now();
    let mut later = first.clone();
    while !shown(&later) {
        assert!(
            started.elapsed().as_secs() < 30,
            "the title shows once built"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        later = scene_frame(&mut h, &p, 2.0, H_SIZE).await.0;
    }
    let mut e = klipolo_engine::headless::Headless::new(registry)
        .await
        .unwrap();
    let (exported, _) = scene_frame(&mut e, &p, 2.0, H_SIZE).await;
    assert!(shown(&exported), "the export waits for its build");
    assert_eq!(
        klipolo_engine::render::scene::text_builds_on_other_threads(),
        elsewhere,
        "every build ran on the text build thread"
    );
}
