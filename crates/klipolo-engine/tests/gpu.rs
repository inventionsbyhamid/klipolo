//! GPU pixel tests (run by hand: `cargo test -p klipolo-engine --test gpu -- --ignored`). One per layer kind at most.

mod support;
// Mirrored video layers.
#[path = "gpu/mirror.rs"]
mod mirror;
use std::sync::Arc;

use klipolo_engine::plan::overlay::shown_from_us;
use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::project::{AssetKind, BackgroundKind, ColorLut, FormatKind, Project};
use klipolo_model::timeline::{Clip, ClipSource, ItemContent};
use support::*;

pub fn pixel(bytes: &[u8], stride: u32, x: u32, y: u32) -> [u8; 3] {
    let i = (y * stride + x * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2]]
}

pub fn near(a: [u8; 3], b: [u8; 3], tolerance: i16) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| (*x as i16 - y as i16).abs() <= tolerance)
}

fn use_fixture(p: &mut Project, asset: &str, file: &str, seconds: f64) {
    let a = p.assets.iter_mut().find(|a| a.id == asset).unwrap();
    a.path = support::fixture(file).display().to_string();
    a.duration_us = secs(seconds);
    a.kind = AssetKind::Video;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn video_layer_is_bt709_and_framed_on_the_background() {
    let mut p = project();
    use_fixture(&mut p, "broll", "solid-blue.mp4", 2.0);
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "broll".into() },
        0,
        secs(2.0),
    )];
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#FF0000".into();
    p.background.padding = 0.1;
    // The same clip as a small overlay in the top-left padding: 16:9 contained in a 32×36 px box (32×18, centred).
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![item(
            "pip",
            0.0,
            2.0,
            bx(0.0, 0.0, 0.05, 0.1),
            ItemContent::Video {
                asset_id: "broll".into(),
                source_in_us: 0,
                speed: 1.0,
                volume: 1.0,
                muted: false,
                mirror: false,
                tie: None,
            },
        )],
    )];
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let mut render = async |p: &Project| {
        let plan = frame_plan(
            &PlanContext::new(Arc::new(p.clone())),
            secs(1.0),
            FormatKind::Landscape,
        );
        backend
            .render(RenderRequest {
                plan,
                width: 640,
                height: 360,
                output: Output::Rgba,
            })
            .await
            .unwrap()
            .finish()
            .await
            .unwrap()
    };
    let frame = render(&p).await;
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
    let blue = [0x1E, 0x64, 0xDC];
    // Centre: the BT.709 blue round-trips (BT.601 decoding shifts it by well over 4 levels).
    let centre = pixel(&frame.bytes, frame.stride, 320, 180);
    assert!(near(centre, blue, 4), "{centre:?}");
    // Inside the 0.1 padding: the background.
    let corner = pixel(&frame.bytes, frame.stride, 5, 5);
    assert!(near(corner, [255, 0, 0], 2), "{corner:?}");
    // The overlay, and the background above and below its 16:9 picture.
    let pip = pixel(&frame.bytes, frame.stride, 16, 18);
    assert!(near(pip, blue, 6), "overlay: {pip:?}");
    assert!(near(
        pixel(&frame.bytes, frame.stride, 16, 4),
        [255, 0, 0],
        2
    ));

    // An inverting 2³ LUT on the clip turns the main footage's blue into its complement; overlays keep no look.
    let dir = tempfile::tempdir().unwrap();
    let cube = dir.path().join("invert.cube");
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for b in [0, 1] {
        for g in [0, 1] {
            for r in [0, 1] {
                text += &format!("{} {} {}\n", 1 - r, 1 - g, 1 - b);
            }
        }
    }
    std::fs::write(&cube, text).unwrap();
    p.luts = vec![ColorLut {
        id: "invert".into(),
        name: "Invert".into(),
        path: cube.display().to_string(),
    }];
    p.timeline.main[0].look.lut_id = Some("invert".into());
    let frame = render(&p).await;
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
    let centre = pixel(&frame.bytes, frame.stride, 320, 180);
    assert!(near(centre, [0xE1, 0x9B, 0x23], 6), "inverted: {centre:?}");
    let pip = pixel(&frame.bytes, frame.stride, 16, 18);
    assert!(near(pip, blue, 6), "overlay without the look: {pip:?}");
}

/// Cap's readback is pipelined (it returns the previous frame); a seek must always show the requested time.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn every_render_shows_the_requested_time() {
    let mut p = project();
    use_fixture(&mut p, "broll", "solid-blue.mp4", 2.0);
    use_fixture(&mut p, "screen", "bars-30fps-0.5gop.mp4", 4.0);
    let mut blue = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(2.0));
    blue.id = "blue".into();
    let mut bars = Clip::new(
        ClipSource::Asset {
            id: "screen".into(),
        },
        0,
        secs(2.0),
    );
    bars.id = "bars".into();
    p.timeline.main = vec![blue, bars];
    let ctx = PlanContext::new(Arc::new(p));
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let blue_px = [0x1E, 0x64, 0xDC];
    for (at, expect_blue) in [(0.5, true), (3.0, false), (1.0, true), (2.5, false)] {
        let plan = frame_plan(&ctx, secs(at), FormatKind::Landscape);
        let frame = backend
            .render(RenderRequest {
                plan,
                width: 320,
                height: 180,
                output: Output::Rgba,
            })
            .await
            .unwrap()
            .finish()
            .await
            .unwrap();
        let centre = pixel(&frame.bytes, frame.stride, 40, 90);
        assert_eq!(
            near(centre, blue_px, 6),
            expect_blue,
            "at {at}s: {centre:?}"
        );
    }
}

/// Phones store portrait video as landscape frames plus a display matrix; imports record the upright size and the
/// compositor turns the stored frames to match.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn phone_video_is_drawn_upright() {
    let mut p = project();
    use_fixture(&mut p, "broll", "phone-portrait.mov", 1.0);
    let phone = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    (phone.width, phone.height) = (720, 1280); // what the import probe stores
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "broll".into() },
        0,
        secs(1.0),
    )];
    let plan = frame_plan(
        &PlanContext::new(Arc::new(p)),
        secs(0.5),
        FormatKind::Portrait,
    );
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let frame = backend
        .render(RenderRequest {
            plan,
            width: 360,
            height: 640,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    let at = |x, y| pixel(&frame.bytes, frame.stride, x, y);
    // Stored left (red) is shown on top, stored right (blue) below, across the whole width.
    for (x, y, rgb) in [
        (40, 60, [255, 0, 0]),
        (320, 60, [255, 0, 0]),
        (40, 580, [0, 0, 255]),
        (320, 580, [0, 0, 255]),
    ] {
        assert!(near(at(x, y), rgb, 12), "({x}, {y}): {:?}", at(x, y));
    }
}

async fn render_plan(p: Project, at: f64) -> klipolo_engine::session::RenderedRgba {
    let plan = frame_plan(
        &PlanContext::new(Arc::new(p)),
        secs(at),
        FormatKind::Landscape,
    );
    GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets()
        .render(RenderRequest {
            plan,
            width: 640,
            height: 360,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap()
}

/// A moved image is said as a moved video is (`Rendered::missing`, the copy row's code), never as an
/// English warning; the frame still draws without it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_missing_image_is_said_as_a_missing_video_is() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone.png");
    let mut p = red_background(project());
    p.assets.iter_mut().find(|a| a.id == "logo").unwrap().path = gone.display().to_string();
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![item(
            "logo",
            0.0,
            2.0,
            bx(0.25, 0.25, 0.5, 0.5),
            ItemContent::Image {
                asset_id: "logo".into(),
            },
        )],
    )];
    let f = render_plan(p, 1.0).await;
    assert_eq!(f.missing, [Arc::from(gone.display().to_string())]);
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    assert!(near(pixel(&f.bytes, f.stride, 320, 180), [255, 0, 0], 2));
}

/// A moved background image is said as a moved video is (by type, not by its message); the frame draws
/// black behind.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_missing_background_image_is_said_as_a_missing_video_is() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("backdrop.png");
    let mut p = project();
    p.timeline.main.clear();
    p.timeline.tracks.clear();
    p.assets.iter_mut().find(|a| a.id == "logo").unwrap().path = gone.display().to_string();
    p.background.kind = BackgroundKind::Image;
    p.background.asset_id = Some("logo".into());
    let f = render_plan(p, 1.0).await;
    assert_eq!(f.missing, [Arc::from(gone.display().to_string())]);
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    assert!(near(pixel(&f.bytes, f.stride, 320, 180), [0, 0, 0], 2));
}

/// The preview backend's decoder pool holds its redraw signal (`GpuBackend::assemble`): a still drawn
/// while a file's disk is slow to answer (`slow_stats`) is drawn again once it answers, with nothing else happening, and
/// says what the disk answered (here, that the file isn't there).
#[cfg(feature = "trial")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_still_drawn_while_a_disk_is_slow_is_drawn_again_when_it_answers() {
    use klipolo_engine::render::pool::slow_stats;
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let file: Arc<str> = dir.path().join("intro.mp4").display().to_string().into();
    slow_stats(file.clone(), Some(Duration::from_millis(1_500)));
    let mut p = project();
    p.assets.iter_mut().find(|a| a.id == "broll").unwrap().path = file.to_string();
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "broll".into() },
        0,
        secs(2.0),
    )];
    let plan = frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape);
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap();
    let redraw = backend.redraw().expect("the preview's redraw signal");
    let mut draw = async || {
        backend
            .render(RenderRequest {
                plan: plan.clone(),
                width: 320,
                height: 180,
                output: Output::Rgba,
            })
            .await
            .unwrap()
            .finish()
            .await
            .unwrap()
    };
    let first = draw().await;
    assert!(first.missing.is_empty() && first.warnings.is_empty());
    // Other loads (the font scan) may wake it too: each wake draws the still again, until one says the answer.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::timeout_at(deadline, redraw.notified())
            .await
            .expect("drawn again once the disk answers");
        let frame = draw().await;
        if !frame.missing.is_empty() {
            assert_eq!(frame.missing, std::slice::from_ref(&file));
            break;
        }
    }
    slow_stats(file, None);
}

fn red_background(mut p: Project) -> Project {
    p.timeline.main.clear();
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#FF0000".into();
    p
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn image_layer_is_drawn_contained() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("green.png");
    image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 255, 0, 255]))
        .save(&png)
        .unwrap();
    let mut p = red_background(project());
    p.assets.iter_mut().find(|a| a.id == "logo").unwrap().path = png.display().to_string();
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![item(
            "logo",
            0.0,
            2.0,
            bx(0.25, 0.25, 0.5, 0.5),
            ItemContent::Image {
                asset_id: "logo".into(),
            },
        )],
    )];
    // The preview never waits for a decode: the first frame draws without the image, the redraw signal fires
    // when it is uploaded, and the next frame has it.
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap();
    let redraw = backend.redraw().unwrap();
    let plan = frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape);
    let mut render = async || {
        let req = RenderRequest {
            plan: plan.clone(),
            width: 640,
            height: 360,
            output: Output::Rgba,
        };
        backend.render(req).await.unwrap().finish().await.unwrap()
    };
    let first = render().await;
    assert!(near(
        pixel(&first.bytes, first.stride, 320, 180),
        [255, 0, 0],
        2
    ));
    // The font scan a new backend starts fires the same signal when it finishes, which may come first.
    let f = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            redraw.notified().await;
            let f = render().await;
            if near(pixel(&f.bytes, f.stride, 320, 180), [0, 255, 0], 2) {
                break f;
            }
        }
    })
    .await
    .expect("redraw after the image loads, then drawn with it");
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    assert!(near(pixel(&f.bytes, f.stride, 320, 180), [0, 255, 0], 2));
    assert!(
        near(pixel(&f.bytes, f.stride, 170, 180), [255, 0, 0], 2),
        "contained: the 1:1 image leaves the box's sides empty"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn text_layer_draws_glyphs_inside_its_box() {
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item("t", 0.0, 2.0, bx(0.1, 0.4, 0.8, 0.2), text("MMMMMM"))],
    )];
    let f = render_plan(p, 1.0).await;
    let bright = (64..576)
        .step_by(4)
        .flat_map(|x| (150..210).step_by(4).map(move |y| (x, y)))
        .filter(|&(x, y)| pixel(&f.bytes, f.stride, x, y)[0] > 200)
        .count();
    assert!(bright > 20, "white glyph pixels inside the box: {bright}");
    assert_eq!(pixel(&f.bytes, f.stride, 10, 10), [0, 0, 0]);
}

