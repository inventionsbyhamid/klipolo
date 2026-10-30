// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/zoom_spring.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted (cap_project types from vendor::cap::compat); ZoomTransformTimeline::{from_project, from_project_for_clip, from_project_for_outgoing_clip, from_project_for_recording_clip} and CursorCropMaps::from_project removed (they read Cap's ProjectConfiguration); build_time_map no longer subtracts clip transitions or splits at fullscreen-text hold windows (Klipolo's TimelineConfiguration has segments only, and Klipolo passes none); Cap's tests module removed (it builds ProjectConfiguration, StyleSegment and ClipTransition fixtures); targets_at finds the active segment by binary search over a running maximum of the sorted segments' ends, and checks instant segments only when there are any (an Auto zoom following the cursor is thousands of segments, and a scan per 8 ms step made an hour-long one quadratic).

//! Deterministic spring-simulated zoom transform timeline.
//!
//! Replaces both the duration-based easing state machine that used to live in
//! `zoom.rs` (spring_ease / spring_ease_out / instant_ease over
//! `ZOOM_DURATION`) and the cursor-following `ZoomFocusInterpolator` layer.
//!
//! Three channels — `amount` (zoom scale), `center` (2D framing center in
//! `SegmentBounds::from_amount_center` travel space) and `activity` (the 0/1
//! "any zoom active" step that drives camera scale-during-zoom) — are each
//! integrated by an analytic [`SpringMassDamperSimulation`] chasing
//! step-function targets, retargeted every 8 ms step with velocity always
//! carried across retargets. There are no fixed animation durations and no
//! boundary special-cases: segment starts/ends/re-aims are just target changes
//! the spring smooths through for continuous motion.
//!
//! The timeline lives in TIMELINE time. Cursor events are in RECORDING time,
//! so click-cluster construction and the "active cluster at time t" lookup map
//! through the project's [`TimelineConfiguration`] (identity when absent).
//!
//! Precompute is lazy (`ensure_precomputed_until`) and must happen in the same
//! mutable phase render loops already used for the focus interpolator; the
//! render hot path only calls [`ZoomTransformTimeline::sample`], which is an
//! index + lerp over the cached samples with no allocation and no locking.
//! The result is a pure function of (segments, cursor events, spring config),
//! so sequential playback and seeking produce bit-identical transforms, and
//! export matches playback by construction.

use crate::vendor::cap::compat::{
    Crop, CursorEvents, CursorMoveEvent, ScreenMovementSpring, TimelineConfiguration, XY, ZoomMode,
    ZoomSegment,
};

use crate::vendor::cap::rendering::{
    spring_mass_damper::{SpringMassDamperSimulation, SpringMassDamperSimulationConfig},
    zoom::{InterpolatedZoom, SegmentBounds},
};

/// Fixed precompute step (125 Hz), matching the sampling density the old
/// focus interpolator used.
const STEP_MS: f64 = 8.0;

/// Instant-animation segments snap all channels (no spring) while inside the
/// segment and within this window of its boundaries.
const INSTANT_SNAP_WINDOW_SECS: f64 = 0.1;

/// While the zoom amount is at (or within a hair of) identity the viewport
/// covers the whole frame regardless of the center, so the center channel is
/// free to track its target instantly — this pre-aims upcoming zooms so they
/// scale straight toward their focus. Any center jump while amount <= this
/// bound moves the viewport by at most (bound - 1) of the card: sub-pixel.
const CENTER_PREAIM_MAX_AMOUNT: f32 = 1.0005;

/// Greedy click-cluster bounding-box limits, as a fraction of the visible
/// zoomed viewport (50% width x 70% height).
const CLUSTER_WIDTH_RATIO: f64 = 0.5;
const CLUSTER_HEIGHT_RATIO: f64 = 0.7;

/// Fallback focus when a segment has no usable cursor data.
const FALLBACK_FOCUS: (f64, f64) = (0.5, 0.5);

/// Maps raw display-UV cursor coordinates into cropped-content UV space.
///
/// Cursor events are normalized to the FULL recorded display, but zoom
/// centers ([`SegmentBounds::from_amount_center`]) are proportions of the
/// rendered (cropped) content. Without this remap a cropped recording aims
/// its auto zoom at the wrong spot — and clustering dead-zone distances are
/// measured in the wrong scale. Identity when the recording is uncropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorCropMap {
    /// Crop top-left in raw display UV.
    offset: XY<f64>,
    /// Crop size in raw display UV.
    scale: XY<f64>,
}

