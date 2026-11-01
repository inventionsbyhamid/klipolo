//! A video layer showing part of its picture (cropped, or filling its box) is drawn with only that part, in preview
//! and in export, upright from a phone's turned frames, with or without effects and through its blend; and a clip
//! fitted in a frame of another shape has its own picture blurred round it with the blurred video background.
//!
//! The layer is the phone fixture (shown 720×1280: red on top, blue below) in the left half of a 16:9 frame over grey:
//! contained, it is a tall picture with grey either side, red above the middle and blue below.

use std::sync::Arc;

use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::contrib::builtin;
use klipolo_model::crop::Crop;
use klipolo_model::motion::Blend;
use klipolo_model::params::ParamValue;
use klipolo_model::project::{BackgroundKind, FormatKind, Project};
use klipolo_model::timeline::{EffectInstance, ItemContent};

use super::support::*;
use super::{near, pixel, use_fixture};

const W: u32 = 640;
const H: u32 = 360;
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const GREY: [u8; 3] = [0x80, 0x80, 0x80];

/// The phone clip as a layer in the frame's left half over grey.
fn phone_layer(
    fill: bool,
    crop: Option<Crop>,
    effects: Vec<EffectInstance>,
    blend: Option<Blend>,
) -> Project {
    let mut p = project();
    use_fixture(&mut p, "broll", "phone-portrait.mov", 1.0);
    let phone = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    (phone.width, phone.height) = (720, 1280);
    p.timeline.main.clear();
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#808080".into();
    let content = ItemContent::Video {
        asset_id: "broll".into(),
        source_in_us: 0,
        speed: 1.0,
        volume: 1.0,
        muted: true,
        mirror: false,
        fill,
        crop,
        tie: None,
    };
    let mut layer = item("v", 0.0, 1.0, bx(0.0, 0.0, 0.5, 1.0), content);
    layer.effects = effects;
    layer.layout.blend = blend;
    p.timeline.tracks = vec![overlay_track("o", vec![layer])];
    p
}

/// The bottom half: only blue is left.
fn bottom_half() -> Option<Crop> {
    Some(Crop {
        top: 0.5,
        ..Crop::NONE
    })
}

/// A vignette at nothing: an effect that leaves the picture as it is.
fn no_op_effect() -> Vec<EffectInstance> {
    let vignette = EffectInstance {
        id: "e".into(),
        ..EffectInstance::new(builtin::EFFECT_VIGNETTE)
    };
    vec![vignette.with("amount", ParamValue::Number(0.0))]
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn a_video_layer_draws_only_the_part_of_its_picture_it_shows() {
    for export in [false, true] {
        let mut backend = if export {
            GpuBackend::for_export().await.unwrap()
        } else {
            GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
                .await
                .unwrap()
                .wait_for_assets()
        };
        let mut render = async |p: Project| {
            let plan = frame_plan(
                &PlanContext::new(Arc::new(p)),
                secs(0.5),
                FormatKind::Landscape,
            );
            let request = RenderRequest {
                plan,
                width: W,
                height: H,
                output: Output::Rgba,
            };
            let frame = backend
                .render(request)
                .await
                .unwrap()
                .finish()
                .await
                .unwrap();
            assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
            frame
        };
        let side = if export { "export" } else { "preview" };
        let check = |frame: &klipolo_engine::session::RenderedRgba,
                     what: &str,
                     want: &[((u32, u32), [u8; 3])]| {
            for &((x, y), rgb) in want {
                let got = pixel(&frame.bytes, frame.stride, x, y);
                assert!(
                    near(got, rgb, 14),
                    "{side}, {what}: ({x}, {y}) is {got:?}, not {rgb:?}"
                );
            }
        };
        // Contained whole (202×360 centred in the 320×360 half): red above, blue below, grey beside.
        let whole = render(phone_layer(false, None, Vec::new(), None)).await;
        check(
            &whole,
            "whole",
            &[((160, 60), RED), ((160, 300), BLUE), ((20, 180), GREY)],
        );
        // Cropped to its bottom half (720×640, 320×284 in the box): all blue, the red gone.
        let cropped = render(phone_layer(false, bottom_half(), Vec::new(), None)).await;
        check(
            &cropped,
            "cropped",
            &[((160, 60), BLUE), ((160, 300), BLUE), ((160, 10), GREY)],
        );
        // The same through an effect, and multiplied over white (which gives the layer back).
        let effected = render(phone_layer(false, bottom_half(), no_op_effect(), None)).await;
        check(
            &effected,
            "cropped, with an effect",
            &[((160, 60), BLUE), ((160, 300), BLUE)],
        );
        let mut white = phone_layer(false, bottom_half(), Vec::new(), Some(Blend::Multiply));
        white.background.color = "#FFFFFF".into();
        let multiplied = render(white).await;
        check(
            &multiplied,
            "cropped, multiplied",
            &[((160, 60), BLUE), ((160, 10), [255, 255, 255])],
        );
        // Filling the box: the middle of the picture covers it, the grey beside it gone.
        let filled = render(phone_layer(true, None, Vec::new(), None)).await;
        check(
            &filled,
            "filled",
            &[((20, 60), RED), ((300, 300), BLUE), ((340, 180), GREY)],
        );
    }
}

/// The blurred video background: a 16:9 clip fitted in 9:16 has its own picture, dimmed, above and below it to the
/// frame's edges, where the plain background would leave black.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn a_fitted_clip_has_its_own_picture_blurred_and_dimmed_round_it() {
    use klipolo_model::timeline::{Clip, ClipSource, Framing, FramingMode};
    let blue = [0x1E, 0x64, 0xDC];
    let fitted = |kind: BackgroundKind| {
        let mut p = project();
        use_fixture(&mut p, "broll", "solid-blue.mp4", 2.0);
        let mut clip = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(2.0));
        clip.framing.insert(
            FormatKind::Portrait,
            Framing {
                mode: FramingMode::Fit,
                focus_x: 0.5,
                focus_y: 0.5,
                follow: None,
            },
        );
        p.timeline.main = vec![clip];
        p.timeline.tracks.clear();
        p.background.kind = kind;
        p
    };
    for export in [false, true] {
        let mut backend = if export {
            GpuBackend::for_export().await.unwrap()
        } else {
            GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
                .await
                .unwrap()
                .wait_for_assets()
        };
        let mut render = async |p: Project| {
            let plan = frame_plan(
                &PlanContext::new(Arc::new(p)),
                secs(1.0),
                FormatKind::Portrait,
            );
            let request = RenderRequest {
                plan,
                width: 360,
                height: 640,
                output: Output::Rgba,
            };
            let frame = backend
                .render(request)
                .await
                .unwrap()
                .finish()
                .await
                .unwrap();
            assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
            frame
        };
        let side = if export { "export" } else { "preview" };
        let plain = render(fitted(BackgroundKind::None)).await;
        let at = |f: &klipolo_engine::session::RenderedRgba, x, y| pixel(&f.bytes, f.stride, x, y);
        // The 16:9 picture is 202 px tall in the middle; black above and below it.
        assert!(
            near(at(&plain, 180, 320), blue, 6),
            "{side}: {:?}",
            at(&plain, 180, 320)
        );
        assert!(
            near(at(&plain, 180, 60), [0, 0, 0], 2),
            "{side}: {:?}",
            at(&plain, 180, 60)
        );
        let blurred = render(fitted(BackgroundKind::Blur)).await;
        assert!(
            near(at(&blurred, 180, 320), blue, 6),
            "{side}: the clip itself"
        );
        // Above, below and in the corners: the blue, dimmed, never black or bright.
        for (x, y) in [(180, 60), (180, 600), (1, 1), (358, 638)] {
            let [r, g, b] = at(&blurred, x, y);
            assert!(
                b > 40 && b + 30 < blue[2] && b > r && b > g,
                "{side}: ({x}, {y}) is {:?}",
                [r, g, b]
            );
        }
    }
}