/// An empty project with a Waves layer and a text (0–10 s each) draws both at 5 s, not black
/// (its export's frame count: `canvas.rs`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_canvas_of_only_layers_draws_them_at_five_seconds() {
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    let waves = ItemContent::generator(
        "klipolo.builtin/generator.waves".into(),
        std::collections::BTreeMap::new(),
    );
    p.timeline.tracks = vec![overlay_track(
        "bg",
        vec![item("bg", 0.0, 10.0, bx(0.0, 0.0, 1.0, 1.0), waves)],
    )];
    let without_text = render_plan(p.clone(), 5.0).await;
    p.timeline.tracks.push(overlay_track(
        "titles",
        vec![item("t", 0.0, 10.0, bx(0.1, 0.4, 0.8, 0.2), text("MMMMMM"))],
    ));
    let f = render_plan(p, 5.0).await;
    let samples = || {
        (0..640)
            .step_by(8)
            .flat_map(|x| (0..360).step_by(8).map(move |y| (x, y)))
    };
    let lit = samples()
        .filter(|&(x, y)| pixel(&f.bytes, f.stride, x, y) != [0, 0, 0])
        .count();
    assert!(
        lit > samples().count() / 2,
        "the generator fills the frame: {lit}"
    );
    let changed = samples()
        .filter(|&(x, y)| {
            !near(
                pixel(&f.bytes, f.stride, x, y),
                pixel(&without_text.bytes, without_text.stride, x, y),
                24,
            )
        })
        .count();
    assert!(changed > 10, "the text draws over it: {changed}");
}

/// A word wider than its title's box breaks between its letters inside the box, as the
/// canvas editor and the webview's measure break it, instead of being cut at the box's edge (preview and export).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_word_wider_than_its_box_breaks_inside_it_and_is_never_cut() {
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    // 72 px at 1080 is 24 px at 360: twenty Ms are about 400 px wide, the box 0.15 × 640 = 96 px (wrapped at ~105).
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item(
            "t",
            0.0,
            2.0,
            bx(0.4, 0.4, 0.15, 0.1),
            text("MMMMMMMMMMMMMMMMMMMM"),
        )],
    )];
    let f = render_plan(p, 1.0).await;
    let lit: Vec<(u32, u32)> = (0..640)
        .flat_map(|x| (0..360).map(move |y| (x, y)))
        .filter(|&(x, y)| pixel(&f.bytes, f.stride, x, y)[0] > 200)
        .collect();
    let (left, right) = lit
        .iter()
        .fold((u32::MAX, 0), |(l, r), &(x, _)| (l.min(x), r.max(x)));
    let (top, bottom) = lit
        .iter()
        .fold((u32::MAX, 0), |(t, b), &(_, y)| (t.min(y), b.max(y)));
    // Lines 28.8 px apart: the word runs on over at least three of them, all within the box's wrap width.
    assert!(
        bottom - top >= 60,
        "the word goes on under its first line: rows {top}..{bottom}"
    );
    assert!(
        right - left <= 112,
        "every letter within the box's wrap width: columns {left}..{right}"
    );
}

/// A text whose font isn't here draws in the stand-in (Inter) and the frame says so, so an export
/// stops on it unless the person chose the stand-in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_in_a_missing_font_is_drawn_in_inter_and_the_frame_says_so() {
    klipolo_engine::render::fonts::configure_for_tests();
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    let mut content = text("MMMMMM");
    if let ItemContent::Text { style, .. } = &mut content {
        style.font = klipolo_model::font::FontRef {
            family: "Nowhere Sans".into(),
            source: klipolo_model::font::FontSource::Installed,
        };
    }
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item("t", 0.0, 2.0, bx(0.1, 0.4, 0.8, 0.2), content)],
    )];
    let f = render_plan(p, 1.0).await;
    assert!(
        f.warnings.iter().any(|w| w == "frame.fontMissing"),
        "{:?}",
        f.warnings
    );
    let bright = (64..576)
        .step_by(4)
        .flat_map(|x| (150..210).step_by(4).map(move |y| (x, y)))
        .filter(|&(x, y)| pixel(&f.bytes, f.stride, x, y)[0] > 200)
        .count();
    assert!(bright > 20, "drawn in the stand-in: {bright}");
}

/// A Japanese title in an English project needs Noto Sans JP; until it is downloaded the
/// frame says so and no other face is borrowed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn japanese_text_without_its_noto_download_says_so_and_borrows_no_other_face() {
    klipolo_engine::render::fonts::configure_for_tests();
    let mut p = red_background(project());
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item(
            "t",
            0.0,
            2.0,
            bx(0.1, 0.4, 0.8, 0.2),
            text("今日は予算"),
        )],
    )];
    let f = render_plan(p, 1.0).await;
    assert!(
        f.warnings.iter().any(|w| w == "frame.cjkFontMissing"),
        "{:?}",
        f.warnings
    );
    assert!(!klipolo_engine::render::fonts::cjk_registered(
        klipolo_model::text_language::CjkScript::Jp
    ));
}

/// One landscape frame (1920 × 1080) at `at` s of a black project whose captions (the built-in `style`, `caption.karaoke`)
/// show the page "Hello world" from 1 s to 2 s, "world" said from 1.4 s to 1.8 s.
async fn render_with_captions(style: &str, at: f64) -> klipolo_engine::session::RenderedRgba {
    use std::collections::BTreeMap;

    use klipolo_model::captions::{Captions, Pages};

    let mut p = Project::new("Captions");
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#000000".into();
    p.captions.enabled = true;
    p.captions.style = Some(klipolo_model::contrib::ContribId::builtin(style));
    let page = Pages::one_page(
        "v",
        secs(1.0),
        secs(2.0),
        &[
            ("Hello", secs(1.0), secs(1.3)),
            ("world", secs(1.4), secs(1.8)),
        ],
    );
    let captions = Captions {
        style: klipolo_model::contrib::ContribId::builtin(style),
        pages: BTreeMap::from([(FormatKind::Landscape, Arc::new(page))]),
    };
    let ctx = PlanContext::new(Arc::new(p)).with_captions(Arc::new(captions));
    let plan = frame_plan(&ctx, secs(at), FormatKind::Landscape);
    GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets()
        .render(RenderRequest {
            plan,
            width: 1920,
            height: 1080,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap()
}

/// Karaoke draws the word being said in #FFD400 (its style's accent) and the word before it white,
/// in its own face, TikTok Sans ExtraBold, through the caption renderer.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn karaoke_caption_draws_the_word_being_said_in_yellow() {
    let f = render_with_captions("caption.karaoke", 1.5).await;
    let (mut yellow, mut white) = (Vec::new(), Vec::new());
    // Karaoke's landscape block is centred at y 0.85 of 1080.
    for y in 800..1040 {
        for x in 0..1920 {
            match pixel(&f.bytes, f.stride, x, y) {
                [r, g, b] if r > 200 && g > 170 && g < 230 && b < 90 => yellow.push(x),
                [r, g, b] if r > 220 && g > 220 && b > 220 => white.push(x),
                _ => {}
            }
        }
    }
    let mean = |xs: &[u32]| xs.iter().map(|&x| f64::from(x)).sum::<f64>() / xs.len().max(1) as f64;
    assert!(
        yellow.len() > 200,
        "\"world\" is yellow ({} px)",
        yellow.len()
    );
    assert!(white.len() > 200, "\"Hello\" is white ({} px)", white.len());
    assert!(
        mean(&yellow) > mean(&white),
        "the yellow word is the second: {:.0} vs {:.0}",
        mean(&yellow),
        mean(&white)
    );
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
}

/// The render thread never builds a caption page. The preview's first frame of a page
/// asks the build thread for it and draws nothing (never a stall); the page shows once the reply lands. A page drawn
/// the export's way waits for its reply on the same path, and is still built on the build thread.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_render_thread_never_builds_a_caption_page() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    klipolo_engine::render::captions::forbid_builds_on_this_thread();
    assert!(
        !d.preview_page("caption.bold", 0),
        "nothing to draw yet: the page is being built"
    );
    let started = std::time::Instant::now();
    while !d.preview_page("caption.bold", 0) {
        assert!(started.elapsed().as_secs() < 30, "the page shows");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(d.builds() >= 1);
    d.draw_page("caption.bold", 7).await;
}

/// While a page's atlas is built again under another key (a hovered style or font, a
/// new output size), the preview draws the same page's last atlas, never a blank frame; a page with other words is
/// never stood in for.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_preview_draws_a_pages_last_atlas_while_its_new_one_builds() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    d.draw_page("caption.bold", 0).await;
    assert!(
        d.preview_page("caption.loud", 0),
        "page 0 in Loud: Bold's atlas of it while Loud's builds"
    );
    assert!(
        !d.preview_page("caption.loud", 5),
        "another page's words are never drawn for it"
    );
}

/// At 2160 × 3840 a Bold page is about 4.7 MB, so the byte bound evicts before the count does:
/// the atlases kept stay within 16 MB, fewer than four, and the page drawn is still kept.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_atlas_cache_keeps_its_byte_budget() {
    let mut d = klipolo_engine::test_support::caption_drawer_at((2160, 3840)).await;
    for page in 30..40 {
        d.draw_page("caption.bold", page).await;
    }
    eprintln!(
        "caption atlases kept (Bold, 2160x3840, lavapipe): {} atlases, {} bytes",
        d.atlases(),
        d.atlas_bytes()
    );
    assert!(
        d.atlas_bytes() <= 16 << 20 && d.atlases() < 4,
        "{} atlases, {} bytes",
        d.atlases(),
        d.atlas_bytes()
    );
    let builds = d.builds();
    d.draw_page("caption.bold", 39).await;
    assert_eq!(d.builds(), builds, "the page drawn is kept");
}

/// The caption drawer keeps at most four atlases per format, within 16 MB, and the same page in a
/// new revision whose words didn't change (a trim elsewhere, a caption drag) finds its atlas: no build.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_atlas_cache_reuses_unchanged_pages_and_keeps_its_budget() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    // 40 pages, each drawn once: at most 4 atlases per format stay, within 16 MB.
    for page in 0..40 {
        d.draw_page("caption.bold", page).await;
    }
    assert!(
        d.atlases() <= 4 && d.atlas_bytes() <= 16 << 20,
        "{} atlases, {} bytes",
        d.atlases(),
        d.atlas_bytes()
    );
    eprintln!(
        "caption atlases kept (Bold, 1080x1920, lavapipe): {} atlases, {} bytes, {} builds",
        d.atlases(),
        d.atlas_bytes(),
        d.builds()
    );
    // The same page in a new revision whose words didn't change (a trim elsewhere, a caption drag): no build.
    let builds = d.builds();
    d.draw_page_in_new_revision("caption.bold", 39).await;
    assert_eq!(d.builds(), builds);
}