impl CursorCropMap {
    /// `None` when the crop covers the whole screen (identity) or when the
    /// inputs are degenerate.
    pub fn from_crop(crop: &Crop, screen_size: XY<u32>) -> Option<Self> {
        if screen_size.x == 0 || screen_size.y == 0 || crop.size.x == 0 || crop.size.y == 0 {
            return None;
        }
        if crop.position.x == 0
            && crop.position.y == 0
            && crop.size.x >= screen_size.x
            && crop.size.y >= screen_size.y
        {
            return None;
        }
        let screen = XY::new(f64::from(screen_size.x), f64::from(screen_size.y));
        Some(Self {
            offset: XY::new(
                f64::from(crop.position.x) / screen.x,
                f64::from(crop.position.y) / screen.y,
            ),
            scale: XY::new(
                f64::from(crop.size.x) / screen.x,
                f64::from(crop.size.y) / screen.y,
            ),
        })
    }

    /// Raw display UV -> content UV. Positions outside the crop map outside
    /// [0, 1]; consumers clamp at the point of use so movement into a
    /// cropped-away strip still aims the camera at that content edge.
    fn map(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.offset.x) / self.scale.x,
            (y - self.offset.y) / self.scale.y,
        )
    }
}

struct TimedCursorCropMap {
    start: f64,
    end: f64,
    crop: Option<CursorCropMap>,
}

struct CursorCropMaps {
    base: Option<CursorCropMap>,
    styles: Vec<TimedCursorCropMap>,
}

struct CroppedClusters {
    crop: Option<CursorCropMap>,
    clusters: Vec<ClickCluster>,
}

struct TimedClusterVariant {
    start: f64,
    end: f64,
    variant: usize,
}

enum ZoomClusters {
    Static(Vec<ClickCluster>),
    Styled {
        variants: Vec<CroppedClusters>,
        styles: Vec<TimedClusterVariant>,
    },
}

impl ZoomClusters {
    fn new(
        cursor_events: &CursorEvents,
        recording_start: f64,
        recording_end: f64,
        segment: &ZoomSegment,
        crops: &CursorCropMaps,
    ) -> Self {
        let build = |crop| {
            build_clusters(
                cursor_events,
                recording_start,
                recording_end,
                segment.amount,
                crop,
            )
        };
        let base = build(crops.base);
        let overlaps =
            |style: &&TimedCursorCropMap| style.start <= segment.end && style.end > segment.start;
        if !crops
            .styles
            .iter()
            .filter(overlaps)
            .any(|style| style.crop != crops.base)
        {
            return Self::Static(base);
        }

        let mut variants = vec![CroppedClusters {
            crop: crops.base,
            clusters: base,
        }];
        let styles = crops
            .styles
            .iter()
            .filter(overlaps)
            .map(|style| {
                let variant = variants
                    .iter()
                    .position(|variant| variant.crop == style.crop)
                    .unwrap_or_else(|| {
                        let index = variants.len();
                        variants.push(CroppedClusters {
                            crop: style.crop,
                            clusters: build(style.crop),
                        });
                        index
                    });
                TimedClusterVariant {
                    start: style.start,
                    end: style.end,
                    variant,
                }
            })
            .collect();
        Self::Styled { variants, styles }
    }

    fn center_at(&self, timeline_secs: f64, recording_ms: f64) -> Option<(f64, f64)> {
        let clusters = match self {
            Self::Static(clusters) => clusters,
            Self::Styled { variants, styles } => {
                let variant = styles
                    .iter()
                    .rev()
                    .find(|style| timeline_secs >= style.start && timeline_secs < style.end)
                    .map_or(0, |style| style.variant);
                &variants[variant].clusters
            }
        };
        cluster_center_at_time(clusters, recording_ms)
    }
}

#[derive(Debug)]
pub(crate) struct ClickCluster {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    start_time_ms: f64,
    last_time_ms: f64,
}

impl ClickCluster {
    fn new(x: f64, y: f64, time_ms: f64) -> Self {
        Self {
            min_x: x,
            max_x: x,
            min_y: y,
            max_y: y,
            start_time_ms: time_ms,
            last_time_ms: time_ms,
        }
    }

