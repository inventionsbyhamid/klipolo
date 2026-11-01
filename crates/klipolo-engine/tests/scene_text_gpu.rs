//! Scene text on the GPU (lavapipe in CI): long text shrinks, then wraps, then clips to its box; empty
//! and missing-glyph text is honest; a counting number lays nothing out once warm; letters rise one by one; a marker
//! highlight grows under its span; unusual text stays in its box; an emoji image node draws; a scene's atlases stay
//! within their budget. Goldens are compared within 2/255; `UPDATE_GOLDEN=1` writes them. Every test takes the GPU
//! turn (`gpu_turn`): some count GPU objects, which the others would make meanwhile.

mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn long_text_shrinks_then_wraps_then_clips() {
    let _gpu = gpu_turn().await;
    let mut h = scene_headless(LOWER_THIRD).await;
    let short = h.render_with_param("title", "Ana Lee").await;
    let long = h
        .render_with_param(
            "title",
            &"Maximiliana Wolfeschlegelsteinhausen-Bergerdorff".repeat(2),
        )
        .await;
    let box_px = lower_third_title_box_px();
    assert!(ink_bounds(&long).within(box_px), "never outside its box");
    assert!(
        ink_height(&long) < 2.0 * ink_height(&short) + 4.0,
        "shrinks before it wraps"
    );
    assert_eq!(h.notes(), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn empty_and_missing_glyph_text_is_honest() {
    let _gpu = gpu_turn().await;
    let mut h = scene_headless(LOWER_THIRD).await;
    let empty = h.render_with_param("title", "").await;
    assert_eq!(ink_bounds_in(&empty, lower_third_title_box_px()), None);
    let cjk = h.render_with_param("title", "李安").await; // no Noto SC downloaded in CI
    assert!(
        h.notes()
            .iter()
            .any(|n| n.starts_with("frame.cjkFontMissing")),
        "the missing-CJK-font code"
    );
    let _ = cjk;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_counting_number_lays_nothing_out_once_warm() {
    let _gpu = gpu_turn().await;
    let mut h = scene_headless(COUNTER).await; // counts 0 → 10,000 over 1 s
    for i in 0..10 {
        h.render_at(i as f64 / 60.0).await;
    }
    let (objects, layouts) = (
        klipolo_engine::render::effects::gpu_objects_made(),
        klipolo_engine::render::scene::text_layout_calls(),
    );
    for i in 10..310 {
        h.render_at(i as f64 / 300.0).await;
    }
    assert_eq!(
        klipolo_engine::render::effects::gpu_objects_made(),
        objects,
        "no GPU objects"
    );
    assert_eq!(
        klipolo_engine::render::scene::text_layout_calls(),
        layouts,
        "no layout per frame"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn letters_rise_one_by_one_and_a_counter_reads_its_value() {
    let _gpu = gpu_turn().await;
    let mut h = scene_headless(KINETIC_LETTERS).await;
    compare_golden(&h.render_at(0.25).await, "scene/letters-rise-0.25.png");
    let mut c = scene_headless(COUNTER).await;
    compare_golden(&c.render_at(1.0).await, "scene/counter-1.0.png"); // "10,000"
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_marker_highlight_grows_under_its_span() {
    let _gpu = gpu_turn().await;
    let mut h = scene_headless(MARKER).await;
    compare_golden(&h.render_at(0.6).await, "scene/marker-0.6.png");
    // It grows: wider at the end than while its letters still come in.
    let yellow = |img: &image::RgbaImage| {
        img.pixels()
            .filter(|p| p.0[0] > 200 && p.0[1] > 180 && p.0[2] < 120)
            .count()
    };
    let (early, late) = (
        yellow(&h.render_at(0.6).await),
        yellow(&h.render_at(1.5).await),
    );
    assert!(
        early > 0 && late > early + 200,
        "the marker grows: {early} then {late} pixels"
    );
}

/// The lower third in its hold, with a short name and with one too long for its box (shrunk to half, then wrapped).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_lower_third_draws_its_name_and_role() {
    let _gpu = gpu_turn().await;
    let mut h = scene_headless(LOWER_THIRD).await;
    compare_golden(
        &h.render_with_param("title", "Ana Lee").await,
        "scene/lower-third.png",
    );
    let long = "Maximiliana Wolfeschlegelsteinhausen-Bergerdorff".repeat(2);
    compare_golden(
        &h.render_with_param("title", &long).await,
        "scene/lower-third-long.png",
    );
}

/// Each unusual title draws inside its box, quickly, and says only what the text notes say.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn unusual_titles_stay_in_their_box() {
    let _gpu = gpu_turn().await;
    let mut h = scene_headless(LOWER_THIRD).await;
    let box_px = lower_third_title_box_px();
    let long = "Wolfeschlegelsteinhausen ".repeat(20);
    for title in [
        "Maximiliana Wolfeschlegelsteinhausen-Bergerdorff Ana Lee Jr.",
        // 500 characters: past what half its size holds even wrapped, so it clips to the box.
        long.as_str(),
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{2764}\u{FE0F} Ana",
        "Ana\nLee",
        "שלום עולם",
        "مرحبا بالعالم",
        "",
    ] {
        let started = std::time::Instant::now();
        let img = h.render_with_param("title", title).await;
        assert!(
            started.elapsed().as_secs() < 20,
            "{title:?} took {:?}",
            started.elapsed()
        );
        if let Some(ink) = ink_bounds_in(&img, lower_third_title_box_px()) {
            assert!(
                ink_bounds(&img).within(box_px),
                "{title:?}: {ink:?} past {box_px:?}"
            );
        }
    }
    assert!(
        h.notes()
            .iter()
            .all(|n| !n.starts_with("frame.sceneTextCut")),
        "{:?}",
        h.notes()
    );
}

/// An image node showing an emoji draws one picture in its box: in colour where the system has an emoji face, else the
/// text stand-in, never nothing and never past its box.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_emoji_image_node_draws_in_its_box() {
    let _gpu = gpu_turn().await;
    const EMOJI: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"image","emoji":"❤️","box":[0.4,0.3,0.2,0.4]}]}"##;
    let mut h = scene_headless(EMOJI).await;
    let img = h.render_at(1.0).await;
    let (w, hh) = (f64::from(H_SIZE.0), f64::from(H_SIZE.1));
    let inside = |x: u32, y: u32| {
        let (x, y) = (f64::from(x), f64::from(y));
        x >= 0.4 * w - 1.0 && x <= 0.6 * w + 1.0 && y >= 0.3 * hh - 1.0 && y <= 0.7 * hh + 1.0
    };
    let mut drawn = 0;
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[..3].iter().any(|&c| c > 24) {
            assert!(inside(x, y), "ink at ({x}, {y}) is past the box");
            drawn += 1;
        }
    }
    assert!(drawn > 50, "it draws: {drawn} pixels");
}