/// A new title (it fades in, as the editor adds it) at the time the editor puts the playhead to show it
/// (`shown_from_us`): the preview's first frame, which never waits for the font scan, and export both draw it. At the
/// title's own start the fade-in hides it in both, so a paused preview parked there looked like a missing title
/// that export had.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_new_title_shows_where_the_editor_puts_the_playhead() {
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    let title = animated(
        item("t", 1.0, 3.0, bx(0.1, 0.4, 0.8, 0.2), text("MMMMMM")),
        Some("fade"),
        Some("fade"),
    );
    let shown = shown_from_us(&title);
    assert_eq!(shown, secs(1.3));
    p.timeline.tracks = vec![overlay_track("t", vec![title])];
    let ctx = ctx(p);
    let glyphs = async |backend: &mut GpuBackend, at| {
        let plan = frame_plan(&ctx, at, FormatKind::Landscape);
        let req = RenderRequest {
            plan,
            width: 640,
            height: 360,
            output: Output::Rgba,
        };
        let f = backend.render(req).await.unwrap().finish().await.unwrap();
        (64..576)
            .step_by(4)
            .flat_map(|x| (150..210).step_by(4).map(move |y| (x, y)))
            .filter(|&(x, y)| pixel(&f.bytes, f.stride, x, y)[0] > 200)
            .count()
    };
    let mut preview = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap();
    let mut export = GpuBackend::for_export().await.unwrap();
    for (name, backend) in [("preview", &mut preview), ("export", &mut export)] {
        let drawn = glyphs(backend, shown).await;
        assert!(
            drawn > 20,
            "{name}: white glyph pixels once it is shown: {drawn}"
        );
        assert_eq!(
            glyphs(backend, secs(1.0)).await,
            0,
            "{name}: its first frame is fully faded out"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn black_layer_fades_what_is_below() {
    let mut p = red_background(project());
    let mut clip = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(2.0));
    clip.fade_in_us = secs(1.0);
    p.timeline.main = vec![clip];
    // The clip's file is missing, so only the background shows under the half-way fade.
    p.assets.iter_mut().find(|a| a.id == "broll").unwrap().path = "/nope.mp4".into();
    let f = render_plan(p, 0.5).await;
    assert!(near(pixel(&f.bytes, f.stride, 320, 180), [128, 0, 0], 3));
}

/// The native path end to end, without a window: a frame rendered with `Output::Native` lands in a slot (nothing is
/// read back), and the presenter copies it into a detached CAMetalLayer's drawable and presents it. Releasing the
/// drawables (hidden, or no project open) never sets the layer's `contents`, which Core Animation logged as undefined
/// behaviour once per native launch, and the next frame shows as usual.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn native_frames_reach_a_metal_layer_without_a_readback() {
    use klipolo_engine::frames::trailer::FrameMeta;
    use klipolo_engine::platform::NativeSurface;
    use klipolo_engine::present::FramePresenter;
    use klipolo_engine::present::macos::NativeLayer;
    use klipolo_engine::present::native::ShownSink;
    use klipolo_engine::session::Pixels;

    let mut p = project();
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#FF0000".into();
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let (tx, mut shown) = tokio::sync::mpsc::unbounded_channel();
    let sink: ShownSink = Arc::new(move |meta: &FrameMeta, _, _| {
        let _ = tx.send(meta.time_us);
    });
    let mut presenter = backend
        .attach_native(NativeSurface::Metal(NativeLayer::detached()), sink)
        .unwrap();
    let plan = frame_plan(&ctx(p), secs(0.0), FormatKind::Landscape);
    let mut show = async |presenter: &mut dyn FramePresenter, time_us| {
        let mut rendered = backend
            .render(RenderRequest {
                plan: plan.clone(),
                width: 320,
                height: 180,
                output: Output::Native,
            })
            .await
            .unwrap();
        assert!(matches!(rendered.pixels, Pixels::Native(Some(_))));
        let frame = rendered.pixels.frame().await.unwrap();
        presenter.present(
            frame,
            FrameMeta {
                time_us,
                ..FrameMeta::default()
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), shown.recv())
            .await
            .expect("presented within 3 s")
    };
    assert_eq!(show(&mut presenter, 1234).await, Some(1234));

    // Core Animation writes its warning straight to fd 2: send it to a file while the presenter releases, then shows
    // the next frame (which it does only after the release).
    let mut log = tempfile::tempfile().unwrap();
    // SAFETY: fd 2 is saved, pointed at the file and put back; the file outlives both.
    let saved = unsafe { libc::dup(2) };
    unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&log), 2) };
    presenter.release();
    let after = show(&mut presenter, 5678).await;
    unsafe {
        libc::dup2(saved, 2);
        libc::close(saved);
    }
    assert_eq!(after, Some(5678));
    let mut logged = String::new();
    std::io::Seek::rewind(&mut log).unwrap();
    std::io::Read::read_to_string(&mut log, &mut logged).unwrap();
    assert!(!logged.contains("CAMetalLayer"), "{logged}");
}

/// Playback frames over the socket are NV12: BT.709 limited-range luma, then interleaved chroma, one stride apart.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn nv12_frames_are_bt709_limited_range() {
    let mut p = project();
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#FF0000".into();
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let plan = frame_plan(&ctx(p), secs(0.0), FormatKind::Landscape);
    let frame = backend
        .render(RenderRequest {
            plan,
            width: 322,
            height: 180,
            output: Output::Nv12,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert_eq!(frame.stride, 324, "rows padded to 4 bytes");
    let uv = (frame.stride * frame.height) as usize;
    assert_eq!(frame.bytes.len(), uv + uv / 2);
    // Pure red: Y 16 + 219 × 0.2126, Cb 128 − 224 × 0.1146, Cr 128 + 224 × 0.5.
    let (y, cb, cr) = (frame.bytes[0], frame.bytes[uv], frame.bytes[uv + 1]);
    assert!(
        (62..=63).contains(&y) && (101..=103).contains(&cb) && (239..=240).contains(&cr),
        "{y} {cb} {cr}"
    );
}

/// Play, pause, play again on the socket: the still between renders at full size, so the session texture is made
/// again at the playback size, and the NV12 conversion must read the new texture, not the first one (it froze).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn nv12_playback_after_a_pause_shows_the_current_frame() {
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let mut render = async |colour: &str, output: Output, (width, height): (u32, u32)| {
        let mut p = project();
        p.background.kind = BackgroundKind::Solid;
        p.background.color = colour.into();
        let plan = frame_plan(&ctx(p), secs(0.0), FormatKind::Landscape);
        backend
            .render(RenderRequest {
                plan,
                width,
                height,
                output,
            })
            .await
            .unwrap()
            .finish()
            .await
            .unwrap()
    };
    let (still, playback) = ((1280, 720), (960, 540));
    let luma = |frame: &klipolo_engine::session::RenderedRgba| frame.bytes[0];
    render("#000000", Output::Rgba, still).await;
    // BT.709 limited range: pure red's Y is 62, pure blue's 31.
    assert_eq!(luma(&render("#FF0000", Output::Nv12, playback).await), 62);
    render("#000000", Output::Rgba, still).await;
    assert_eq!(
        luma(&render("#0000FF", Output::Nv12, playback).await),
        31,
        "the frame being played, not the first"
    );
}

/// The warm-up at Home compiles every pipeline on its own thread and hands the engine's first start a
/// backend that draws.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_warm_up_hands_over_a_backend_that_draws() {
    let mut backend = klipolo_engine::render::warm::Warmup::start(
        klipolo_engine::render::mode::RenderMode::Interactive,
    )
    .finish()
    .await
    .expect("a warmed backend")
    .wait_for_assets();
    let plan = frame_plan(
        &ctx(red_background(project())),
        secs(0.0),
        FormatKind::Landscape,
    );
    let frame = backend
        .render(RenderRequest {
            plan,
            width: 320,
            height: 180,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert!(near(
        pixel(&frame.bytes, frame.stride, 160, 90),
        [255, 0, 0],
        2
    ));
}

/// `p` on a `background` canvas with one `rgb` square image layer, 0–4 s, centred in a half-size box: its blend and
/// motion are the caller's.
fn square_over(background: &str, rgb: [u8; 3], dir: &std::path::Path) -> Project {
    let png = dir.join(format!("{:02x}{:02x}{:02x}.png", rgb[0], rgb[1], rgb[2]));
    image::RgbaImage::from_pixel(64, 64, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
        .save(&png)
        .unwrap();
    let mut p = red_background(project());
    p.background.color = background.into();
    p.assets.iter_mut().find(|a| a.id == "logo").unwrap().path = png.display().to_string();
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![item(
            "logo",
            0.0,
            4.0,
            bx(0.25, 0.25, 0.5, 0.5),
            ItemContent::Image {
                asset_id: "logo".into(),
            },
        )],
    )];
    p
}

fn with_blend(mut p: Project, blend: klipolo_model::motion::Blend) -> Project {
    p.timeline.tracks[0].items[0].layout.blend = Some(blend);
    p
}

/// Multiply and screen are blend states of the layer pipeline: 50 % grey multiplied over white is 50 %
/// grey, and screened over white stays white.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_multiply_layer_darkens_by_what_is_under_it() {
    use klipolo_model::motion::Blend;
    let dir = tempfile::tempdir().unwrap();
    let grey = square_over("#FFFFFF", [128, 128, 128], dir.path());
    let f = render_plan(with_blend(grey.clone(), Blend::Multiply), 1.0).await;
    let centre = pixel(&f.bytes, f.stride, 320, 180);
    assert!(near(centre, [128, 128, 128], 2), "multiply: {centre:?}");
    let f = render_plan(with_blend(grey, Blend::Screen), 1.0).await;
    let centre = pixel(&f.bytes, f.stride, 320, 180);
    assert!(near(centre, [255, 255, 255], 2), "screen: {centre:?}");
}