    fn can_add(&self, x: f64, y: f64, max_w: f64, max_h: f64) -> bool {
        let new_w = self.max_x.max(x) - self.min_x.min(x);
        let new_h = self.max_y.max(y) - self.min_y.min(y);
        new_w <= max_w && new_h <= max_h
    }

    fn add(&mut self, x: f64, y: f64, time_ms: f64) {
        self.min_x = self.min_x.min(x);
        self.max_x = self.max_x.max(x);
        self.min_y = self.min_y.min(y);
        self.max_y = self.max_y.max(y);
        self.last_time_ms = time_ms;
    }

    fn center(&self) -> (f64, f64) {
        (
            (self.min_x + self.max_x) / 2.0,
            (self.min_y + self.max_y) / 2.0,
        )
    }
}

/// Greedily clusters ALL mouse movement inside a segment's RECORDING-time
/// range into bounding boxes limited to a fraction of the visible zoomed
/// viewport. Every move participates — not just clicks — so the camera
/// re-aims whenever the cursor travels outside the current cluster's
/// dead-zone box (hovering into a corner pans the view there even without a
/// click). Click positions are inherently part of the move stream, so they
/// need no separate pass.
pub(crate) fn build_clusters(
    cursor_events: &CursorEvents,
    segment_start_secs: f64,
    segment_end_secs: f64,
    zoom_amount: f64,
    crop: Option<CursorCropMap>,
) -> Vec<ClickCluster> {
    let start_ms = segment_start_secs * 1000.0;
    let end_ms = segment_end_secs * 1000.0;
    let cluster_w = CLUSTER_WIDTH_RATIO / zoom_amount.max(1.0);
    let cluster_h = CLUSTER_HEIGHT_RATIO / zoom_amount.max(1.0);
    // Clustering happens in CONTENT UV space: dead-zone box limits are
    // fractions of the visible (cropped) viewport, so raw display UVs must be
    // remapped before distances mean what the constants say they mean.
    let map_uv = |x: f64, y: f64| crop.map_or((x, y), |c| c.map(x, y));

    // Non-finite coordinates (corrupted files, synthetic event generators)
    // must never reach the cluster math: NaN propagates through min/max and
    // clamp, which would poison the spring targets for the whole timeline.
    let finite = |m: &&CursorMoveEvent| m.x.is_finite() && m.y.is_finite() && m.time_ms.is_finite();

    let events_in_range: Vec<&CursorMoveEvent> = cursor_events
        .moves
        .iter()
        .filter(finite)
        .filter(|m| m.time_ms >= start_ms && m.time_ms <= end_ms)
        .collect();

    if events_in_range.is_empty() {
        let fallback = cursor_events
            .moves
            .iter()
            .filter(finite)
            .rev()
            .find(|m| m.time_ms <= start_ms)
            .or_else(|| {
                cursor_events
                    .moves
                    .iter()
                    .filter(finite)
                    .find(|m| m.time_ms >= start_ms)
            });

        if let Some(evt) = fallback {
            let (x, y) = map_uv(evt.x, evt.y);
            return vec![ClickCluster::new(x, y, evt.time_ms)];
        }
        return vec![];
    }

    let mut clusters = Vec::new();
    let first = events_in_range[0];
    let (first_x, first_y) = map_uv(first.x, first.y);
    let mut current = ClickCluster::new(first_x, first_y, first.time_ms);

    for evt in &events_in_range[1..] {
        let (x, y) = map_uv(evt.x, evt.y);
        if current.can_add(x, y, cluster_w, cluster_h) {
            current.add(x, y, evt.time_ms);
        } else {
            clusters.push(current);
            current = ClickCluster::new(x, y, evt.time_ms);
        }
    }
    clusters.push(current);

    clusters
}

/// The active cluster at `time_ms` (RECORDING time) is the last cluster whose
/// first event time is <= t; the spring smooths the discrete re-aims.
pub(crate) fn cluster_center_at_time(
    clusters: &[ClickCluster],
    time_ms: f64,
) -> Option<(f64, f64)> {
    clusters
        .iter()
        .rev()
        .find(|c| c.start_time_ms <= time_ms)
        .or_else(|| clusters.first())
        .map(|c| c.center())
}

