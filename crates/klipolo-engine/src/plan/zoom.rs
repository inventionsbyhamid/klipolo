//! Eased zoom over the main footage. Targets are normalized to the main SOURCE frame as it's
//! shown (a mirrored camera-only clip's mirror image of its file), so every format zooms the same
//! part of the picture.
//! - **Rect zooms** become Cap zoom segments in timeline seconds that Cap's spring timeline (vendored) smooths
//!   (`springs`): items closer than `SETTLE_US` form a run simulated on its own, so precompute is proportional to the
//!   zoomed time and an edit recomputes only the runs it changed (a run moved in time as a whole is reused).
//! - **Auto zooms** over cursor data are camera runs (`camera`): planned ahead of time from the
//!   takes' facts and raw cursor, with the Rect zooms inside them as fixed shots. Elsewhere (an imported clip, a take
//!   not loaded yet) an Auto zoom looks at the centre on the springs.
//!
//! `window` checks the camera runs first, then the springs: two binary searches, no allocation. A rebuild bigger than
//! `INLINE_STEPS` (spring steps, or 125 a second of camera run) is finished off the actor (`ZoomBuild`).

use std::sync::Arc;

use klipolo_model::layout::BoxN;
use klipolo_model::project::Project;
use klipolo_model::query::{clip_starts, main_track_duration};
use klipolo_model::time::Us;
use klipolo_model::timeline::{ItemContent, TrackKind, ZoomTarget};

use super::takes::Takes;
use crate::media::cursor::CursorTracks;

mod camera;
mod springs;

pub use camera::{CameraKey, CameraRun, auto_rect};
pub use springs::zoom_runs;

/// One zoom item: (start, end, scale, target), compared to decide whether a context can reuse the springs.
pub type ZoomItem = (Us, Us, f64, ZoomTarget);

/// How long the springs take to come to rest after a run's last zoom ends.
pub const SETTLE_US: Us = 2_000_000;
/// A rebuild that needs more spring steps than this (125 per second of run; a camera run counts the same) is computed
/// rule) is computed off the actor; smaller ones (dragging one zoom) cost well under a millisecond.
pub const INLINE_STEPS: usize = 4_000;
const CAMERA_STEPS_PER_SEC: usize = 125;

/// What a zoom track is built for: the springs' items, each camera run's key, and the timeline's length.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomFor {
    pub items: Vec<ZoomItem>,
    pub cameras: Vec<CameraKey>,
    pub duration_us: Us,
}

pub struct ZoomTrack {
    target: ZoomFor,
    runs: Vec<springs::Run>,
    /// In time order, never overlapping.
    cameras: Vec<(CameraKey, CameraRun)>,
    /// Every run is simulated and every camera planned. A partial track (a big rebuild still running off the actor) is
    /// never reused as if it were complete: the next context builds again, reusing its runs.
    complete: bool,
}

/// A track still being built: runs and plans taken from an earlier track, and those left to make.
pub struct ZoomBuild {
    target: ZoomFor,
    springs: springs::SpringBuild,
    cameras: Vec<(camera::AutoRun, Option<Arc<klipolo_cursor::camera::Plan>>)>,
    /// A plan an earlier track had for the same span, with the key it was made for, shown while this one's is made (a
    /// take's activity grew). Kept under its own key, so a context taking this partial track never reuses it as the
    /// new plan.
    stand_ins: Vec<Option<(CameraKey, CameraRun)>>,
    inputs: camera::Inputs,
}

/// The visible zoom tracks' items, sorted by start.
fn visible(p: &Project) -> Vec<ZoomItem> {
    let mut items: Vec<ZoomItem> = p
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Zoom && !t.hidden)
        .flat_map(|t| &t.items)
        .filter_map(|i| match &i.content {
            ItemContent::Zoom { target, scale } => {
                Some((i.start_us, i.end_us(), *scale, target.clone()))
            }
            _ => None,
        })
        .collect();
    items.sort_by_key(|i| (i.0, i.1));
    items
}

/// The springs' items of `p` with the takes the plan follows in `cursors`: the Rect zooms outside camera runs, and
/// the Auto zooms over no cursor data, centred.
pub fn zoom_items(p: &Project, cursors: &CursorTracks) -> Vec<ZoomItem> {
    let starts = clip_starts(&p.timeline.main);
    camera::split(visible(p), p, &starts, &Takes::of(p, cursors)).0
}

impl ZoomBuild {
    /// `p`'s zooms with the takes the plan follows in `cursors`, taking every spring run and camera plan `previous`
    /// already made.
    pub fn new(p: &Arc<Project>, cursors: &CursorTracks, previous: Option<&ZoomTrack>) -> Self {
        let starts: Arc<[Us]> = clip_starts(&p.timeline.main).into();
        Self::of(p.clone(), starts, Arc::new(Takes::of(p, cursors)), previous).1
    }