/// Motion is planned, not drawn: a still layer and the same layer sliding in with keys draw the same
/// number of passes. Overlay reads what is under it: one read-below pass, and the layer pass it splits continues
/// after it (two more). White overlaid on dark grey brightens it to twice its value.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn motion_without_effects_adds_no_pass() {
    use klipolo_model::motion::{Blend, Key, Motion, MotionKeys, MotionUse};
    let dir = tempfile::tempdir().unwrap();
    let still = square_over("#404040", [255, 255, 255], dir.path());
    let mut moving = still.clone();
    moving.timeline.tracks[0].items[0].layout.motion = Some(Motion {
        enter: Some(MotionUse {
            preset: klipolo_model::contrib::builtin::motion("slide"),
            params: Default::default(),
            duration_us: secs(1.0),
            ease: None,
        }),
        keys: Some(MotionKeys {
            x: vec![
                Key {
                    t_us: 0,
                    v: 0.0,
                    ease: None,
                },
                Key {
                    t_us: secs(4.0),
                    v: 0.1,
                    ease: None,
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let mut passes = async |p: &Project, at: f64| {
        let plan = frame_plan(&ctx(p.clone()), secs(at), FormatKind::Landscape);
        let req = RenderRequest {
            plan,
            width: 640,
            height: 360,
            output: Output::Rgba,
        };
        let f = backend.render(req).await.unwrap().finish().await.unwrap();
        (backend.passes_last_frame(), f)
    };
    let (alone, _) = passes(&still, 2.0).await;
    let (moved, _) = passes(&moving, 0.5).await;
    assert_eq!(moved, alone, "motion adds no pass");
    let (overlaid, f) = passes(&with_blend(still, Blend::Overlay), 2.0).await;
    assert_eq!(
        overlaid,
        alone + 2,
        "the read-below pass, and the layer pass after it"
    );
    let centre = pixel(&f.bytes, f.stride, 320, 180);
    assert!(near(centre, [128, 128, 128], 2), "overlay: {centre:?}");
    assert!(
        near(pixel(&f.bytes, f.stride, 20, 20), [64, 64, 64], 2),
        "outside the layer"
    );
}

/// A title keyed up 1× → 3× is laid out once, past its largest size (4×, the next √2 step), with mipmaps,
/// and drawn smaller until it grows: the same texture on every frame, and its quad its box times its scale.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_keyed_up_is_laid_out_once_at_its_largest_size() {
    use klipolo_engine::plan::Layer;
    use klipolo_engine::render::text::TextDrawer;
    use klipolo_model::motion::{Key, Motion, MotionKeys};
    let mut title = item("t", 0.0, 3.0, bx(0.3, 0.4, 0.4, 0.1), text("Grow"));
    title.layout.motion = Some(Motion {
        keys: Some(MotionKeys {
            scale: vec![
                Key {
                    t_us: 0,
                    v: 1.0,
                    ease: None,
                },
                Key {
                    t_us: secs(3.0),
                    v: 3.0,
                    ease: None,
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut p = red_background(project());
    p.timeline.tracks = vec![overlay_track("t", vec![title])];
    let c = ctx(p);
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let mut first: Option<wgpu::Texture> = None;
    for k in 0..10 {
        let plan = frame_plan(&c, secs(0.3 * f64::from(k)), FormatKind::Landscape);
        let Some(Layer::Text(t)) = plan.layers.iter().find(|l| matches!(l, Layer::Text(_))) else {
            panic!("the title draws");
        };
        let (texture, quad, up) = drawer
            .prepare(
                &dev.device,
                &dev.queue,
                t,
                plan.canvas,
                (640, 360),
                &mut Vec::new(),
            )
            .unwrap();
        drawer.end_frame();
        assert_eq!(up, 4.0);
        assert!(
            texture.mip_level_count() > 1,
            "mipmapped: drawn smaller than laid out"
        );
        let drawn = f64::from(texture.width()) * t.scale / up;
        assert!((quad.size.x - drawn).abs() < 1e-9);
        match &first {
            None => first = Some(texture),
            Some(f) => assert!(*f == texture, "laid out again at frame {k}"),
        }
    }
    // Its texture is cut to its ink, laid out at 4×: at least three times the box's height.
    let box_h = 0.1 * 360.0;
    assert!(f64::from(first.unwrap().height()) >= 3.0 * box_h);
}

/// The camera bubble turns with its rotation keys (review: draw them), in its one composite pass: a square bubble
/// turned 45° leaves its corners to the screen under it, and its centre is still the camera.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn the_camera_bubble_turns_with_its_rotation_keys() {
    use klipolo_engine::plan::VideoRole;
    use klipolo_model::motion::{Key, Motion, MotionKeys};
    use klipolo_model::timeline::{CameraLayout, CameraShape};
    let mut p = project();
    use_fixture(&mut p, "screen", "solid-red.mp4", 2.0);
    use_fixture(&mut p, "camera", "solid-blue.mp4", 2.0);
    let mut clip = Clip::new(ClipSource::Recording { id: "rec".into() }, 0, secs(2.0));
    clip.id = "a".into();
    clip.camera = Some(CameraLayout {
        shape: CameraShape::Square,
        border: false,
        ..CameraLayout::default()
    });
    p.timeline.main = vec![clip];
    let turned = |deg: f64| {
        let mut p = p.clone();
        p.timeline.main[0].camera.as_mut().unwrap().bubble.motion = Some(Motion {
            keys: Some(MotionKeys {
                rotation: vec![Key {
                    t_us: 0,
                    v: deg,
                    ease: None,
                }],
                ..Default::default()
            }),
            ..Default::default()
        });
        p
    };
    let plan = frame_plan(&ctx(turned(45.0)), secs(1.0), FormatKind::Landscape);
    let camera = videos(&plan)
        .into_iter()
        .find(|v| v.role == VideoRole::Camera)
        .unwrap();
    assert_eq!(camera.rotation, 45.0);
    let cam = camera.dest;
    let at = |fx: f64, fy: f64| {
        (
            ((cam.x + cam.w * fx) * 640.0) as u32,
            ((cam.y + cam.h * fy) * 360.0) as u32,
        )
    };
    let (corner, centre) = (at(0.12, 0.12), at(0.5, 0.5));
    let (red, blue) = ([255, 0, 0], [0x1E, 0x64, 0xDC]);
    let upright = render_plan(turned(0.0), 1.0).await;
    let px = |f: &klipolo_engine::session::RenderedRgba, (x, y): (u32, u32)| {
        pixel(&f.bytes, f.stride, x, y)
    };
    assert!(
        near(px(&upright, corner), blue, 12),
        "upright: {:?}",
        px(&upright, corner)
    );
    let f = render_plan(turned(45.0), 1.0).await;
    assert!(
        near(px(&f, corner), red, 12),
        "turned: {:?}",
        px(&f, corner)
    );
    assert!(near(px(&f, centre), blue, 12), "{:?}", px(&f, centre));
}

/// A half-width title keyed 1× → 3× in a 1080p export: its texture, laid out bigger, still fits whole
/// (no cut padding or box), and the drawn text stays centred on its box at rest and grown, where the texture's cap
/// on the box alone had shifted it right by its clamped width.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_wide_title_keyed_up_stays_centred_and_whole() {
    use klipolo_engine::plan::Layer;
    use klipolo_engine::render::text::TextDrawer;
    use klipolo_model::motion::{Key, Motion, MotionKeys};
    use klipolo_model::timeline::{ItemContent, TextStyle};
    let mut title = item(
        "t",
        0.0,
        3.0,
        bx(0.25, 0.4, 0.5, 0.1),
        ItemContent::Text {
            text: "MMMMMMMM".into(),
            style: TextStyle {
                size: 96.0,
                ..TextStyle::default()
            },
        },
    );
    let key = |s: f64, v: f64| Key {
        t_us: secs(s),
        v,
        ease: None,
    };
    title.layout.motion = Some(Motion {
        keys: Some(MotionKeys {
            scale: vec![key(0.0, 1.0), key(3.0, 3.0)],
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    p.timeline.tracks = vec![overlay_track("t", vec![title])];
    let c = ctx(p);
    let size = (1920, 1080);

    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let plan = frame_plan(&c, 0, FormatKind::Landscape);
    let Some(Layer::Text(t)) = plan.layers.iter().find(|l| matches!(l, Layer::Text(_))) else {
        panic!("the title draws");
    };
    let (texture, _, up) = drawer
        .prepare(
            &dev.device,
            &dev.queue,
            t,
            plan.canvas,
            size,
            &mut Vec::new(),
        )
        .unwrap();
    assert!(up >= 3.0, "laid out at least at its largest size: {up}");
    assert!(
        texture.width() < 4096,
        "whole, not clamped: {}",
        texture.width()
    );

    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    for (at, scale) in [(0.0, 1.0), (2.7, 2.8)] {
        let req = RenderRequest {
            plan: frame_plan(&c, secs(at), FormatKind::Landscape),
            width: size.0,
            height: size.1,
            output: Output::Rgba,
        };
        let f = backend.render(req).await.unwrap().finish().await.unwrap();
        let bright: Vec<u32> = (470..500)
            .step_by(2)
            .flat_map(|y| (0..size.0).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&f.bytes, f.stride, x, y)[0] > 200)
            .map(|(x, _)| x)
            .collect();
        let (left, right) = (bright.iter().min().unwrap(), bright.iter().max().unwrap());
        let centre = f64::from(left + right) / 2.0;
        assert!(
            (centre - 960.0).abs() <= 3.0 * scale,
            "at {at} s the text spans {left}–{right}, centred at {centre}, not 960"
        );
    }
}

/// A page's word atlas holds each word's masks in its own cell (R the fill, G the outlined silhouette, B
/// the silhouette blurred for the shadow), within the atlas budget. Prints the atlas bytes and build time for a Bold
/// page and a Clean two-line page at 1080 × 1920 (on lavapipe in CI: label them as such).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_pages_atlas_holds_each_words_masks_in_its_own_cell() {
    let atlas =
        klipolo_engine::render::captions::test_atlas("caption.bold", &["MAKE", "IT", "POP"]).await;
    assert_eq!(atlas.cells.len(), 3);
    for cell in &atlas.cells {
        let fill = atlas.channel_sum(*cell, 0);
        let silhouette = atlas.channel_sum(*cell, 1);
        assert!(
            fill > 0 && silhouette > fill,
            "the outline is outside the fill"
        );
        assert!(
            atlas.channel_sum(*cell, 2) > 0,
            "Bold's shadow is blurred into B"
        );
        assert_eq!(
            atlas.channel_sum(*cell, 3),
            0,
            "Bold has no glow: A stays empty"
        );
    }
    assert!(
        atlas.bytes <= 4 << 20,
        "{} bytes for three words",
        atlas.bytes
    );
    let clean = klipolo_engine::render::captions::test_atlas(
        "caption.clean",
        &[
            "So", "the", "budget", "has", "three", "columns", "and", "a", "total.",
        ],
    )
    .await;
    assert_eq!(clean.lines, 2, "a Clean page of nine words takes two lines");
    // Arcade's active word glows: its fill is blurred into A.
    let arcade =
        klipolo_engine::render::captions::test_atlas("caption.arcade", &["GG", "EZ"]).await;
    for cell in &arcade.cells {
        assert!(arcade.channel_sum(*cell, 3) > 0, "Arcade's glow is in A");
    }
    for (name, a) in [("Bold, 3 words", &atlas), ("Clean, 2 lines", &clean)] {
        eprintln!(
            "caption atlas ({name}, 1080x1920): {}x{} px, {} bytes, built in {:.1} ms ({:.1} ms the first time)",
            a.size.0, a.size.1, a.bytes, a.build_ms, a.cold_ms
        );
    }
}

/// One 640 × 360 frame of `c` at `at` s through `backend`.
async fn draw_at(
    backend: &mut GpuBackend,
    c: &PlanContext,
    at: f64,
) -> klipolo_engine::session::RenderedRgba {
    backend
        .render(RenderRequest {
            plan: frame_plan(c, secs(at), FormatKind::Landscape),
            width: 640,
            height: 360,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap()
}

/// White glyph pixels (not the red outline's) in the middle band of a 640 × 360 frame.
fn lit(f: &klipolo_engine::session::RenderedRgba) -> usize {
    (64..576)
        .step_by(2)
        .flat_map(|x| (150..210).step_by(2).map(move |y| (x, y)))
        .filter(|&(x, y)| pixel(&f.bytes, f.stride, x, y)[1] > 200)
        .count()
}

/// A title the playback lookahead reaches is built on the title build thread before its
/// first frame, so the render thread shapes and draws nothing as it enters. Once it has left, its texture stays 8
/// frames, so a scrub back across its end builds nothing either; after that it is gone, and a cold seek builds it on
/// the render thread again (a layout and a draw).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_entering_during_playback_was_built_ahead_and_is_kept_briefly() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    // The system font scan first: one that finished mid-test would draw every title again with the scanned fonts.
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    drop(TextDrawer::new(&dev.device, &dev.queue, true));
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    let mut content = text("Thirty characters in one title");
    if let ItemContent::Text { style, .. } = &mut content {
        style.outline = Some(klipolo_model::timeline::Outline {
            color: "#FF0000".into(),
            width: 4.0,
        });
    }
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item("t", 1.0, 1.0, bx(0.1, 0.4, 0.8, 0.2), content)],
    )];
    let c = ctx(p);
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap();
    let built = |b: &GpuBackend| b.text_stats().0.built_here;
    assert_eq!(lit(&draw_at(&mut backend, &c, 0.5).await), 0);
    // Playing from 0.5 s, the lookahead reaches the title's start, a second before it (`warm_ahead`).
    backend.warm(&frame_plan(&c, secs(1.0), FormatKind::Landscape));
    let started = std::time::Instant::now();
    let mut at = 0.5;
    while backend.text_stats().0.in_flight > 0 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the title is built ahead"
        );
        at = (at + 1.0 / 30.0f64).min(0.95);
        draw_at(&mut backend, &c, at).await;
    }
    let before = built(&backend);
    for k in 0..6 {
        let f = draw_at(&mut backend, &c, 1.0 + f64::from(k) / 30.0).await;
        assert!(lit(&f) > 20, "frame {k} of the title draws it");
    }
    assert_eq!(
        built(&backend) - before,
        0,
        "nothing shaped or drawn on the render thread as it entered"
    );
    // It leaves at 2 s; scrubbed back across its end within 8 frames.
    for at in [2.05, 2.1, 2.15] {
        assert_eq!(lit(&draw_at(&mut backend, &c, at).await), 0);
    }
    assert!(lit(&draw_at(&mut backend, &c, 1.95).await) > 20);
    assert_eq!(
        built(&backend) - before,
        0,
        "scrubbed back within 8 frames: its texture was kept"
    );
    for _ in 0..10 {
        draw_at(&mut backend, &c, 2.5).await;
    }
    assert!(lit(&draw_at(&mut backend, &c, 1.5).await) > 20);
    assert_eq!(
        built(&backend) - before,
        2,
        "gone 8 frames after it left: a cold seek lays it out and draws it here"
    );
}

/// Titles that left keep their textures 8 frames, and those count toward
/// `MAX_TEXT_BYTES`: ten full-frame 4K titles shown one after another, a frame each, would keep about 330 MB. The drawer
/// never holds more than the ceiling, the textures kept going oldest first: the title that just left is still kept,
/// and the first is built again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn titles_that_left_stay_briefly_within_the_text_ceiling_oldest_first() {
    use klipolo_engine::plan::{Layer, TextLayer};
    use klipolo_engine::render::text::{MAX_TEXT_BYTES, TextDrawer};
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let paragraph =
        "Every word of this paragraph fills the frame from one side to the other one. ".repeat(7);
    let mut p = red_background(project());
    p.timeline.tracks = (0..10)
        .map(|i| {
            let title = item(
                &format!("t{i}"),
                0.0,
                10.0,
                bx(0.0, 0.0, 1.0, 1.0),
                text(&paragraph),
            );
            overlay_track(&format!("x{i}"), vec![title])
        })
        .collect();
    let plan = frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape);
    let titles: Vec<TextLayer> = plan
        .layers
        .iter()
        .filter_map(|l| match l {
            Layer::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(titles.len(), 10);
    let size = (3840, 2160);
    let show = |drawer: &mut TextDrawer, t: &TextLayer| {
        drawer.in_frame(t);
        drawer.begin_frame();
        let drawn = drawer.prepare(
            &dev.device,
            &dev.queue,
            t,
            plan.canvas,
            size,
            &mut Vec::new(),
        );
        drawer.end_frame();
        assert!(drawn.is_some(), "{} draws", t.id);
    };
    let (mut most, mut most_bytes) = (0, 0);
    for t in &titles {
        show(&mut drawer, t);
        let held = drawer.stats();
        assert!(held.bytes <= MAX_TEXT_BYTES, "{} MB held", held.bytes >> 20);
        most = most.max(held.held);
        most_bytes = most_bytes.max(held.bytes);
    }
    println!(
        "most titles held: {most}; most bytes {} MB",
        most_bytes >> 20
    );
    assert!(most >= 2, "titles that left are kept a while: {most} held");
    let before = drawer.stats().built_here;
    show(&mut drawer, &titles[8]);
    assert_eq!(
        drawer.stats().built_here,
        before,
        "the title that just left is kept"
    );
    show(&mut drawer, &titles[0]);
    assert_eq!(
        drawer.stats().built_here,
        before + 2,
        "the oldest went first: laid out and drawn again"
    );
}

/// The one text layer of `p`'s frame at 1 s.
fn the_title(p: Project) -> (klipolo_engine::plan::TextLayer, (u32, u32)) {
    let plan = frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape);
    let title = plan
        .layers
        .iter()
        .find_map(|l| match l {
            klipolo_engine::plan::Layer::Text(t) => Some(t.clone()),
            _ => None,
        })
        .expect("a title");
    (title, plan.canvas)
}

/// A title of `words` in `rect`, `size` px at 1080, with `outline` px of red outline (none at 0).
fn titled(words: &str, rect: klipolo_model::layout::BoxN, size: f64, outline: f64) -> Project {
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    let style = klipolo_model::timeline::TextStyle {
        size,
        outline: (outline > 0.0).then(|| klipolo_model::timeline::Outline {
            color: "#FF0000".into(),
            width: outline,
        }),
        ..klipolo_model::timeline::TextStyle::default()
    };
    let content = ItemContent::Text {
        text: words.into(),
        style,
    };
    p.timeline.tracks = vec![overlay_track("t", vec![item("t", 0.0, 2.0, rect, content)])];
    p
}

/// An outline is one glyph a letter (the letter grown by it), not up to ~90 stamped
/// copies: a shadowed title with a 12 px outline draws three glyph areas (the shadow, the outline, the fill), where
/// the stamps drew 1 + 2 × 80. Its outline reaches 12 px past the letter on both sides of a stem.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn an_outline_is_one_glyph_a_letter_not_stamped_copies() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let p = titled("I", bx(0.4, 0.4, 0.2, 0.2), 72.0, 12.0);
    let (title, canvas) = the_title(p.clone());
    assert!(title.style.shadow, "a shadowed title");
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    drawer
        .prepare(
            &dev.device,
            &dev.queue,
            &title,
            canvas,
            (1920, 1080),
            &mut Vec::new(),
        )
        .expect("drawn");
    assert_eq!(
        drawer.stats().glyph_areas,
        3,
        "the shadow, the outline and the fill: one glyph area each"
    );
    // Through the middle of the stem (the shadow is black on black): red, white, red.
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let f = backend
        .render(RenderRequest {
            plan: frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape),
            width: 1920,
            height: 1080,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    let row: Vec<[u8; 3]> = (700..1220)
        .map(|x| pixel(&f.bytes, f.stride, x, 540))
        .collect();
    let white: Vec<usize> = (0..row.len()).filter(|&i| row[i][1] > 128).collect();
    let red = |i: &usize| row[*i][0] > 128 && row[*i][1] <= 128;
    let (first, last) = (white[0], *white.last().unwrap());
    let left = (0..first).filter(red).count();
    let right = (last + 1..row.len()).filter(red).count();
    println!(
        "stem {} px, outline {left} px left and {right} px right",
        last + 1 - first
    );
    for side in [left, right] {
        assert!(
            (10..=14).contains(&side),
            "a 12 px outline: {left} and {right} px"
        );
    }
}

/// A title's texture holds its ink and the room round it, not its whole box: a short
/// word in a full-width box gets a texture a little wider than the word, placed where the box centres it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_short_title_in_a_wide_box_gets_a_texture_of_its_ink() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let (title, canvas) = the_title(titled("Hi", bx(0.0, 0.3, 1.0, 0.4), 72.0, 0.0));
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let (texture, quad, _) = drawer
        .prepare(
            &dev.device,
            &dev.queue,
            &title,
            canvas,
            (1920, 1080),
            &mut Vec::new(),
        )
        .expect("drawn");
    // "Hi" at 72 px is about 70 px wide; its box 1920 × 432.
    assert!(
        texture.width() < 300 && texture.height() < 250,
        "{} × {}",
        texture.width(),
        texture.height()
    );
    assert!(
        (quad.center.x - 960.0).abs() < 4.0 && (quad.center.y - 540.0).abs() < 8.0,
        "centred on its box: {:?}",
        (quad.center.x, quad.center.y)
    );
}

/// The title glyph atlas can't only grow: after a title whose glyphs need more than a
/// 2048² atlas holds (36 letters at 600 px, at 4K), it is made anew; a small title after it leaves it be.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_glyph_atlas_is_made_anew_after_a_title_past_its_cap() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let size = (3840, 2160);
    let (big, canvas) = the_title(titled(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        bx(0.0, 0.0, 1.0, 1.0),
        300.0,
        0.0,
    ));
    let (small, _) = the_title(titled("Hi", bx(0.3, 0.4, 0.4, 0.2), 72.0, 0.0));
    let before = drawer.stats().atlas_resets;
    drawer
        .prepare(&dev.device, &dev.queue, &big, canvas, size, &mut Vec::new())
        .expect("the big title");
    assert_eq!(
        drawer.stats().atlas_resets,
        before + 1,
        "made anew after the big title"
    );
    drawer.end_frame();
    drawer
        .prepare(
            &dev.device,
            &dev.queue,
            &small,
            canvas,
            size,
            &mut Vec::new(),
        )
        .expect("the small title");
    assert_eq!(drawer.stats().atlas_resets, before + 1, "not again");
}

/// A black project whose one title (`words`, `color`) shows from 1 s to 2 s in the middle band.
fn coming_title(words: &str, color: &str) -> PlanContext {
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    let mut content = text(words);
    if let ItemContent::Text { style, .. } = &mut content {
        style.color = color.into();
        style.shadow = false;
    }
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item("t", 1.0, 1.0, bx(0.1, 0.4, 0.8, 0.2), content)],
    )];
    ctx(p)
}