/// One entry of the timeline-time -> recording-time mapping derived from
/// [`TimelineConfiguration::get_segment_time`]'s accumulation logic.
#[derive(Clone, Copy)]
struct TimeMapSegment {
    timeline_start: f64,
    timeline_end: f64,
    recording_start: f64,
    timescale: f64,
    recording_clip: u32,
}

fn build_time_map(timeline: Option<&TimelineConfiguration>) -> Vec<TimeMapSegment> {
    let Some(timeline) = timeline else {
        return Vec::new();
    };

    let mut map = Vec::with_capacity(timeline.segments.len());
    let mut accum = 0.0;
    for segment in timeline.segments.iter() {
        let duration = segment.duration();
        if !duration.is_finite() || duration <= 0.0 {
            continue;
        }
        map.push(TimeMapSegment {
            timeline_start: accum,
            timeline_end: accum + duration,
            recording_start: segment.start,
            timescale: segment.timescale,
            recording_clip: segment.recording_clip,
        });
        accum += duration;
    }

    map
}

/// Maps a timeline timestamp to recording seconds. Identity when no timeline
/// is configured; clamps to the nearest edit boundary outside the timeline.
fn map_timeline_to_recording_secs(
    map: &[TimeMapSegment],
    timeline_secs: f64,
    recording_clip: Option<u32>,
    prefer_outgoing: bool,
) -> f64 {
    let Some(first) = map.first() else {
        return timeline_secs;
    };

    if timeline_secs <= first.timeline_start {
        return first.recording_start;
    }

    let contains = |segment: &&TimeMapSegment| {
        timeline_secs >= segment.timeline_start && timeline_secs < segment.timeline_end
    };
    let segment = recording_clip
        .and_then(|recording_clip| {
            if prefer_outgoing {
                map.iter()
                    .filter(contains)
                    .find(|segment| segment.recording_clip == recording_clip)
            } else {
                map.iter()
                    .rev()
                    .filter(contains)
                    .find(|segment| segment.recording_clip == recording_clip)
            }
        })
        .or_else(|| map.iter().rev().find(contains));
    if let Some(segment) = segment {
        return segment.recording_start
            + (timeline_secs - segment.timeline_start) * segment.timescale;
    }

    let last = map.last().expect("map is non-empty");
    last.recording_start + (last.timeline_end - last.timeline_start) * last.timescale
}

/// One precomputed step. Sample times are implicit: `samples[i]` is the state
/// at `i * STEP_MS`, so lookup is pure index math.
#[derive(Clone, Copy, Debug)]
struct TimelineSample {
    amount: f32,
    center: XY<f32>,
    activity: f32,
    snapped: bool,
}

struct PrecomputeState {
    /// 2D framing center in `from_amount_center` travel space.
    center_sim: SpringMassDamperSimulation,
    /// x = zoom amount, y = zoom activity (0/1 step -> smooth camera driver).
    aux_sim: SpringMassDamperSimulation,
    /// Last center target while a segment was active. Held during zoom-out so
    /// the outgoing framing stays anchored instead of re-aiming mid-flight.
    held_center_target: XY<f32>,
}

struct StepTargets {
    amount: f32,
    center: XY<f32>,
    activity: f32,
    segment_active: bool,
    snap: bool,
}

/// Deterministic, lazily precomputed zoom transform timeline.
///
/// Construction is cheap (clusters only); integration happens on demand via
/// [`Self::ensure_precomputed_until`] which must be called from a mutable
/// phase (exactly where the old focus interpolator's precompute ran) before
/// [`Self::sample`] is used for the corresponding frame times.
pub struct ZoomTransformTimeline {
    samples: Vec<TimelineSample>,
    state: Option<PrecomputeState>,
    zoom_segments: Vec<ZoomSegment>,
    /// Klipolo: parallel to `zoom_segments` (sorted by start): the largest end
    /// among it and every segment before it, for `targets_at`'s search.
    max_ends: Vec<f64>,
    /// Klipolo: whether any segment is an instant animation (Klipolo sets none).
    any_instant: bool,
    /// Parallel to `zoom_segments`: prebuilt clusters (RECORDING-time ms) for
    /// Auto segments, `None` for Manual ones.
    clusters: Vec<Option<ZoomClusters>>,
    time_map: Vec<TimeMapSegment>,
    recording_clip: Option<u32>,
    prefer_outgoing: bool,
    /// Total number of samples covering [0, duration] plus one lerp partner.
    total_samples: usize,
}

