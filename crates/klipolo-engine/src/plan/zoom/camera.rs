//! Auto zooms planned by the smart camera. An Auto zoom layer over
//! takes with cursor data is one `CameraRun`, planned by `klipolo_cursor::camera` over its span ahead of time from:
//! - **the focus:** `camera::focus_from` of the takes' facts on the timeline (`on_timeline_between`, exactly what
//!   `on_timeline` gives there), the function chat plans its zooms with, so chat's zooms play as it planned them.
//!   The kinds a narrowed `using` leaves out stay on screen (`keep_from`);
//! - **the fixed shots:** the person's Rect zooms inside the span, as they made them;
//! - **the glide:** the raw cursor (`RawCursor`, each clip's take looked up once), never the style's smoothing.
//!
//! A run is planned on its own clock (0 at its start), so one moved in time as a whole (a ripple) keeps its plan; its
//! `CameraKey` holds everything the plan reads, relative to its start. An Auto zoom over no cursor data (an imported
//! clip, a take not loaded yet) looks at the centre on the springs, as before (`auto_rect(None, scale)`).
//! **Cost:** listing the runs and their keys is O(Auto items × (log clips + clips under them)); a plan is
//! O(focus log focus) plus its glide, O(60 Hz steps of its shots); a lookup is two binary searches with no allocation.

use std::sync::Arc;

use klipolo_cursor::camera::{
    Fixed, Focus, Input, Plan, Rules, focus_from, keep_from, plan_keeping,
};
use klipolo_cursor::timeline::{RawCursor, TakeFacts, on_timeline_between};
use klipolo_model::cursor_style::FocusKind;
use klipolo_model::layout::BoxN;
use klipolo_model::project::{Project, Recording};
use klipolo_model::time::Us;

use super::super::takes::Takes;

mod runs;

pub(super) use runs::split;

/// An Auto zoom's planned camera over `[start_us, end_us)`; its plan is on its own clock (0 at `start_us`).
pub struct CameraRun {
    pub start_us: Us,
    pub end_us: Us,
    pub plan: Arc<Plan>,
}

/// What a run's plan depends on, compared to decide whether a context can reuse it: the Auto item (length, scale,
/// `using`), the Rect items inside it, and each clip touching it (where it starts, what it plays, and its take's file,
/// samples and activity), all relative to its start.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraKey {
    len: Us,
    scale: u64,
    using: Vec<FocusKind>,
    fixed: Vec<(Us, Us, [u64; 3])>,
    clips: Vec<ClipKey>,
}

/// A clip under a run: where it starts (from the run's start), its source window and speed, and its take's part.
type ClipKey = (Us, Us, Us, u64, Option<TakeKey>);
/// A take's part of a key: its cursor file, its samples, its activity's fingerprint and its screen's nudge.
type TakeKey = (Arc<str>, usize, u64, Us);
/// A take's facts, by its cursor file.
type Known = (Arc<str>, Arc<TakeFacts>);

/// One Auto run to plan: its key, its span and what its plan reads.
pub(super) struct AutoRun {
    pub(super) key: CameraKey,
    pub(super) start: Us,
    pub(super) end: Us,
    scale: f64,
    using: Vec<FocusKind>,
    fixed: Vec<Fixed>,
    /// The facts of each take under it, of the activity its key names.
    facts: Vec<Known>,
}

/// What planning a run reads, shared so a build can run off the actor.
#[derive(Clone)]
pub(super) struct Inputs {
    pub(super) p: Arc<Project>,
    pub(super) starts: Arc<[Us]>,
    pub(super) takes: Arc<Takes>,
}

/// The part of the main source frame a zoom at `scale` shows around `center` (the frame's centre without one): a
/// `1 / scale` square, kept inside the frame.
pub fn auto_rect(center: Option<(f32, f32)>, scale: f64) -> BoxN {
    let size = 1.0 / scale.max(1.0);
    let (cx, cy) = center.map_or((0.5, 0.5), |(x, y)| (f64::from(x), f64::from(y)));
    BoxN {
        x: (cx - size / 2.0).clamp(0.0, 1.0 - size),
        y: (cy - size / 2.0).clamp(0.0, 1.0 - size),
        w: size,
        h: size,
    }
}

/// Plans `run` (see the module notes). O(focus log focus + clips under it × log facts + its glide's steps).
pub(super) fn plan_run(run: &AutoRun, i: &Inputs) -> Plan {
    let (p, starts, start) = (&*i.p, &*i.starts, run.start);
    let facts_of = |r: &Recording| {
        let path = r.cursor_path()?;
        (run.facts.iter())
            .find(|(p, _)| **p == *path)
            .map(|(_, f)| f.clone())
    };
    let facts = on_timeline_between(p, starts, &facts_of, start, run.end);
    let own = |f: Focus| Focus {
        at: f.at - start,
        until: f.until.saturating_sub(start),
        ..f
    };
    let focus: Vec<Focus> = (focus_from(&facts, &run.using, start, run.end).into_iter())
        .map(own)
        .collect();
    let keep: Vec<Focus> = (keep_from(&facts, &run.using, start, run.end).into_iter())
        .map(own)
        .collect();
    let input = Input {
        span: (0, run.end - start),
        focus: &focus,
        fixed: &run.fixed,
        rules: Rules {
            max_scale: run.scale,
            ..Rules::default()
        },
    };
    let track_of = |r: &Recording| i.takes.of_recording(r).map(|t| t.track.clone());
    let raw = RawCursor::new(p, starts, &track_of, start, run.end);
    plan_keeping(&input, &keep).with_glide(&|t| raw.at(t.saturating_add(start)))
}

/// The part of the source frame `plan` shows at `t` on its own clock, or None while it shows the whole frame.
pub(super) fn window(plan: &Plan, t: Us) -> Option<BoxN> {
    let v = plan.view_at(t);
    if v.scale.is_nan() || v.scale <= 1.0005 {
        return None;
    }
    let w = 1.0 / v.scale;
    let at = |c: f64| (c - w / 2.0).clamp(0.0, 1.0 - w);
    Some(BoxN {
        x: at(v.cx),
        y: at(v.cy),
        w,
        h: w,
    })
}
