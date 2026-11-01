//! Export's cursors before its first frame: every take and its smoothing loaded as the preview holds them, every shape
//! in range in the atlas from frame 0, and preview and
//! export planning the same cursor. A separate cursor is never dropped silently (`export_cursor/asks.rs`), preview and
//! export draw the same pixels (`export_cursor/pixels.rs`), and what it costs (`export_cursor/timing.rs`). The screens'
//! activity is made before frame 1 (`export_cursor/analysing.rs`), so export plans the preview's camera runs and follow
//! crops (`export_cursor/activity.rs`). The GPU tests need a GPU (lavapipe in CI), the pixel and cost tests
//! also `scripts/gen-engine-fixtures.sh`:
//! `WGPU_BACKEND=vulkan cargo test -p klipolo-engine --test export_cursor -- --ignored`.

mod support;

#[path = "export_cursor/activity.rs"]
mod activity;
#[path = "export_cursor/analysing.rs"]
mod analysing;
#[path = "export_cursor/asks.rs"]
mod asks;
#[path = "export_cursor/events.rs"]
mod events;
#[path = "export_cursor/fixture.rs"]
mod fixture;
#[path = "export_cursor/pixels.rs"]
mod pixels;
#[path = "export_cursor/retries.rs"]
mod retries;
#[path = "export_cursor/shapes.rs"]
mod shapes;
/// The activity pass's synthetic screen (shared with `activity_pass`).
#[path = "activity_pass/synthetic.rs"]
#[allow(dead_code)]
mod synthetic;
#[path = "export_cursor/timing.rs"]
mod timing;
#[path = "export_cursor/typing.rs"]
mod typing;
#[path = "export_cursor/worker.rs"]
mod worker;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

pub use fixture::{Shape, add_take, click, fixture, glide, rest, write_take};
use klipolo_cursor::format::Line;
use klipolo_engine::export::prepare::{Retry, cursor_shapes, load_cursors, prepare_cursors};
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::plan::{CursorDraw, CursorLayer, FramePlan, Layer, PlanContext, frame_plan};
use klipolo_engine::registry::Registry;
use klipolo_engine::render::cursor::CursorPass;
use klipolo_engine::render::device::RenderDevice;
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::project::Project;
use support::secs;

pub const WAIT: Duration = Duration::from_secs(30);
/// Export's analysis trying a screen that can't be read now again at once (three times), then asking.
pub const NOW: Retry<'static> = Retry {
    waits: &[Duration::ZERO; 3],
    without: Vec::new(),
};
/// The fixture's frame rate.
pub const FPS: u32 = 30;

/// Export's context for `p`: its takes as an export loads them (`export::prepare::load_cursors`).
pub fn export_ctx(p: &Project) -> PlanContext {
    let loaded = load_cursors(p);
    assert!(loaded.missing.is_empty(), "every cursor can be read");
    PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), loaded.cursors)
}

/// The preview's takes for `p`, as the session asks for them (`want` on each publish, read and smoothed off the actor),
/// waited for.
pub fn preview_cursors(p: &Project, tracks: CursorTracks) -> Arc<CursorTracks> {
    let cursors = Arc::new(tracks);
    let wants = wanted(p);
    cursors.want(&wants);
    assert!(cursors.wait_ready(&wants, WAIT), "the preview's takes");
    cursors
}

/// The preview's context for `p` drawing `cursors`' takes.
pub fn preview_ctx(p: &Project, cursors: &Arc<CursorTracks>) -> PlanContext {
    let (ctx, build) = PlanContext::starting(Arc::new(p.clone()), None, cursors);
    match build {
        None => ctx,
        Some(build) => {
            let mut ctx = ctx;
            assert!(ctx.with_zoom(build.finish()));
            ctx
        }
    }
}