struct RecordingClipSelection {
    recording_clip: Option<u32>,
    prefer_outgoing: bool,
}

impl ZoomTransformTimeline {
    pub fn new(
        zoom_segments: &[ZoomSegment],
        timeline: Option<&TimelineConfiguration>,
        cursor_events: &CursorEvents,
        spring: ScreenMovementSpring,
        duration_secs: f64,
        crop: Option<CursorCropMap>,
    ) -> Self {
        Self::new_for_recording_clip(
            zoom_segments,
            timeline,
            cursor_events,
            spring,
            duration_secs,
            CursorCropMaps {
                base: crop,
                styles: Vec::new(),
            },
            RecordingClipSelection {
                recording_clip: None,
                prefer_outgoing: false,
            },
        )
    }

    fn new_for_recording_clip(
        zoom_segments: &[ZoomSegment],
        timeline: Option<&TimelineConfiguration>,
        cursor_events: &CursorEvents,
        spring: ScreenMovementSpring,
        duration_secs: f64,
        crops: CursorCropMaps,
        selection: RecordingClipSelection,
    ) -> Self {
        let RecordingClipSelection {
            recording_clip,
            prefer_outgoing,
        } = selection;
        let mut zoom_segments = zoom_segments.to_vec();
        zoom_segments.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
        let max_ends: Vec<f64> = zoom_segments
            .iter()
            .scan(f64::NEG_INFINITY, |end, s| {
                *end = end.max(s.end);
                Some(*end)
            })
            .collect();
        let any_instant = zoom_segments.iter().any(|s| s.instant_animation);

        let time_map = if zoom_segments.is_empty() {
            Vec::new()
        } else {
            build_time_map(timeline)
        };
        let clusters = zoom_segments
            .iter()
            .map(|segment| match segment.mode {
                ZoomMode::Auto => {
                    let recording_start = map_timeline_to_recording_secs(
                        &time_map,
                        segment.start,
                        recording_clip,
                        prefer_outgoing,
                    );
                    let recording_end = map_timeline_to_recording_secs(
                        &time_map,
                        segment.end,
                        recording_clip,
                        prefer_outgoing,
                    )
                    .max(recording_start);
                    Some(ZoomClusters::new(
                        cursor_events,
                        recording_start,
                        recording_end,
                        segment,
                        &crops,
                    ))
                }
                ZoomMode::Manual { .. } => None,
            })
            .collect();

        let duration_secs = if duration_secs.is_finite() {
            duration_secs.max(0.0)
        } else {
            0.0
        };
        // One sample per step across the duration, plus one trailing sample so
        // a lookup right at the end always has a lerp partner.
        let total_samples = (duration_secs * 1000.0 / STEP_MS).ceil() as usize + 2;
        if zoom_segments.is_empty() {
            return Self {
                samples: Vec::new(),
                state: None,
                zoom_segments,
                max_ends,
                any_instant,
                clusters,
                time_map,
                recording_clip,
                prefer_outgoing,
                total_samples: 1,
            };
        }

        let spring_config = SpringMassDamperSimulationConfig {
            tension: spring.stiffness,
            mass: spring.mass,
            friction: spring.damping,
        };

        let mut timeline = Self {
            samples: Vec::new(),
            state: None,
            zoom_segments,
            max_ends,
            any_instant,
            clusters,
            time_map,
            recording_clip,
            prefer_outgoing,
            total_samples,
        };

        // Seed the simulations at rest on the t=0 target so the very first
        // frame is already correct and `samples` is never empty.
        let initial = timeline.targets_at(0.0, XY::new(0.5, 0.5));
        let mut center_sim = SpringMassDamperSimulation::new(spring_config);
        center_sim.set_position(initial.center);
        center_sim.set_velocity(XY::new(0.0, 0.0));
        center_sim.set_target_position(initial.center);

        let mut aux_sim = SpringMassDamperSimulation::new(spring_config);
        aux_sim.set_position(XY::new(initial.amount, initial.activity));
        aux_sim.set_velocity(XY::new(0.0, 0.0));
        aux_sim.set_target_position(XY::new(initial.amount, initial.activity));

        timeline.samples.push(TimelineSample {
            amount: initial.amount.max(1.0),
            center: XY::new(
                initial.center.x.clamp(0.0, 1.0),
                initial.center.y.clamp(0.0, 1.0),
            ),
            activity: initial.activity.clamp(0.0, 1.0),
            snapped: false,
        });
        timeline.state = Some(PrecomputeState {
            center_sim,
            aux_sim,
            held_center_target: initial.center,
        });
        timeline
    }

