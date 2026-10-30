//! The cursor as a planned layer: where it lands through zoom and framing, how big and how
//! visible it is, its press, ripples and smear: a pure function of the project, the take and the frame time, planned
//! with no allocation. The fixtures here; placement in `cursor_layer/place.rs`, the effects in `cursor_layer/effects.rs`,
//! the view it is drawn in (hidden, off the zoomed view, masked, clip speed) in `cursor_layer/views.rs`, preview
//! against export, transitions, the session, allocation and speed in `cursor_layer/parity.rs`, and the take's clock
//! against its screen's frames in `cursor_layer/clock.rs`.

mod support;

#[path = "cursor_layer/clock.rs"]
mod clock;
#[path = "cursor_layer/effects.rs"]
mod effects;
#[path = "cursor_layer/parity.rs"]
mod parity;
#[path = "cursor_layer/place.rs"]
mod place;
#[path = "cursor_layer/views.rs"]
mod views;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use klipolo_cursor::format::{Capture, Line};
use klipolo_engine::media::cursor::{CursorTracks, TakeCursor, wanted};
use klipolo_engine::plan::{CursorLayer, Layer, PlanContext, VideoLayer, VideoRole, frame_plan};
use klipolo_engine::registry::Registry;
use klipolo_model::cursor_style::CursorStyle;
use klipolo_model::project::FormatKind::{self, Landscape};
use klipolo_model::project::Project;
use support::secs;

/// Counts this thread's allocations (`planning_a_cursor_allocates_nothing`).
struct Counting;

thread_local! {
    pub static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator; the counter is a const-initialised thread-local (no allocation).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(p, layout, size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// The captured area's height in points (`support::takes::header`).
pub const AREA_H: f64 = 900.0;

/// A take's lines: its header (a 1440 × 900 point screen, exact clicks), then `more`.
pub fn take(more: impl IntoIterator<Item = Line>) -> Vec<Line> {
    let mut lines = vec![support::takes::header(Capture::Separate)];
    lines.extend(more);
    lines
}

/// The cursor resting at `(x, y)` from `from` to `to` seconds: a heartbeat every 500 ms.
pub fn rest(x: f32, y: f32, from: f64, to: f64) -> impl Iterator<Item = Line> {
    let beats = ((to - from) * 2.0).round() as i64;
    (0..=beats).map(move |k| Line::Move {
        t: secs(from) + k * 500_000,
        x,
        y,
    })
}

/// A straight glide from `a` to `b` between `from` and `to` seconds, at 120 Hz.
pub fn glide(a: (f32, f32), b: (f32, f32), from: f64, to: f64) -> impl Iterator<Item = Line> {
    let steps = ((to - from) * 120.0).round() as i64;
    (0..=steps).map(move |k| {
        let s = k as f32 / steps as f32;
        Line::Move {
            t: secs(from) + (secs(to) - secs(from)) * k / steps,
            x: a.0 + (b.0 - a.0) * s,
            y: a.1 + (b.1 - a.1) * s,
        }
    })
}

/// A press of button `btn` at `(x, y)` from `down` to `up` seconds.
pub fn press(btn: u8, (x, y): (f32, f32), down: f64, up: f64) -> [Line; 2] {
    let line = |t, d| Line::Button { t, btn, d, x, y };
    [line(secs(down), 1), line(secs(up), 0)]
}

/// The default style changed by `f`.
pub fn style(f: impl FnOnce(&mut CursorStyle)) -> CursorStyle {
    let mut s = CursorStyle::default();
    f(&mut s);
    s
}

/// Export's context: the project's takes read and smoothed before the first frame (`load_all`).
pub fn export_ctx(p: Project) -> PlanContext {
    let tracks = CursorTracks::default();
    tracks.load_all(&wanted(&p));
    PlanContext::with_cursors(Arc::new(p), Registry::builtin(), Arc::new(tracks))
}

/// Every cursor layer of the frame at `t` seconds in `format`, a transition's sides included.
pub fn cursors_at(ctx: &PlanContext, t: f64, format: FormatKind) -> Vec<CursorLayer> {
    let plan = frame_plan(ctx, secs(t), format);
    let mut out = Vec::new();
    for layer in &plan.layers {
        let sides: Vec<&Layer> = match layer {
            Layer::Transition(t) => t.outgoing.iter().chain(&t.incoming).collect(),
            other => vec![other],
        };
        out.extend(sides.into_iter().filter_map(|l| match l {
            Layer::Cursor(c) => Some(c.clone()),
            _ => None,
        }));
    }
    out
}

/// The cursor layer of the frame at `t` seconds (landscape), if there is one.
pub fn cursor_at(ctx: &PlanContext, t: f64) -> Option<CursorLayer> {
    let mut all = cursors_at(ctx, t, Landscape);
    assert!(all.len() <= 1, "{all:?}");
    all.pop()
}

/// The main footage of the frame at `t` seconds in `format`.
pub fn footage(ctx: &PlanContext, t: f64, format: FormatKind) -> VideoLayer {
    let plan = frame_plan(ctx, secs(t), format);
    plan.videos()
        .find(|v| v.role == VideoRole::Main)
        .cloned()
        .expect("main footage")
}

/// The take the project's first recording draws (export's).
pub fn the_take(ctx: &PlanContext) -> Arc<TakeCursor> {
    let path = &ctx.project.recordings[0].cursor.as_ref().unwrap().path;
    ctx.cursors.get(path).expect("loaded")
}

/// Where footage `v` shows source point `(x, y)`: its box's mapping of its crop.
pub fn mapped(v: &VideoLayer, (x, y): (f32, f32)) -> (f64, f64) {
    let (x, y) = (f64::from(x), f64::from(y));
    (
        v.dest.x + (x - v.crop.x) / v.crop.w * v.dest.w,
        v.dest.y + (y - v.crop.y) / v.crop.h * v.dest.h,
    )
}

pub fn near(a: (f64, f64), b: (f64, f64), eps: f64) {
    assert!(
        (a.0 - b.0).abs() <= eps && (a.1 - b.1).abs() <= eps,
        "{a:?} != {b:?}"
    );
}