/// Every cursor layer of `plan`, a transition's sides included.
pub fn cursors_of(plan: &FramePlan) -> Vec<&CursorLayer> {
    let mut out = Vec::new();
    for layer in &plan.layers {
        let sides: Vec<&Layer> = match layer {
            Layer::Transition(t) => t.outgoing.iter().chain(&t.incoming).collect(),
            other => vec![other],
        };
        out.extend(sides.into_iter().filter_map(|l| match l {
            Layer::Cursor(c) => Some(c),
            _ => None,
        }));
    }
    out
}

pub fn device() -> Arc<RenderDevice> {
    futures::executor::block_on(RenderDevice::new()).expect("a GPU (lavapipe in CI)")
}

#[test]
#[ignore = "needs a GPU (lavapipe in CI)"]
fn export_frames_have_the_cursor_from_frame_zero() {
    // One separate take, two shapes (the second from 1 s), the cursor gliding: 60 frames at 30 fps.
    let dir = tempfile::tempdir().unwrap();
    let shapes = [
        Shape {
            id: 3,
            side: 32,
            rgba: [255, 0, 0, 255],
            damaged: false,
        },
        Shape {
            id: 7,
            side: 40,
            rgba: [0, 0, 255, 255],
            damaged: false,
        },
    ];
    let mut lines: Vec<Line> = vec![
        Line::Shape { t: 0, id: 3 },
        Line::Shape {
            t: secs(1.0),
            id: 7,
        },
    ];
    lines.extend(glide((0.2, 0.3), (0.7, 0.6), 0.0, 2.0));
    let take = write_take(dir.path(), 1, &shapes, lines);
    let mut p = Project::new("One take");
    p.settings.fps = FPS;
    add_take(
        &mut p,
        1,
        "/media/screen",
        2.0,
        (&take, CursorCapture::Separate),
        (0.0, 2.0),
    );
    // What export does before its first frame.
    let loaded = load_cursors(&p);
    let dev = device();
    let mut pass = CursorPass::new(&dev.device, Arc::default());
    let shapes = cursor_shapes(&p, &loaded.cursors);
    assert_eq!(
        prepare_cursors(&loaded.cursors, &shapes, &mut pass, &dev),
        0
    );
    let ctx = PlanContext::with_cursors(Arc::new(p), Registry::builtin(), loaded.cursors);
    let mut seen = BTreeSet::new();
    for n in 0..60 {
        let plan = frame_plan(
            &ctx,
            klipolo_engine::plan::frame_start_us(n, FPS),
            Landscape,
        );
        let cursor = *cursors_of(&plan).first().expect("a cursor on every frame");
        let CursorDraw::Shape { take, shape, .. } = &cursor.look else {
            panic!("frame {n}: {:?}", cursor.look);
        };
        // Its real shape is in the atlas before the frame draws: never the Arrow stand-in.
        assert!(pass.resident(take, *shape), "frame {n}: shape {shape}");
        pass.begin_frame();
        assert!(
            pass.prepare(cursor, (256, 144), true).is_some(),
            "frame {n}"
        );
        seen.insert(*shape);
    }
    assert_eq!(seen, BTreeSet::from([3, 7]), "both shapes");
}

#[test]
fn export_draws_the_same_cursor_as_preview() {
    let dir = tempfile::tempdir().unwrap();
    let p = fixture(dir.path(), false);
    let export = export_ctx(&p);
    let preview = preview_ctx(&p, &preview_cursors(&p, CursorTracks::default()));
    let end = export.video_duration_us;
    let (mut drawn, mut transitions) = (0, 0);
    for n in 0..600 {
        let t = end * n / 600;
        let plan = frame_plan(&export, t, Landscape);
        assert_eq!(
            plan,
            frame_plan(&preview, t, Landscape),
            "frame {n} at {t} µs"
        );
        drawn += cursors_of(&plan).len();
        transitions += usize::from(matches!(plan.layers.first(), Some(Layer::Transition(_))));
    }
    // A cursor on nearly every frame, two in the dissolve.
    assert!(
        drawn > 550 && transitions > 50,
        "{drawn} cursors, {transitions} transition frames"
    );
}