/// Pixels of `f`'s middle band whose colour is near `rgb`.
fn painted(f: &klipolo_engine::session::RenderedRgba, rgb: [u8; 3]) -> usize {
    (64..576)
        .step_by(2)
        .flat_map(|x| (150..210).step_by(2).map(move |y| (x, y)))
        .filter(|&(x, y)| near(pixel(&f.bytes, f.stride, x, y), rgb, 40))
        .count()
}

/// A preview backend whose system font scan is done (one finishing mid-test would draw every title again).
async fn scanned_preview() -> GpuBackend {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    drop(TextDrawer::new(&dev.device, &dev.queue, true));
    GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
}

/// A title asked ahead and drawn at once, before its build lands, is waited for
/// (or kept as it lands), never built a second time on the render thread, and drawn.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_asked_ahead_and_drawn_at_once_is_not_built_twice() {
    let c = coming_title("Waited for", "#FFFFFF");
    let mut backend = scanned_preview().await;
    draw_at(&mut backend, &c, 0.5).await;
    let before = backend.text_stats().0.built_here;
    backend.warm(&frame_plan(&c, secs(1.0), FormatKind::Landscape));
    let f = draw_at(&mut backend, &c, 1.0).await;
    assert!(painted(&f, [255, 255, 255]) > 20, "the title draws");
    assert_eq!(
        backend.text_stats().0.built_here - before,
        0,
        "nothing built on the render thread"
    );
}