/// The blurred background is drawn small and blurred (the blur hides the size it is drawn at): preview and export draw
/// it the same, over picture content that isn't flat (the bars), above and below the fitted clip.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn the_blurred_background_is_the_same_in_preview_and_export() {
    use klipolo_model::timeline::{Clip, ClipSource, Framing, FramingMode};
    let mut p = project();
    use_fixture(&mut p, "broll", "bars-30fps-0.5gop.mp4", 4.0);
    let mut clip = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(4.0));
    clip.framing.insert(
        FormatKind::Portrait,
        Framing {
            mode: FramingMode::Fit,
            focus_x: 0.5,
            focus_y: 0.5,
            follow: None,
        },
    );
    p.timeline.main = vec![clip];
    p.timeline.tracks.clear();
    p.background.kind = BackgroundKind::Blur;
    let mut frames = Vec::new();
    for export in [false, true] {
        let mut backend = if export {
            GpuBackend::for_export().await.unwrap()
        } else {
            GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
                .await
                .unwrap()
                .wait_for_assets()
        };
        let plan = frame_plan(
            &PlanContext::new(Arc::new(p.clone())),
            secs(1.0),
            FormatKind::Portrait,
        );
        let request = RenderRequest {
            plan,
            width: 360,
            height: 640,
            output: Output::Rgba,
        };
        let frame = backend
            .render(request)
            .await
            .unwrap()
            .finish()
            .await
            .unwrap();
        assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
        frames.push(frame);
    }
    let (preview, export) = (&frames[0], &frames[1]);
    let points = [
        (30, 40),
        (180, 40),
        (330, 40),
        (30, 600),
        (180, 600),
        (330, 600),
        (2, 2),
        (357, 637),
    ];
    let at = |f: &klipolo_engine::session::RenderedRgba, (x, y): (u32, u32)| {
        pixel(&f.bytes, f.stride, x, y)
    };
    for point in points {
        let (a, b) = (at(preview, point), at(export, point));
        assert!(near(a, b, 8), "{point:?}: preview {a:?}, export {b:?}");
    }
    // Not flat: the bars' colours show through the blur, so the agreement means something.
    let colours: Vec<[u8; 3]> = points.iter().map(|&point| at(preview, point)).collect();
    assert!(
        colours.iter().any(|c| !near(*c, colours[0], 12)),
        "the background is one flat colour: {colours:?}"
    );
}
