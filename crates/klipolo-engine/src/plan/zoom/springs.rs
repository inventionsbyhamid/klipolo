//! The spring runs: Rect zooms become Cap zoom segments in timeline seconds, and Cap's spring timeline (vendored)
//! smooths every move. Items closer than `SETTLE_US` form a run that is simulated on its own, from rest at its first
//! start to `SETTLE_US` after its last end; outside every run nothing is zoomed. A run is kept by its items relative to
//! its start, so one moved in time as a whole (a ripple) is reused.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use klipolo_model::layout::BoxN;
use klipolo_model::time::{Us, us_to_secs};
use klipolo_model::timeline::ZoomTarget;

use super::{SETTLE_US, ZoomItem};
use crate::vendor::cap::compat::{
    CursorEvents, GlideDirection, ScreenMovementSpring, ZoomMode, ZoomSegment,
};
use crate::vendor::cap::rendering::zoom_spring::ZoomTransformTimeline;

const STEPS_PER_SEC: usize = 125;

/// Items closer than `SETTLE_US`, simulated together from `start_us` (their first start) to `end_us`.
pub(super) struct Run {
    /// Items relative to `start_us`.
    key: Vec<ZoomItem>,
    start_us: Us,
    end_us: Us,
    timeline: Arc<ZoomTransformTimeline>,
}

/// Splits zooms sorted by start into runs, as index ranges: a zoom joins the run while it starts less than `SETTLE_US`
/// after every earlier zoom in it has ended. The springs carry through a run, so each zoom in it can change the picture
/// until `SETTLE_US` after the run's last end. `span` gives a zoom's (start, end). O(n).
pub fn zoom_runs<T>(sorted: &[T], span: impl Fn(&T) -> (Us, Us)) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut last_end = span(&sorted[i]).1;
        let mut j = i + 1;
        while let Some((start, end)) = sorted.get(j).map(&span) {
            if start >= last_end + SETTLE_US {
                break;
            }
            last_end = last_end.max(end);
            j += 1;
        }
        out.push(i..j);
        i = j;
    }
    out
}

/// Cap's "travel space" centre: 0 pins the viewport to the left/top edge, 1 to the right/bottom.
fn travel(centre: f64, scale: f64) -> f32 {
    let view = 1.0 / scale;
    if view >= 0.9999 {
        0.5
    } else {
        ((centre - view / 2.0) / (1.0 - view)).clamp(0.0, 1.0) as f32
    }
}

fn key_hash(key: &[ZoomItem]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (start, end, scale, target) in key {
        (start, end, scale.to_bits()).hash(&mut h);
        if let ZoomTarget::Rect { rect } = target {
            [rect.x, rect.y, rect.w, rect.h]
                .map(f64::to_bits)
                .hash(&mut h);
        }
    }
    h.finish()
}

/// The spring timeline of one run (items relative to its start).
fn simulate(key: &[ZoomItem]) -> Arc<ZoomTransformTimeline> {
    let mut segments = Vec::with_capacity(key.len());
    segments.extend(key.iter().map(|(start, end, scale, target)| {
        let (cx, cy) = match target {
            ZoomTarget::Auto { .. } => (0.5, 0.5),
            ZoomTarget::Rect { rect } | ZoomTarget::Push { rect, .. } => {
                (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
            }
        };
        ZoomSegment {
            start: us_to_secs(*start),
            end: us_to_secs(*end),
            amount: *scale,
            mode: ZoomMode::Manual {
                x: travel(cx, *scale),
                y: travel(cy, *scale),
            },
            glide_direction: GlideDirection::default(),
            glide_speed: 0.5,
            instant_animation: false,
            edge_snap_ratio: 0.25,
        }
    }));
    let span = key.iter().map(|i| i.1).max().unwrap_or(0) + SETTLE_US;
    let mut timeline = ZoomTransformTimeline::new(
        &segments,
        None,
        &CursorEvents::default(),
        ScreenMovementSpring::default(),
        us_to_secs(span),
        None,
    );
    timeline.precompute();
    Arc::new(timeline)
}

/// Runs being built: those taken from an earlier track, and those left to simulate.
pub(super) struct SpringBuild {
    runs: Vec<(Vec<ZoomItem>, Us, Option<Arc<ZoomTransformTimeline>>)>,
}

impl SpringBuild {
    /// `items` (sorted by start) split into runs, taking every run `previous` already simulated.
    pub(super) fn new(items: &[ZoomItem], previous: &[Run]) -> Self {
        let mut known: HashMap<u64, Vec<&Run>> = HashMap::new();
        for run in previous {
            known.entry(key_hash(&run.key)).or_default().push(run);
        }
        let mut runs = Vec::new();
        for run in zoom_runs(items, |i| (i.0, i.1)) {
            let start = items[run.start].0;
            let mut key = Vec::with_capacity(run.len());
            key.extend(
                items[run]
                    .iter()
                    .map(|(s, e, scale, target)| (s - start, e - start, *scale, target.clone())),
            );
            let reused = known
                .get(&key_hash(&key))
                .and_then(|runs| runs.iter().find(|r| r.key == key))
                .map(|r| r.timeline.clone());
            runs.push((key, start, reused));
        }
        Self { runs }
    }

    /// Spring steps left to simulate.
    pub(super) fn steps(&self) -> usize {
        self.runs
            .iter()
            .filter(|(_, _, done)| done.is_none())
            .map(|(key, _, _)| {
                let span = key.iter().map(|i| i.1).max().unwrap_or(0) + SETTLE_US;
                span as usize * STEPS_PER_SEC / 1_000_000
            })
            .sum()
    }

    /// The runs, simulating those left when `simulate` is true; with whether every run is in.
    pub(super) fn runs(&self, simulate_left: bool) -> (Vec<Run>, bool) {
        let mut runs = Vec::with_capacity(self.runs.len());
        let mut complete = true;
        for (key, start, done) in &self.runs {
            let Some(timeline) = done
                .clone()
                .or_else(|| simulate_left.then(|| simulate(key)))
            else {
                complete = false;
                continue;
            };
            let span = key.iter().map(|i| i.1).max().unwrap_or(0) + SETTLE_US;
            runs.push(Run {
                key: key.clone(),
                start_us: *start,
                end_us: start + span,
                timeline,
            });
        }
        (runs, complete)
    }
}

/// The visible part of the main source frame at `at_us` in `runs` (source-normalized), or None when not zoomed.
pub(super) fn window(runs: &[Run], at_us: Us) -> Option<BoxN> {
    let i = runs
        .partition_point(|r| r.start_us <= at_us)
        .checked_sub(1)?;
    let run = &runs[i];
    if at_us >= run.end_us {
        return None;
    }
    let z = run.timeline.sample(us_to_secs(at_us - run.start_us) as f32);
    let (tl, br) = (z.bounds.top_left, z.bounds.bottom_right);
    let (w, h) = (br.x - tl.x, br.y - tl.y);
    if w <= 1.0005 && h <= 1.0005 {
        return None;
    }
    Some(BoxN {
        x: -tl.x / w,
        y: -tl.y / h,
        w: 1.0 / w,
        h: 1.0 / h,
    })
}