/// A title edited while its build is in flight (its colour) is asked again,
/// and the frame draws the edit from the second build: no stale texture, and nothing built on the render thread.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_edited_while_its_build_is_in_flight_draws_the_edit() {
    let white = coming_title("Edited ahead", "#FFFFFF");
    let green = coming_title("Edited ahead", "#00FF00");
    let mut backend = scanned_preview().await;
    draw_at(&mut backend, &white, 0.5).await;
    let before = backend.text_stats().0.built_here;
    backend.warm(&frame_plan(&white, secs(1.0), FormatKind::Landscape));
    backend.warm(&frame_plan(&green, secs(1.0), FormatKind::Landscape));
    let f = draw_at(&mut backend, &green, 1.0).await;
    assert!(painted(&f, [0, 255, 0]) > 20, "the edit draws");
    assert_eq!(painted(&f, [255, 255, 255]), 0, "never the stale texture");
    assert_eq!(
        backend.text_stats().0.built_here - before,
        0,
        "the edit was built ahead too"
    );
}

/// A build asked before the fonts changed is dropped when it lands, so the title
/// is laid out and drawn again with the new fonts.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_build_asked_before_a_font_change_is_dropped() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let (title, canvas) = the_title(titled("Fonts changed", bx(0.1, 0.4, 0.8, 0.2), 72.0, 0.0));
    let out = (640, 360);
    drawer.ahead(&dev, &title, canvas, out);
    assert_eq!(drawer.stats().in_flight, 1);
    drawer.fonts_changed();
    // Frames go on while the build lands (dropped as it does).
    for _ in 0..100 {
        drawer.begin_frame();
        drawer.end_frame();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let before = drawer.stats().built_here;
    drawer.in_frame(&title);
    drawer.begin_frame();
    assert!(
        drawer
            .prepare(
                &dev.device,
                &dev.queue,
                &title,
                canvas,
                out,
                &mut Vec::new()
            )
            .is_some()
    );
    assert_eq!(
        drawer.stats().built_here - before,
        2,
        "laid out and drawn here: the build from before the change was dropped"
    );
}

/// Builds asked ahead never ask for more than the text ceiling between them:
/// four huge 4K titles asked at once ask for at most `MAX_TEXT_BYTES` (each sized to the room the others left).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn builds_asked_ahead_stay_within_the_text_ceiling() {
    use klipolo_engine::plan::Layer;
    use klipolo_engine::render::text::{MAX_TEXT_BYTES, TextDrawer};
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let mut p = red_background(project());
    p.timeline.tracks = (0..4)
        .map(|i| {
            let letters = ItemContent::Text {
                text: "A".repeat(300),
                style: klipolo_model::timeline::TextStyle {
                    size: 400.0,
                    ..klipolo_model::timeline::TextStyle::default()
                },
            };
            let huge = item(
                &format!("t{i}"),
                0.0,
                2.0,
                bx(-1.5, -1.5, 4.0, 4.0),
                letters,
            );
            overlay_track(&format!("x{i}"), vec![huge])
        })
        .collect();
    let plan = frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape);
    for layer in &plan.layers {
        if let Layer::Text(t) = layer {
            drawer.ahead(&dev, t, plan.canvas, (3840, 2160));
        }
    }
    let asked = drawer.stats();
    println!(
        "{} builds asked, {} MB between them",
        asked.in_flight,
        asked.in_flight_bytes >> 20
    );
    assert!(asked.in_flight >= 1);
    assert!(
        asked.in_flight_bytes <= MAX_TEXT_BYTES,
        "{} MB asked",
        asked.in_flight_bytes >> 20
    );
}

/// A frame's text drawers report their own time. A frame with no
/// title draws none; one building and drawing a title costs the drawer CPU, and its GPU time where the device has
/// timestamps (lavapipe has them: there it is never n/a); a steady frame of the same title costs less CPU.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn the_text_drawers_own_time_is_measured_per_frame() {
    klipolo_engine::render::text_timing::ask_for_timestamps();
    let c = coming_title("Timed on its own", "#FFFFFF");
    let mut backend = scanned_preview().await;
    draw_at(&mut backend, &c, 0.5).await;
    backend.time_text(true);
    draw_at(&mut backend, &c, 0.5).await;
    let none = backend.text_time();
    assert!(!none.drew, "no title, nothing drawn: {none:?}");
    let f = draw_at(&mut backend, &c, 1.0).await;
    assert!(painted(&f, [255, 255, 255]) > 20, "the title draws");
    let built = backend.text_time();
    draw_at(&mut backend, &c, 1.1).await;
    let steady = backend.text_time();
    println!(
        "{}: built {built:?}, steady {steady:?}",
        backend.adapter_name()
    );
    assert!(built.drew && steady.drew);
    assert!(built.cpu_us > 0.0 && steady.cpu_us > 0.0);
    assert!(
        steady.cpu_us < built.cpu_us,
        "a steady frame lays nothing out"
    );
    if backend.adapter_name().contains("llvmpipe") {
        let (built, steady) = (built.gpu_ms.unwrap(), steady.gpu_ms.unwrap());
        assert!(built > 0.0 && steady > 0.0, "{built} ms, {steady} ms");
    }
    backend.time_text(false);
    draw_at(&mut backend, &c, 1.2).await;
    assert_eq!(
        backend.text_time(),
        Default::default(),
        "off: nothing timed"
    );
}

/// Huge 4K titles (300 letters at 400 px in boxes of 4 × 4 canvases), `count` of them on tracks of their own: each
/// build asked ahead takes most of the text ceiling.
fn huge_titles(count: usize) -> (Vec<klipolo_engine::plan::TextLayer>, (u32, u32)) {
    use klipolo_engine::plan::Layer;
    let mut p = red_background(project());
    p.timeline.tracks = (0..count)
        .map(|i| {
            let letters = ItemContent::Text {
                text: "A".repeat(300),
                style: klipolo_model::timeline::TextStyle {
                    size: 400.0,
                    ..klipolo_model::timeline::TextStyle::default()
                },
            };
            let huge = item(
                &format!("t{i}"),
                0.0,
                2.0,
                bx(-1.5, -1.5, 4.0, 4.0),
                letters,
            );
            overlay_track(&format!("x{i}"), vec![huge])
        })
        .collect();
    let plan = frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape);
    let titles = plan
        .layers
        .iter()
        .filter_map(|l| match l {
            Layer::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    (titles, plan.canvas)
}

/// Frames of a drawer that draws nothing until every build asked has landed (30 s at most).
fn until_landed(drawer: &mut klipolo_engine::render::text::TextDrawer) {
    let started = std::time::Instant::now();
    while drawer.stats().in_flight > 0 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the builds land"
        );
        drawer.begin_frame();
        drawer.end_frame();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// A build dropped while the title build thread makes it (the fonts changed, or
/// a frame stopped waiting for it) still holds its texture's bytes until it lands, so they stay counted toward the text
/// ceiling; when it lands it is let go, and nothing is held.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_dropped_build_counts_until_it_lands_then_releases_its_bytes() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let (title, canvas) = the_title(titled(
        "Dropped while it builds",
        bx(0.1, 0.4, 0.8, 0.2),
        72.0,
        4.0,
    ));
    drawer.ahead(&dev, &title, canvas, (1920, 1080));
    let asked = drawer.stats().in_flight_bytes;
    assert!(asked > 0);
    drawer.fonts_changed();
    assert_eq!(
        drawer.stats().in_flight_bytes,
        asked,
        "still being made: still counted"
    );
    until_landed(&mut drawer);
    let after = drawer.stats();
    assert_eq!(
        (after.in_flight_bytes, after.bytes, after.held),
        (0, 0, 0),
        "landed and let go"
    );
}

/// A title built ahead and not shown yet holds its texture, so the next title
/// asked ahead is sized to the room it leaves: what the drawer holds and what it has asked for stay within the text
/// ceiling together.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_built_ahead_and_not_shown_counts_toward_the_next_ask() {
    use klipolo_engine::render::text::{MAX_TEXT_BYTES, TextDrawer};
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let (titles, canvas) = huge_titles(2);
    let out = (3840, 2160);
    drawer.ahead(&dev, &titles[0], canvas, out);
    until_landed(&mut drawer);
    let held = drawer.stats().bytes;
    assert!(held > 0, "the first title is held, not shown yet");
    drawer.ahead(&dev, &titles[1], canvas, out);
    let asked = drawer.stats().in_flight_bytes;
    println!("held {} MB, then asked {} MB", held >> 20, asked >> 20);
    assert!(
        held + asked <= MAX_TEXT_BYTES,
        "held {} MB + asked {} MB",
        held >> 20,
        asked >> 20
    );
    until_landed(&mut drawer);
    assert!(drawer.stats().bytes <= MAX_TEXT_BYTES);
}

/// A black 4K project: three full-frame white paragraphs from 0 to 3 s (about 84 MB of the 96 MiB text ceiling between
/// them) and a short green title in a full-frame box from 1 s to 2 s (its box's estimate at full resolution, about
/// 37 MB, is past the room they leave; its ink isn't).
fn crowded_titles() -> PlanContext {
    let paragraph =
        "Every word of this paragraph fills the frame from one side to the other one. ".repeat(6);
    let mut p = red_background(project());
    p.background.color = "#000000".into();
    let mut tracks: Vec<_> = (0..3)
        .map(|i| {
            let mut content = text(&paragraph);
            if let ItemContent::Text { style, .. } = &mut content {
                style.shadow = false;
            }
            overlay_track(
                &format!("p{i}"),
                vec![item(
                    &format!("p{i}"),
                    0.0,
                    3.0,
                    bx(0.0, 0.0, 1.0, 1.0),
                    content,
                )],
            )
        })
        .collect();
    let mut short = text("Sharp");
    if let ItemContent::Text { style, .. } = &mut short {
        style.color = "#00FF00".into();
        style.shadow = false;
    }
    tracks.push(overlay_track(
        "t",
        vec![item("t", 1.0, 1.0, bx(0.0, 0.0, 1.0, 1.0), short)],
    ));
    p.timeline.tracks = tracks;
    ctx(p)
}

/// One 4K frame of `c` at `at` s through `backend`.
async fn draw_4k(
    backend: &mut GpuBackend,
    c: &PlanContext,
    at: f64,
) -> klipolo_engine::session::RenderedRgba {
    backend
        .render(RenderRequest {
            plan: frame_plan(c, secs(at), FormatKind::Landscape),
            width: 3840,
            height: 2160,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap()
}

/// A title built ahead enters exactly as sharp as one built cold. With most of
/// the text ceiling held by the frame's other titles, the build ahead used to be sized by its box's estimate (half
/// resolution, and the softer note) where the cold build tries full resolution and fits its ink: the two entrance
/// frames are now the same, pixel for pixel, and neither says it drew softer.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_built_ahead_enters_as_sharp_as_one_built_cold() {
    use klipolo_engine::render::text::TEXT_OVER_BUDGET;
    let c = crowded_titles();
    let mut ahead = scanned_preview().await;
    draw_4k(&mut ahead, &c, 0.5).await;
    ahead.warm(&frame_plan(&c, secs(1.0), FormatKind::Landscape).titles_only());
    let started = std::time::Instant::now();
    let mut at = 0.5;
    while ahead.text_stats().0.in_flight > 0 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the title is built ahead"
        );
        at = (at + 1.0 / 30.0f64).min(0.95);
        draw_4k(&mut ahead, &c, at).await;
    }
    let before = ahead.text_stats().0.built_here;
    let built_ahead = draw_4k(&mut ahead, &c, 1.0).await;
    assert_eq!(
        ahead.text_stats().0.built_here,
        before,
        "built ahead, not on the render thread"
    );
    let mut cold = scanned_preview().await;
    draw_4k(&mut cold, &c, 0.5).await;
    let built_cold = draw_4k(&mut cold, &c, 1.0).await;
    let softer = |f: &klipolo_engine::session::RenderedRgba| {
        f.warnings.iter().any(|w| w == TEXT_OVER_BUDGET)
    };
    assert!(!softer(&built_cold), "the cold build is sharp");
    let green = painted_4k(&built_cold, [0, 255, 0]);
    assert!(green > 100, "the title draws: {green} green pixels");
    let differ = built_ahead
        .bytes
        .chunks_exact(4)
        .zip(built_cold.bytes.chunks_exact(4))
        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 2))
        .count();
    println!(
        "{differ} pixels differ; built ahead says softer: {}",
        softer(&built_ahead)
    );
    assert!(!softer(&built_ahead), "built ahead: never softer");
    assert_eq!(differ, 0, "the same entrance frame");
}