/// A scene's text atlases stay within 16 MiB a format, drawn softer past it (said once);
/// a fifth scene's text in one format pushes the least recently drawn out (an LRU of 4).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn text_atlases_stay_within_their_budget() {
    let _gpu = gpu_turn().await;
    // Split by word (words share no cell; repeated letters do).
    let big = "WW ".repeat(160);
    let big = big.trim_end();
    let node = |y: f64| {
        format!(
            r##"{{"type":"text","text":"{big}","box":[0,{y},1,0.25],"size":400,"fill":"#FFFFFF","split":"words"}}"##
        )
    };
    let json = format!(
        r#"{{"scene":1,"duration":2,"nodes":[{},{},{},{}]}}"#,
        node(0.0),
        node(0.25),
        node(0.5),
        node(0.75)
    );
    let mut h = scene_headless(&json).await;
    h.render_at(1.0).await;
    let stats = h.stats();
    assert!(
        stats.text_bytes <= klipolo_engine::render::scene::MAX_TEXT_ATLAS_BYTES,
        "{} bytes",
        stats.text_bytes
    );
    assert!(
        h.notes().iter().any(|n| n == "engine.frame.textsSofter"),
        "{:?}",
        h.notes()
    );
    // Five scenes with text, one after another, in one format: four kept.
    let mut h = scene_headless(LOWER_THIRD).await;
    for k in 0..5 {
        h.render_with_param("title", &format!("Name {k}")).await;
    }
    assert!(
        h.stats().texts <= klipolo_engine::render::scene::MAX_TEXT_SCENES,
        "{}",
        h.stats().texts
    );
}

/// What scene text costs (run by hand; the gate skips `scene_frame_costs`): building the lower third's
/// text (layout and atlas) on its first frame, and the drawer's CPU time a frame for the counter and the rising letters
/// while they move (lavapipe, debug: meaningful only against each other; the [Mac] check measures release on Metal).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI); run by hand"]
async fn scene_frame_costs_of_text() {
    let _gpu = gpu_turn().await;
    for (name, json, from, to) in [
        ("lower third", LOWER_THIRD, 0.4, 0.9),
        ("counter", COUNTER, 0.05, 0.95),
        ("letters", KINETIC_LETTERS, 0.05, 0.5),
        ("marker", MARKER, 0.2, 0.9),
    ] {
        let mut h = scene_headless(json).await;
        let started = std::time::Instant::now();
        h.render_at(from).await;
        let first = started.elapsed();
        let first_cpu = h.stats().cpu_us_last;
        let mut cpu = Vec::new();
        for i in 0..30 {
            h.render_at(from + (to - from) * f64::from(i) / 30.0).await;
            cpu.push(h.stats().cpu_us_last);
        }
        cpu.sort_by(f64::total_cmp);
        let s = h.stats();
        println!(
            "{name}: first frame {:.1} ms (drawer {:.0} µs, its text built then); moving frames drawer CPU median {:.0} µs, \
             p90 {:.0} µs; text kept {} at {} bytes",
            first.as_secs_f64() * 1e3,
            first_cpu,
            cpu[15],
            cpu[27],
            s.texts,
            s.text_bytes
        );
    }
}
