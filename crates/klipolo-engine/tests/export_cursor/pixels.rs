//! Preview against export, pixel for pixel: the fixture's frames through the preview's compositor once
//! its shapes have arrived (decoded off the render thread), and through export's (strict, its shapes preloaded by the
//! export's own preparation: a headless render), every channel within 1/255. The fixture has clicks, a shape change, a
//! 2× Rect zoom, a dissolve into a second take (two cursor slots) and a shape that doesn't decode, which both refuse on
//! its take, so both draw it as the Arrow look: an export that met it only as a frame drew it would draw that frame's
//! stand-in at the shape's size instead.

use std::sync::Arc;
use std::time::Duration;

use klipolo_engine::headless::Headless;
use klipolo_engine::media::cursor::CursorTracks;
use klipolo_engine::plan::{CursorDraw, FramePlan, Layer, VideoRole, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::registry::Registry;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{PreviewBackend, RenderRequest, RenderedRgba};
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::time::Us;

use crate::{cursors_of, fixture, preview_ctx, preview_cursors};

const SIZE: (u32, u32) = (640, 360);

/// 30 frames 40 ms apart: the click, the shape change and the zoom, then into the dissolve.
fn frames() -> impl Iterator<Item = Us> {
    (0..30).map(|n| n * 40_000)
}

async fn preview_frame(backend: &mut GpuBackend, plan: FramePlan) -> RenderedRgba {
    let req = RenderRequest {
        plan,
        width: SIZE.0,
        height: SIZE.1,
        output: Output::Rgba,
    };
    backend.render(req).await.unwrap().finish().await.unwrap()
}

/// `f`'s rows without their padding.
fn rows(f: &RenderedRgba) -> Vec<u8> {
    (0..f.height as usize)
        .flat_map(|y| {
            let start = y * f.stride as usize;
            f.bytes[start..start + f.width as usize * 4].to_vec()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI) and scripts/gen-engine-fixtures.sh"]
async fn preview_and_export_pixels_match_on_the_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let p = fixture(dir.path(), true);
    let project = Arc::new(p.clone());
    // Export's copy: what an export does before its first frame, then strict frames.
    let mut export = Headless::new(Registry::builtin()).await.unwrap();
    export.load_cursors(&p);
    // The preview's copy: its takes as the session asks for them; its shapes asked for by its first frames, decoded on
    // their own thread and uploaded at a later frame's start.
    let cursors = preview_cursors(&p, CursorTracks::default());
    let ctx = preview_ctx(&p, &cursors);
    let mut preview = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap();
    preview.set_cursors(cursors.clone());
    let redraw = preview.redraw().unwrap();
    for t in frames() {
        preview_frame(&mut preview, frame_plan(&ctx, t, Landscape)).await;
    }
    for _ in 0..100 {
        if !preview.cursor_shapes_waiting() {
            break;
        }
        let _ = tokio::time::timeout(Duration::from_millis(200), redraw.notified()).await;
        preview_frame(&mut preview, frame_plan(&ctx, 0, Landscape)).await;
    }
    assert!(!preview.cursor_shapes_waiting(), "the shapes arrived");

    let (mut shapes, mut arrows, mut both, mut zoomed, mut rippled) = (0, 0, 0, 0, 0);
    for t in frames() {
        let plan = frame_plan(&ctx, t, Landscape);
        let cursors = cursors_of(&plan);
        shapes += cursors
            .iter()
            .filter(|c| matches!(c.look, CursorDraw::Shape { .. }))
            .count();
        // Take 1's last shape doesn't decode: refused on its take in the preview too.
        arrows += cursors
            .iter()
            .filter(|c| c.look == CursorDraw::Arrow)
            .count();
        both += usize::from(cursors.len() == 2);
        rippled += usize::from(cursors.iter().any(|c| c.ripple_count > 0));
        let main = plan.videos().find(|v| v.role == VideoRole::Main);
        zoomed += usize::from(main.is_some_and(|v| v.crop.w < 0.9));
        assert!(
            !matches!(plan.layers.first(), Some(Layer::Transition(_))) || cursors.len() == 2,
            "a dissolve draws both takes' cursors at {t} µs"
        );
        let a = rows(&preview_frame(&mut preview, plan).await);
        let b = rows(
            &export
                .render(project.clone(), t, Landscape, SIZE)
                .await
                .unwrap(),
        );
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        assert!(worst <= 1, "at {t} µs a channel differs by {worst}");
    }
    assert!(
        shapes > 15 && arrows > 2 && both > 2,
        "{shapes} shapes, {arrows} arrows, {both} with two"
    );
    assert!(
        zoomed > 5 && rippled > 1,
        "{zoomed} zoomed, {rippled} rippling"
    );
}