/// Green pixels (near `rgb`) of a 4K frame, every fourth pixel each way.
fn painted_4k(f: &klipolo_engine::session::RenderedRgba, rgb: [u8; 3]) -> usize {
    (0..3840)
        .step_by(4)
        .flat_map(|x| (0..2160).step_by(4).map(move |y| (x, y)))
        .filter(|&(x, y)| near(pixel(&f.bytes, f.stride, x, y), rgb, 40))
        .count()
}

/// The cold path: a title built cold on the render thread (a seek or an edit straight into it)
/// strokes each letter's outline once. Its shadow's outline glyphs are the outline's own (it moves a whole pixel
/// across), so a shadowed title makes no more outline glyphs than an unshadowed one; its glyph atlas growing from 256²
/// as the title's glyphs arrive asks for every glyph again, which a kept image answers without a stroke; and a space,
/// which has nothing to stroke, is tried once.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_cold_outlined_title_strokes_each_letter_once() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut counts = Vec::new();
    for shadow in [true, false] {
        let mut p = titled(
            "Thirty characters in one title",
            bx(0.1, 0.4, 0.8, 0.2),
            96.0,
            4.0,
        );
        if let ItemContent::Text { style, .. } = &mut p.timeline.tracks[0].items[0].content {
            style.shadow = shadow;
        }
        let (title, canvas) = the_title(p);
        let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
        drawer
            .prepare(
                &dev.device,
                &dev.queue,
                &title,
                canvas,
                (1920, 1080),
                &mut Vec::new(),
            )
            .expect("drawn");
        let stats = drawer.stats();
        println!(
            "shadow {shadow}: {} outline glyphs, {} strokes",
            stats.outline_glyphs, stats.outline_strokes
        );
        counts.push((stats.outline_glyphs, stats.outline_strokes));
    }
    let [(shadowed, strokes), (plain, _)] = counts[..] else {
        unreachable!()
    };
    assert!(plain > 10, "{plain} outline glyphs");
    assert_eq!(
        shadowed, plain,
        "the shadow's outline glyphs are the outline's own"
    );
    assert_eq!(
        strokes, shadowed as u64,
        "each stroked once, though the atlas grew"
    );
}

/// A title built ahead is touched once, on the frame its build lands: its texture drawn a single
/// time, one pixel at zero opacity, so the GPU has it resident and sampled through the quad's pipeline a second before
/// the title enters (Metal's first use of a 4K outlined title cost its entrance frame 44 ms). The frame it lands on is
/// unchanged and has no pass of its own, and no later frame (its entrance included) touches it again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_built_ahead_is_touched_once_before_it_enters() {
    let c = coming_title("Touched once", "#FFFFFF");
    let mut backend = scanned_preview().await;
    let still = draw_at(&mut backend, &c, 0.6).await;
    let passes = backend.passes_last_frame();
    backend.warm(&frame_plan(&c, secs(1.0), FormatKind::Landscape).titles_only());
    let started = std::time::Instant::now();
    let landed = loop {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the title is built ahead"
        );
        let f = draw_at(&mut backend, &c, 0.6).await;
        let stats = backend.text_stats().0;
        if stats.in_flight == 0 {
            break f;
        }
        assert_eq!(stats.touched, 0, "nothing to touch before it lands");
    };
    assert_eq!(
        backend.text_stats().0.touched,
        1,
        "touched on the frame it landed"
    );
    assert!(
        landed.bytes[..] == still.bytes[..],
        "one pixel at zero opacity: the frame is unchanged"
    );
    assert_eq!(backend.passes_last_frame(), passes, "no pass of its own");
    let built = backend.text_stats().0.built_here;
    for at in [0.7, 0.9, 1.0, 1.05] {
        draw_at(&mut backend, &c, at).await;
    }
    let after = backend.text_stats().0;
    assert_eq!(after.touched, 1, "once per built texture");
    assert_eq!(after.built_here, built, "it entered as built ahead");
}

/// An image quad (titles, images, overlay video) is drawn as two triangles over its own bounds, not one
/// over the whole output. Turned 45°, a square's corners reach past its unturned box and are drawn whole; past them,
/// and everywhere else, the frame is as it was. The square sits off the frame's centre, in its top right, and turned
/// it reaches past the frame's top and right edges: a bound mapped the wrong way round (a
/// lost flip, a mirrored or shifted box) or clamped wrongly to the frame draws it elsewhere, or cut.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_turned_quad_is_drawn_to_its_corners_and_nowhere_else() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = square_over("#000000", [255, 255, 255], dir.path());
    // 192 × 192 px at 640 × 360, centred on (520, 120).
    p.timeline.tracks[0].items[0].layout.rect = bx(0.6625, 0.0666667, 0.3, 0.5333333);
    let flat = render_plan(p.clone(), 1.0).await;
    p.timeline.tracks[0].items[0].layout.rotation = 45.0;
    let turned = render_plan(p, 1.0).await;
    let lit = |f: &klipolo_engine::session::RenderedRgba, x: u32, y: u32| {
        pixel(&f.bytes, f.stride, x, y)[0] > 128
    };
    // Its corners reach 136 px from its centre on each axis, past its unturned box's 96: the left and bottom ones
    // within the frame, the top and right ones past its edges (drawn up to them).
    for (x, y) in [(395, 120), (520, 245), (520, 2), (636, 120)] {
        assert!(!lit(&flat, x, y), "unturned: ({x}, {y}) is past its box");
        assert!(lit(&turned, x, y), "turned: ({x}, {y}) is inside a corner");
    }
    // Inside its unturned box, past its turned edges (|dx| + |dy| > 136).
    for (x, y) in [(600, 200), (440, 40), (600, 40), (440, 200)] {
        assert!(lit(&flat, x, y), "unturned: ({x}, {y}) is inside");
        assert!(!lit(&turned, x, y), "turned: ({x}, {y}) is past its edge");
    }
    // Where a bound mirrored about the frame's middle, or its centre, would draw it: nothing.
    for (x, y) in [(520, 300), (120, 120), (120, 240), (320, 180)] {
        assert!(
            !lit(&flat, x, y) && !lit(&turned, x, y),
            "({x}, {y}) is never drawn"
        );
    }
    let drawn = |f: &klipolo_engine::session::RenderedRgba| {
        f.bytes.chunks_exact(4).filter(|p| p[0] > 128).count()
    };
    // Unturned, it is whole in the frame: 192² px, give or take its antialiased edge. Turned, its top and right corners
    // pass the frame's edges by 15.8 px each, cutting two right triangles of 15.8² px: about
    // 192² − 2 × 250 px.
    let (flat, turned) = (drawn(&flat), drawn(&turned));
    assert!(flat.abs_diff(192 * 192) < 800, "{flat} px");
    let clipped = 192 * 192 - 2 * 250;
    assert!(
        turned.abs_diff(clipped) < 1600,
        "{turned} px, about {clipped}"
    );
}

/// Two titles at once (ids `t` and `u`): `words` in a full-frame box (its box's estimate is a whole 4K frame, its ink
/// a line), at 72 px.
fn two_titles(t: &str, u: &str) -> (Vec<klipolo_engine::plan::TextLayer>, (u32, u32)) {
    use klipolo_engine::plan::Layer;
    let mut p = red_background(project());
    p.timeline.tracks = [("t", t), ("u", u)]
        .iter()
        .filter(|(_, words)| !words.is_empty())
        .map(|(id, words)| {
            let mut content = text(words);
            if let ItemContent::Text { style, .. } = &mut content {
                style.shadow = false;
            }
            overlay_track(
                &format!("x{id}"),
                vec![item(id, 0.0, 2.0, bx(0.0, 0.0, 1.0, 1.0), content)],
            )
        })
        .collect();
    let plan = frame_plan(&ctx(p), secs(1.0), FormatKind::Landscape);
    let titles = plan
        .layers
        .iter()
        .filter_map(|l| match l {
            Layer::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    (titles, plan.canvas)
}

/// One frame of a bare drawer drawing `titles` at 4K: each one's resolution (texture pixels per box pixel, None when
/// left out), and whether the frame said it drew softer.
fn draw_frame(
    dev: &klipolo_engine::render::device::RenderDevice,
    drawer: &mut klipolo_engine::render::text::TextDrawer,
    titles: &[klipolo_engine::plan::TextLayer],
    canvas: (u32, u32),
) -> (Vec<Option<f64>>, bool) {
    use klipolo_engine::render::text::TEXT_OVER_BUDGET;
    for t in titles {
        drawer.in_frame(t);
    }
    drawer.begin_frame();
    let mut warnings = Vec::new();
    let drawn = titles
        .iter()
        .map(|t| {
            drawer
                .prepare(
                    &dev.device,
                    &dev.queue,
                    t,
                    canvas,
                    (3840, 2160),
                    &mut warnings,
                )
                .map(|(_, _, scale)| scale)
        })
        .collect();
    drawer.end_frame();
    (drawn, warnings.iter().any(|w| w == TEXT_OVER_BUDGET))
}

/// Builds asked ahead never soften a title on screen, nor leave out one entering: they
/// are for titles still to come, land within milliseconds, and keep only what fits once they do. Two huge builds in
/// flight between them claim the whole text ceiling; meanwhile the title on screen is edited (a new texture) and
/// another enters, and both draw at full resolution, unsoftened.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn builds_in_flight_never_soften_a_title_on_screen() {
    use klipolo_engine::render::text::{MAX_TEXT_BYTES, TextDrawer};
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let (shown, canvas) = two_titles("On screen", "");
    let (drawn, softer) = draw_frame(&dev, &mut drawer, &shown, canvas);
    assert_eq!((drawn, softer), (vec![Some(1.0)], false), "sharp alone");
    let (huge, huge_canvas) = huge_titles(2);
    for t in &huge {
        drawer.ahead(&dev, t, huge_canvas, (3840, 2160));
    }
    let asked = drawer.stats().in_flight_bytes;
    println!("{} MB asked ahead", asked >> 20);
    assert!(
        asked + (8 << 20) > MAX_TEXT_BYTES,
        "the builds ahead claim the ceiling: {} MB",
        asked >> 20
    );
    let (edited, canvas) = two_titles("On screen, edited", "Entering now");
    let (drawn, softer) = draw_frame(&dev, &mut drawer, &edited, canvas);
    assert_eq!(drawer.stats().in_flight, 2, "both still in flight");
    assert_eq!(
        (drawn, softer),
        (vec![Some(1.0), Some(1.0)], false),
        "the edited title and the one entering draw sharp"
    );
    until_landed(&mut drawer);
    assert!(drawer.stats().bytes <= MAX_TEXT_BYTES);
}

/// A frame that stops waiting for a title's build ahead (250 ms) builds it itself and
/// drops the late build, which still holds its texture's bytes on the title build thread, so they stay counted until
/// it lands; then it is let go, and only the title the frame built is held. The drawer doesn't wait here at all, so
/// the late path runs however fast the machine builds.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_build_a_frame_stopped_waiting_for_counts_until_it_lands() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    drawer.wait_at_most(std::time::Duration::ZERO);
    let out = (3840, 2160);
    let (title, canvas) = two_titles("", "Waited for, then built here");
    let title = &title[0];
    drawer.ahead(&dev, title, canvas, out);
    assert_eq!(drawer.stats().in_flight, 1);
    let before = drawer.stats().built_here;
    drawer.in_frame(title);
    drawer.begin_frame();
    assert!(
        drawer
            .prepare(&dev.device, &dev.queue, title, canvas, out, &mut Vec::new())
            .is_some(),
        "drawn"
    );
    let waited = drawer.stats();
    assert_eq!(
        waited.built_here - before,
        2,
        "the wait ran out: laid out and drawn here"
    );
    assert_eq!(waited.in_flight, 1, "the late build is still in flight");
    assert!(waited.in_flight_bytes > 0, "and still counted");
    drawer.end_frame();
    // The title stays on screen while the builds land.
    let started = std::time::Instant::now();
    while drawer.stats().in_flight > 0 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the builds land"
        );
        drawer.in_frame(title);
        drawer.begin_frame();
        drawer
            .prepare(&dev.device, &dev.queue, title, canvas, out, &mut Vec::new())
            .expect("drawn");
        drawer.end_frame();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let landed = drawer.stats();
    assert_eq!(landed.in_flight_bytes, 0, "every byte asked released");
    assert_eq!(
        landed.built_here, waited.built_here,
        "the texture built here is the one kept: the late build was let go"
    );
    // Only the title's own texture: the dropped build was never kept, so never touched.
    assert_eq!((landed.held, landed.touched), (1, 0), "{landed:?}");
}

