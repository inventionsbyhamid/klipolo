//! The cursor shapes behind the pixel test: a refused shape stays refused when its take is loaded again, nothing is
//! m3), nothing is made for cursors where no clip draws one, and an export pass puts every shape in each of its
//! its first frame. They need a GPU (lavapipe in CI).

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_cursor::format::Line;
use klipolo_engine::export::prepare::{cursor_shapes, load_cursors, preload_formats};
use klipolo_engine::headless::Headless;
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::plan::{CursorDraw, PlanContext, frame_plan};
use klipolo_engine::registry::Registry;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::render::cursor::CursorPass;
use klipolo_engine::render::export::FormatTargets;
use klipolo_engine::render::yuv::Yuv;
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::project::Project;

use crate::support::secs;
use crate::{Shape, add_take, cursors_of, device, fixture, glide, write_take};

/// The cursor `ctx` plans at 1 s.
fn look(ctx: &PlanContext) -> CursorDraw {
    cursors_of(&frame_plan(ctx, secs(1.0), Landscape))[0]
        .look
        .clone()
}

#[test]
#[ignore = "needs a GPU (lavapipe in CI)"]
fn a_refused_shape_stays_refused_when_its_take_is_loaded_again() {
    // The preview's atlas keeps a shape whose image didn't decode as refused; a take evicted and loaded again has
    // forgotten its refusal, so the next frame that draws it must tell it again, or the preview would draw the stand-in
    // where export draws the Arrow look.
    let dir = tempfile::tempdir().unwrap();
    let broken = Shape {
        id: 0,
        side: 48,
        rgba: [255; 4],
        damaged: true,
    };
    let mut lines: Vec<Line> = vec![Line::Shape { t: 0, id: 0 }];
    lines.extend(glide((0.3, 0.3), (0.6, 0.6), 0.0, 2.0));
    let take = write_take(dir.path(), 1, &[broken], lines);
    let mut p = Project::new("Damaged shape");
    add_take(
        &mut p,
        1,
        "/media/screen",
        2.0,
        (&take, CursorCapture::Separate),
        (0.0, 2.0),
    );
    let tracks = Arc::new(CursorTracks::default());
    tracks.load_all(&wanted(&p));
    let ctx = PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), tracks.clone());
    let dev = device();
    let mut pass = CursorPass::new(&dev.device, Arc::default());
    // A preview frame: what arrived goes in, the cursor is drawn (asking for its shape), and the refusals go back.
    let frame = |pass: &mut CursorPass| {
        pass.begin_frame();
        pass.upload_ready(&dev.device, &dev.queue);
        let plan = frame_plan(&ctx, secs(1.0), Landscape);
        let _ = pass.prepare(cursors_of(&plan)[0], (256, 144), false);
        pass.take_refused(|take, shape| tracks.refuse_shape(take, shape));
    };
    assert!(matches!(look(&ctx), CursorDraw::Shape { .. }));
    frame(&mut pass);
    let deadline = Instant::now() + Duration::from_secs(10);
    while pass.shapes_waiting() {
        assert!(Instant::now() < deadline, "the shape's decode");
        std::thread::sleep(Duration::from_millis(10));
        frame(&mut pass);
    }
    assert_eq!(look(&ctx), CursorDraw::Arrow, "refused");
    // Evicted and loaded again: a new take, which plans the shape until a frame tells it again.
    tracks.clear();
    tracks.load_all(&wanted(&p));
    assert!(matches!(look(&ctx), CursorDraw::Shape { .. }));
    frame(&mut pass);
    assert_eq!(look(&ctx), CursorDraw::Arrow, "refused again");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn no_cursor_pass_is_made_where_no_clip_draws_a_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut burned = fixture(dir.path(), false);
    for r in &mut burned.recordings {
        r.cursor.as_mut().unwrap().capture = CursorCapture::Burned;
    }
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    headless.load_cursors(&burned);
    assert!(!headless.cursor_pass_made(), "nothing to draw: no pass");
    headless.load_cursors(&fixture(dir.path(), false));
    assert!(headless.cursor_pass_made(), "the fixture's shapes");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_export_pass_puts_every_shape_in_each_formats_atlas_before_frame_one() {
    // What `export::pass` does before its first frame, for a pass of two formats: each format's
    // atlas holds every shape its clips draw, and the damaged one is refused once, not once per format.
    let dir = tempfile::tempdir().unwrap();
    let backend = GpuBackend::for_export_with(false).await.unwrap();
    let formats = || {
        let target = |size| backend.format_target(size, Yuv::Nv12, false).unwrap();
        FormatTargets::new(vec![target((256, 144)), target((144, 256))])
    };
    let context = |p: &Project| {
        let loaded = load_cursors(p);
        PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), loaded.cursors)
    };
    let p = fixture(dir.path(), false);
    let ctx = context(&p);
    let shapes = cursor_shapes(&p, &ctx.cursors);
    let mut pass = formats();
    assert_eq!(
        preload_formats(&ctx, &mut pass),
        1,
        "the damaged shape, once"
    );
    let usable: Vec<_> = (shapes.iter())
        .filter(|(take, shape, _)| {
            ctx.cursors
                .get(take)
                .is_some_and(|t| t.shape_file(*shape).is_some())
        })
        .collect();
    assert_eq!(usable.len() + 1, shapes.len(), "{shapes:?}");
    for target in pass.iter_mut() {
        target.compositor_mut().with_cursor_pass(|cursor, _| {
            for (take, shape, _) in &usable {
                assert!(cursor.resident(take, *shape), "{take} shape {shape}");
            }
        });
    }
    // Where every cursor is in the frames, no format makes a cursor pass at all.
    let mut burned = p;
    for r in &mut burned.recordings {
        r.cursor.as_mut().unwrap().capture = CursorCapture::Burned;
    }
    let mut pass = formats();
    assert_eq!(preload_formats(&context(&burned), &mut pass), 0);
    assert!(
        pass.iter_mut()
            .all(|t| !t.compositor_mut().cursor_pass_made())
    );
}