    /// Extends the precomputed cache to cover `timeline_secs`. Amortized and
    /// cheap (125 trivial steps per second of content); a no-op once the
    /// requested range — or the whole duration — is covered.
    pub fn ensure_precomputed_until(&mut self, timeline_secs: f32) {
        if self.state.is_none() {
            return;
        }
        let need_ms = (f64::from(timeline_secs).max(0.0)) * 1000.0;
        // +2: floor index plus its lerp partner.
        let need_samples = ((need_ms / STEP_MS).ceil() as usize + 2).min(self.total_samples);
        while self.samples.len() < need_samples && self.state.is_some() {
            self.advance_one_step();
        }
    }

    /// Precomputes the full duration.
    pub fn precompute(&mut self) {
        while self.state.is_some() {
            self.advance_one_step();
        }
    }

    /// Samples the transform at a TIMELINE timestamp: binary index + lerp over
    /// the precomputed steps. No allocation, no locks, no simulation work —
    /// safe for the render hot path. Times outside the precomputed range clamp
    /// to the nearest cached sample.
    pub fn sample(&self, timeline_secs: f32) -> InterpolatedZoom {
        let Some(last) = self.samples.len().checked_sub(1) else {
            return InterpolatedZoom {
                t: 0.0,
                bounds: SegmentBounds::default(),
            };
        };

        let pos = (f64::from(timeline_secs).max(0.0)) * 1000.0 / STEP_MS;
        let idx = (pos as usize).min(last);
        let next = (idx + 1).min(last);
        let frac = (pos - idx as f64).clamp(0.0, 1.0) as f32;

        let a = self.samples[idx];
        let b = self.samples[next];

        let amount = a.amount + (b.amount - a.amount) * frac;
        let center_x = a.center.x + (b.center.x - a.center.x) * frac;
        let center_y = a.center.y + (b.center.y - a.center.y) * frac;
        let activity = a.activity + (b.activity - a.activity) * frac;

        InterpolatedZoom {
            t: f64::from(activity).clamp(0.0, 1.0),
            bounds: SegmentBounds::from_amount_center(
                f64::from(amount),
                XY::new(f64::from(center_x), f64::from(center_y)),
            ),
        }
    }

    /// Whether any precomputed step in `[from_secs, to_secs]` was an instant
    /// snap (no spring). Motion-effect consumers can use this to suppress
    /// velocity-derived effects across intentional discontinuities.
    pub fn snapped_within(&self, from_secs: f32, to_secs: f32) -> bool {
        if self.samples.is_empty() {
            return false;
        }
        let last = self.samples.len() - 1;
        let lo_ms = f64::from(from_secs.min(to_secs)).max(0.0) * 1000.0;
        let hi_ms = f64::from(from_secs.max(to_secs)).max(0.0) * 1000.0;
        let lo = ((lo_ms / STEP_MS) as usize).min(last);
        let hi = ((hi_ms / STEP_MS).ceil() as usize).min(last);
        self.samples[lo..=hi].iter().any(|s| s.snapped)
    }