/// A steady caption frame uploads nothing. Its quads and uniform are what the last frame uploaded, so it
/// skips the queue's writes, each a staging buffer of its own: on Metal the caption drawer's main render-thread cost
/// (about 300 µs a frame on Iris Plus, against 15 for a title's steady frame). A frame whose words moved uploads its
/// quads only; the uniform (the output size, no line boxes in Highlight) stays.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_steady_caption_frame_uploads_nothing() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    d.draw_page("caption.highlight", 3).await;
    assert!(d.preview_at("caption.highlight", 3, 0.2), "drawn");
    let before = d.uploads();
    assert!(d.preview_at("caption.highlight", 3, 0.2));
    assert_eq!(
        d.uploads() - before,
        0,
        "the same frame again uploads nothing"
    );
    assert!(
        d.preview_at("caption.highlight", 3, 0.8),
        "another word said"
    );
    assert_eq!(
        d.uploads() - before,
        1,
        "its quads moved: they upload, the uniform doesn't"
    );
}

/// A caption frame whose words moved writes its quads into a kept staging buffer, mapped again once the
/// GPU is done with it, and copies them in its own encoder: no staging buffer of wgpu's own (`queue.write_buffer`
/// makes, maps and frees one each time). Frames alternate between two moments of a page, every one uploading.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn moving_caption_frames_upload_through_kept_staging_buffers() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    d.draw_page("caption.highlight", 3).await;
    assert!(d.preview_at("caption.highlight", 3, 0.2));
    d.settle();
    let (uploads, writes) = (d.uploads(), d.queue_writes());
    for k in 0..12 {
        let share = if k % 2 == 0 { 0.8 } else { 0.2 };
        assert!(d.preview_at("caption.highlight", 3, share));
        d.settle();
    }
    assert_eq!(d.uploads() - uploads, 12, "every frame's quads moved");
    assert_eq!(
        d.queue_writes() - writes,
        0,
        "all through kept staging buffers"
    );
}

/// Moving caption frames drawn with the GPU's work still in flight (nothing
/// waited for between them) count every upload once, through a kept staging buffer or, with all of them in flight, a
/// queue write; once the GPU catches up, uploads go through the kept buffers again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn moving_caption_frames_in_flight_count_every_upload_once() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    d.draw_page("caption.highlight", 3).await;
    assert!(d.preview_at("caption.highlight", 3, 0.2));
    d.settle();
    let (uploads, writes) = (d.uploads(), d.queue_writes());
    for k in 0..12 {
        let share = if k % 2 == 0 { 0.8 } else { 0.2 };
        assert!(d.preview_at("caption.highlight", 3, share));
    }
    assert_eq!(d.uploads() - uploads, 12, "every frame's quads, once each");
    println!(
        "{} of 12 moving frames in flight fell back to a queue write",
        d.queue_writes() - writes
    );
    d.settle();
    let writes = d.queue_writes();
    assert!(d.preview_at("caption.highlight", 3, 0.8));
    assert_eq!(
        d.queue_writes() - writes,
        0,
        "the GPU caught up: through a kept staging buffer again"
    );
}

/// With every kept staging buffer in flight (here never mapped again, as the
/// export's frame path left them), a moving frame's quads fall back to a queue write, counted once; mapped
/// again, the kept buffers take the uploads again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn with_every_staging_buffer_in_flight_quads_upload_with_a_queue_write() {
    let mut d = klipolo_engine::test_support::caption_drawer().await;
    d.draw_page("caption.highlight", 3).await;
    assert!(d.preview_at("caption.highlight", 3, 0.2));
    d.settle();
    let (uploads, writes) = (d.uploads(), d.queue_writes());
    d.hold_staging(true);
    for k in 0..5 {
        let share = if k % 2 == 0 { 0.8 } else { 0.2 };
        assert!(d.preview_at("caption.highlight", 3, share));
    }
    assert_eq!(d.uploads() - uploads, 5, "every frame's quads moved");
    assert_eq!(
        d.queue_writes() - writes,
        2,
        "three through the kept staging buffers, then queue writes"
    );
    d.hold_staging(false);
    d.settle();
    let writes = d.queue_writes();
    assert!(d.preview_at("caption.highlight", 3, 0.2));
    assert_eq!(
        d.queue_writes() - writes,
        0,
        "mapped again: through a kept staging buffer"
    );
}

/// Caption words for the export-path and pixel tests: the golden sample's.
const CAPTION_WORDS: [(&str, f64, f64); 4] = [
    ("Make", 1.0, 1.3),
    ("$500", 1.35, 1.7),
    ("with", 1.75, 1.95),
    ("this", 2.0, 2.3),
];

/// An export's moving caption frames copy their quads from the kept staging
/// buffers too. Its frame path (`render_formats`, `Compositor::render_yuv`) maps them again after each submit as the
/// preview's does; it didn't, so from the fourth moving frame on every upload was a queue write again (a staging buffer
/// of wgpu's own each time). Twelve frames, the highlight moving between two words.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn an_exports_moving_caption_frames_upload_through_kept_staging_buffers() {
    klipolo_engine::render::fonts::configure_for_tests();
    let times: Vec<f64> = (0..12)
        .map(|k| if k % 2 == 0 { 1.15 } else { 1.5 })
        .collect();
    let stats = klipolo_engine::test_support::caption_stats_via_export_pass(
        "caption.highlight",
        &CAPTION_WORDS,
        &times,
        (540, 960),
    )
    .await;
    // The first frames make the kept staging buffers (and write the uniform once).
    let (settled, last) = (stats[2], stats[11]);
    assert_eq!(
        last.uploads - settled.uploads,
        9,
        "every frame's quads moved"
    );
    assert_eq!(
        last.queue_writes - settled.queue_writes,
        0,
        "all through kept staging buffers: {stats:?}"
    );
}

/// One renderer's caption frames, uploading only what moved, draw what a fresh
/// renderer draws: the first word highlighted, then the second (its quads moved, through a kept staging buffer), then
/// the second again (nothing uploaded).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn one_renderers_moving_caption_frames_draw_what_fresh_ones_do() {
    use klipolo_engine::test_support::{compare, render_caption_frame, render_caption_frames};
    klipolo_engine::render::fonts::configure_for_tests();
    let (style, size, times) = ("caption.highlight", (540, 960), [1.15, 1.5, 1.5]);
    let kept = render_caption_frames(style, &CAPTION_WORDS, &times, size).await;
    let mut fresh = Vec::with_capacity(times.len());
    for (k, (frame, &t)) in kept.iter().zip(&times).enumerate() {
        fresh.push(render_caption_frame(style, &CAPTION_WORDS, t, size).await);
        let (within, mean) = compare(frame, &fresh[k], 6);
        assert!(
            within >= 0.9999 && mean <= 0.05,
            "frame {k} at {t} s: {:.3} % within 6 of a fresh renderer's, mean {mean:.3}",
            within * 100.0
        );
    }
    let (within, _) = compare(&fresh[0], &fresh[1], 6);
    assert!(
        within < 0.999,
        "the highlight moves between the two words: {:.3} % alike",
        within * 100.0
    );
}

/// A softened title whose sharper retry didn't fit remembers it (its
/// resolution and the bytes it needed), so its next retry lays nothing out while the room stays that small. Two huge
/// titles at 4K: the first holds most of the preview's ceiling at full resolution, so the second draws softer; a
/// retry waits a second (`RETRY_AFTER`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_softened_titles_retry_known_too_big_lays_nothing_out() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let (huge, canvas) = huge_titles(2);
    let (drawn, softer) = draw_frame(&dev, &mut drawer, &huge, canvas);
    assert!(
        drawn[0] == Some(1.0) && drawn[1].is_some_and(|s| s < 1.0) && softer,
        "the second draws softer: {drawn:?}"
    );
    let retry = |drawer: &mut TextDrawer| {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let before = drawer.stats().built_here;
        let (again, _) = draw_frame(&dev, drawer, &huge, canvas);
        assert_eq!(again, drawn, "the same resolutions");
        drawer.stats().built_here - before
    };
    assert!(
        retry(&mut drawer) > 0,
        "its sharper layout was tried, and didn't fit"
    );
    assert_eq!(
        retry(&mut drawer),
        0,
        "known too big for this room: nothing laid out"
    );
}

/// A title a frame waits for keeps its build when it lands, whatever builds are still in
/// flight: like any text of the frame it counts none of them. Here two huge builds asked after it claim the rest of
/// the text ceiling, and a title already on screen holds a little: the waited-for build used to be dropped as it
/// landed and the title built again on the render thread.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_waited_for_keeps_its_build_while_others_are_in_flight() {
    use klipolo_engine::render::text::TextDrawer;
    klipolo_engine::render::fonts::configure_for_tests();
    let dev = klipolo_engine::render::device::RenderDevice::new()
        .await
        .unwrap();
    let mut drawer = TextDrawer::new(&dev.device, &dev.queue, true);
    let out = (3840, 2160);
    let (entering, canvas) = the_title(titled("Entering", bx(0.4, 0.45, 0.2, 0.1), 72.0, 0.0));
    let (shown, _) = two_titles("", "On screen, a longer line of words than that");
    let shown = &shown[0];
    let (huge, huge_canvas) = huge_titles(2);
    drawer.ahead(&dev, &entering, canvas, out);
    for t in &huge {
        drawer.ahead(&dev, t, huge_canvas, out);
    }
    assert_eq!(drawer.stats().in_flight, 3);
    drawer.in_frame(shown);
    drawer.in_frame(&entering);
    drawer.begin_frame();
    assert_eq!(drawer.stats().in_flight, 3, "none landed yet");
    assert!(
        drawer
            .prepare(&dev.device, &dev.queue, shown, canvas, out, &mut Vec::new())
            .is_some()
    );
    let before = drawer.stats().built_here;
    assert!(
        drawer
            .prepare(
                &dev.device,
                &dev.queue,
                &entering,
                canvas,
                out,
                &mut Vec::new()
            )
            .is_some()
    );
    assert_eq!(
        drawer.stats().built_here,
        before,
        "it kept the build it waited for"
    );
    drawer.end_frame();
    until_landed(&mut drawer);
}