    /// What `p`'s zooms are built for with `takes` (`starts`: `clip_starts`), and the build, reusing `previous`'s.
    pub(crate) fn of(
        p: Arc<Project>,
        starts: Arc<[Us]>,
        takes: Arc<Takes>,
        previous: Option<&ZoomTrack>,
    ) -> (ZoomFor, Self) {
        let (items, autos) = camera::split(visible(&p), &p, &starts, &takes);
        let inputs = camera::Inputs { p, starts, takes };
        let target = ZoomFor {
            cameras: autos.iter().map(|a| a.key.clone()).collect(),
            // The springs move the main footage only (`plan::clip`) and run to each run's own end, never cut by a length:
            // the key is the main track's, so a layer dragged past it reuses the track.
            duration_us: main_track_duration(&inputs.p.timeline.main),
            items,
        };
        let springs =
            springs::SpringBuild::new(&target.items, previous.map_or(&[][..], |t| &t.runs));
        let old = previous.map_or(&[][..], |t| &t.cameras);
        let mut stand_ins = Vec::with_capacity(autos.len());
        let cameras = (autos.into_iter())
            .map(|a| {
                let reused = old
                    .iter()
                    .find(|(k, _)| *k == a.key)
                    .map(|(_, r)| r.plan.clone());
                let same_span =
                    |r: &&(CameraKey, CameraRun)| (r.1.start_us, r.1.end_us) == (a.start, a.end);
                let stand_in = reused
                    .is_none()
                    .then(|| old.iter().find(same_span))
                    .flatten();
                stand_ins
                    .push(stand_in.map(|(k, r)| (k.clone(), run(a.start, a.end, r.plan.clone()))));
                (a, reused)
            })
            .collect();
        let build = Self {
            target: target.clone(),
            springs,
            cameras,
            stand_ins,
            inputs,
        };
        (target, build)
    }

    /// Built for exactly `target` (a running build of the same target needn't start again).
    pub fn targets(&self, target: &ZoomFor) -> bool {
        self.target == *target
    }

    /// What it builds for.
    pub fn target(&self) -> ZoomFor {
        self.target.clone()
    }

    /// Spring steps left to simulate, and camera runs left to plan at 125 steps a second.
    pub fn steps(&self) -> usize {
        let cameras: usize = (self.cameras.iter())
            .filter(|(_, plan)| plan.is_none())
            .map(|(a, _)| (a.end - a.start).max(0) as usize * CAMERA_STEPS_PER_SEC / 1_000_000)
            .sum();
        self.springs.steps() + cameras
    }

    /// The track with the runs and plans made so far only: the others draw unzoomed, or with the plan an earlier track
    /// had for the same span, until `finish` replaces it.
    pub fn partial(&self) -> ZoomTrack {
        let (runs, _) = self.springs.runs(false);
        let cameras = (self.cameras.iter().zip(&self.stand_ins))
            .filter_map(|((a, plan), stand_in)| match plan {
                Some(p) => Some((a.key.clone(), run(a.start, a.end, p.clone()))),
                None => (stand_in.as_ref())
                    .map(|(k, r)| (k.clone(), run(r.start_us, r.end_us, r.plan.clone()))),
            })
            .collect();
        ZoomTrack {
            target: self.target.clone(),
            runs,
            cameras,
            complete: false,
        }
    }

    /// Simulates the remaining runs and plans the remaining cameras.
    pub fn finish(self) -> ZoomTrack {
        let (runs, _) = self.springs.runs(true);
        let inputs = &self.inputs;
        let cameras = (self.cameras.into_iter())
            .map(|(a, plan)| {
                let plan = plan.unwrap_or_else(|| Arc::new(camera::plan_run(&a, inputs)));
                (a.key.clone(), run(a.start, a.end, plan))
            })
            .collect();
        ZoomTrack {
            target: self.target,
            runs,
            cameras,
            complete: true,
        }
    }
}

fn run(start_us: Us, end_us: Us, plan: Arc<klipolo_cursor::camera::Plan>) -> CameraRun {
    CameraRun {
        start_us,
        end_us,
        plan,
    }
}

impl ZoomTrack {
    pub fn new(p: &Arc<Project>, cursors: &CursorTracks) -> Self {
        ZoomBuild::new(p, cursors, None).finish()
    }

    /// The springs' items.
    pub fn items(&self) -> &[ZoomItem] {
        &self.target.items
    }

    /// The camera runs, in time order, each with its key.
    pub fn cameras(&self) -> &[(CameraKey, CameraRun)] {
        &self.cameras
    }

    /// Complete, for exactly `target`: a new context can take it as it is.
    pub fn matches(&self, target: &ZoomFor) -> bool {
        self.complete && self.target == *target
    }

    /// Built for the same target as `other`, complete or not (a finished build may replace a partial one).
    pub fn same_items(&self, other: &ZoomTrack) -> bool {
        self.target == other.target
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The visible part of the main source frame at `at_us` (source-normalized), or None when not zoomed: the camera
    /// run there, else the springs. O(log runs), no allocation.
    pub fn window(&self, at_us: Us) -> Option<BoxN> {
        let i = self.cameras.partition_point(|(_, c)| c.start_us <= at_us);
        if let Some((_, c)) = i.checked_sub(1).map(|i| &self.cameras[i])
            && at_us < c.end_us
        {
            return camera::window(&c.plan, at_us - c.start_us);
        }
        springs::window(&self.runs, at_us)
    }
}

/// Zoom inside a framing crop: same shape as `base`, scaled by the window's zoom, centred on the window, kept inside
/// the source frame.
pub fn zoom_crop(base: BoxN, window: BoxN) -> BoxN {
    let scale = (1.0 / window.w).max(1.0);
    let (w, h) = (base.w / scale, base.h / scale);
    let (cx, cy) = (window.x + window.w / 2.0, window.y + window.h / 2.0);
    BoxN {
        x: (cx - w / 2.0).clamp(0.0, 1.0 - w),
        y: (cy - h / 2.0).clamp(0.0, 1.0 - h),
        w,
        h,
    }
}