    fn advance_one_step(&mut self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let held_center = state.held_center_target;

        let step_index = self.samples.len();
        let step_secs = step_index as f64 * STEP_MS / 1000.0;
        let targets = self.targets_at(step_secs, held_center);

        let Some(state) = self.state.as_mut() else {
            return;
        };

        if targets.segment_active {
            state.held_center_target = targets.center;
        }

        state.center_sim.set_target_position(targets.center);
        state
            .aux_sim
            .set_target_position(XY::new(targets.amount, targets.activity));

        if targets.snap {
            // Instant animation: park the springs on the target with zero
            // velocity so no motion (or motion-derived effect) leaks through.
            state.center_sim.set_position(targets.center);
            state.center_sim.set_velocity(XY::new(0.0, 0.0));
            state
                .aux_sim
                .set_position(XY::new(targets.amount, targets.activity));
            state.aux_sim.set_velocity(XY::new(0.0, 0.0));
        } else {
            // While the amount spring sits at identity the viewport shows the
            // whole frame no matter where the center is — the center channel
            // is unobservable, so track its target instantly (free pre-aim).
            // An incoming zoom then launches already aimed at its focus and
            // scales straight toward it, instead of zooming about the stale
            // center and dragging over at high magnification (a huge late pan
            // that also detonated the motion blur). The epsilon bounds any
            // theoretical pop to sub-pixel: (amount - 1) * |center jump| of
            // the card, i.e. < 0.05% of the card size.
            if state.aux_sim.position.x <= CENTER_PREAIM_MAX_AMOUNT {
                state.center_sim.set_position(targets.center);
                state.center_sim.set_velocity(XY::new(0.0, 0.0));
            }
            state.center_sim.run(STEP_MS as f32);
            state.aux_sim.run(STEP_MS as f32);
        }

        // Geometric safety: a sprung amount below 1 would show out-of-bounds
        // background ("bounce-out"), so clamp and kill velocity on that axis
        // only. Centers are clamped per-sample instead (the spring target is
        // always in-band, so overshoot is transient and tiny).
        if state.aux_sim.position.x < 1.0 {
            state.aux_sim.position.x = 1.0;
            state.aux_sim.velocity.x = 0.0;
        }

        self.samples.push(TimelineSample {
            amount: state.aux_sim.position.x,
            center: XY::new(
                state.center_sim.position.x.clamp(0.0, 1.0),
                state.center_sim.position.y.clamp(0.0, 1.0),
            ),
            activity: state.aux_sim.position.y.clamp(0.0, 1.0),
            snapped: targets.snap,
        });

        if self.samples.len() >= self.total_samples {
            self.state = None;
        }
    }

    fn targets_at(&self, timeline_secs: f64, held_center: XY<f32>) -> StepTargets {
        // Same active predicate `SegmentsCursor` used: (start, end], the first
        // such segment. Klipolo: by binary search. Segments are sorted by start, so
        // those that start before `t` are a prefix; the first segment whose end
        // reaches `t` is the first whose running maximum of ends does, and it is
        // active exactly when it is in that prefix.
        let started = self
            .zoom_segments
            .partition_point(|s| s.start < timeline_secs);
        let reaching = self.max_ends.partition_point(|&end| end < timeline_secs);
        let active = (reaching < started).then_some(reaching);

        let snap = self.any_instant
            && self.zoom_segments.iter().any(|s| {
                s.instant_animation
                    && timeline_secs >= s.start - INSTANT_SNAP_WINDOW_SECS
                    && timeline_secs <= s.end + INSTANT_SNAP_WINDOW_SECS
            });

        match active {
            Some(index) => {
                let segment = &self.zoom_segments[index];
                let amount = if segment.amount.is_finite() {
                    segment.amount.max(1.0)
                } else {
                    1.0
                };
                let center = match segment.mode {
                    ZoomMode::Manual { x, y } => {
                        (f64::from(x).clamp(0.0, 1.0), f64::from(y).clamp(0.0, 1.0))
                    }
                    ZoomMode::Auto => {
                        let recording_ms = map_timeline_to_recording_secs(
                            &self.time_map,
                            timeline_secs,
                            self.recording_clip,
                            self.prefer_outgoing,
                        ) * 1000.0;
                        let focus = self.clusters[index]
                            .as_ref()
                            .and_then(|clusters| clusters.center_at(timeline_secs, recording_ms))
                            .unwrap_or(FALLBACK_FOCUS);
                        SegmentBounds::calculate_follow_center(
                            (focus.0.clamp(0.0, 1.0), focus.1.clamp(0.0, 1.0)),
                            segment.edge_snap_ratio,
                        )
                    }
                };

                StepTargets {
                    amount: amount as f32,
                    center: XY::new(center.0 as f32, center.1 as f32),
                    activity: 1.0,
                    segment_active: true,
                    snap,
                }
            }
            None => StepTargets {
                amount: 1.0,
                // Hold the last active framing while zooming out so the
                // outgoing shot stays anchored (irrelevant once amount = 1).
                center: held_center,
                activity: 0.0,
                segment_active: false,
                snap,
            },
        }
    }
}
