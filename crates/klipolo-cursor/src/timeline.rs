//! Every take's facts on the timeline: one mapping through the screen's sync nudge,
//! clip speed and cuts. Its types are the planner context's (`klipolo-actions` re-exports them), so the engine's camera
//! and nothing converts on each call.
//!
//! A fact at recording time `t` shows with the screen's frame there, which the screen's nudge moved to `t + nudge`;
//! a recording clip plays it when that falls in its source range, `speed` times faster.

use std::sync::Arc;

use klipolo_model::project::{Project, Recording};
use klipolo_model::query::clip_starts;
use klipolo_model::time::unscaled;
use serde::{Deserialize, Serialize};

use crate::facts::{Dwell, StillRuns, dwells, spans};
use crate::{CursorTrack, Press, Rect01, RegionSpan, Us};

mod bounded;
mod map;

use bounded::kept;
use map::{put, window};

/// A click where it plays on the timeline, and where it was in the main footage's frame (0–1 from the top left).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClickAt {
    pub timeline_us: Us,
    pub x: f32,
    pub y: f32,
}

/// A pause on the timeline, and where the cursor rested.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanAt {
    pub from_us: Us,
    pub to_us: Us,
    pub x: f32,
    pub y: f32,
}

/// A part of the picture (typing, playing media) over a stretch of the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionAt {
    pub from_us: Us,
    pub to_us: Us,
    pub rect: Rect01,
}

/// Every take's facts on the timeline, in timeline order, at most `MAX_FACTS` of each kind. Read from JSON (a planner
/// context), each kind is held to that too (`bounded`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineFacts {
    #[serde(deserialize_with = "kept")]
    pub clicks: Vec<ClickAt>,
    #[serde(deserialize_with = "kept")]
    pub dwells: Vec<SpanAt>,
    #[serde(deserialize_with = "kept")]
    pub typing: Vec<RegionAt>,
    #[serde(deserialize_with = "kept")]
    pub media: Vec<RegionAt>,
    /// The cursor still, the screen quiet and the sound quiet, and no typing (where screen and sound are known).
    #[serde(deserialize_with = "kept")]
    pub idle: Vec<(Us, Us)>,
    /// Every take on the timeline gave its screen activity (typing, media, quiet), and its sound's quiet.
    pub activity_known: bool,
    pub sound_known: bool,
}

pub const MAX_FACTS: usize = 100_000;

/// One take's facts on its recording clock (the app and the engine cache these per take). Each list is sorted by time,
/// and a kind's spans don't overlap. None: not known yet (the activity pass or the sound hasn't run). The screen's
/// typing, media and quiet come from each of its files, shifted by that file's `SourceRun::offset_us` (the
/// `activity` notes). Clone, so the engine adds a take's activity to its cursor facts without building them again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TakeFacts {
    /// The clicks: primary-button presses (`CursorTrack::clicks`).
    pub presses: Vec<Press>,
    pub dwells: Vec<Dwell>,
    pub still: Vec<(Us, Us)>,
    pub typing: Option<Vec<RegionSpan>>,
    pub media: Option<Vec<RegionSpan>>,
    pub quiet_screen: Option<Vec<(Us, Us)>>,
    pub quiet_sound: Option<Vec<(Us, Us)>>,
}

impl TakeFacts {
    /// What the cursor file alone gives (clicks, dwells, still runs); the caller adds the activity and the sound.
    /// O(runs × log samples + presses).
    pub fn of_cursor(track: &CursorTrack, still: &StillRuns) -> Self {
        Self {
            presses: track.clicks().copied().collect(),
            dwells: dwells(track, still),
            still: still.runs().to_vec(),
            ..Self::default()
        }
    }
}

/// The facts of every recording clip on the main track, mapped through its nudge, speed and cuts. A take that `take`
/// can't give is skipped. O(clips × log facts + facts kept).
pub fn on_timeline(
    p: &Project,
    take: &dyn Fn(&Recording) -> Option<Arc<TakeFacts>>,
    min_idle_us: Us,
) -> TimelineFacts {
    let starts = clip_starts(&p.timeline.main);
    mapped(p, &starts, take, 0..starts.len(), Some(min_idle_us))
}

/// `on_timeline` of only the clips touching `[from, to]` (each start and end included), with no idle stretches: what
/// the engine's camera plans one Auto zoom from. A fact starting in `[from, to)` is `on_timeline`'s to the bit
/// (up to `MAX_FACTS`), but for its end: one reaching past `to` may end at the last of these clips, which a plan over
/// the span reads the same. The clip ending at `from` is read, so a fact running in from before keeps its earlier
/// start. `starts` is `clip_starts` of the main track. O(log clips + (clips touching) × log facts + facts kept).
pub fn on_timeline_between(
    p: &Project,
    starts: &[Us],
    take: &dyn Fn(&Recording) -> Option<Arc<TakeFacts>>,
    from: Us,
    to: Us,
) -> TimelineFacts {
    // Clips lie end to end: the last starting before `from` touches it (it holds it, or ends there).
    let first = starts.partition_point(|&s| s < from).saturating_sub(1);
    let end = starts.partition_point(|&s| s <= to);
    mapped(p, starts, take, first.min(end)..end, None)
}

/// The facts of main clips `clips`, with idle stretches of at least `min_idle_us` when asked.
fn mapped(
    p: &Project,
    starts: &[Us],
    take: &dyn Fn(&Recording) -> Option<Arc<TakeFacts>>,
    clips: std::ops::Range<usize>,
    min_idle_us: Option<Us>,
) -> TimelineFacts {
    let mut out = TimelineFacts {
        activity_known: true,
        sound_known: true,
        ..TimelineFacts::default()
    };
    let mut any = false;
    // Idle's inputs on the timeline, only from takes that know both their screen and their sound.
    let (mut still, mut screen, mut sound) = (Vec::new(), Vec::new(), Vec::new());
    let main = &p.timeline.main;
    for (clip, &start) in main[clips.clone()].iter().zip(&starts[clips]) {
        let Some((r, w)) = window(p, clip, start) else {
            continue;
        };
        let Some(f) = take(r) else {
            continue;
        };
        any = true;
        out.activity_known &= f.typing.is_some() && f.media.is_some() && f.quiet_screen.is_some();
        out.sound_known &= f.quiet_sound.is_some();
        for c in &f.presses[w.first(&f.presses, |c| c.down)..] {
            let Some(timeline_us) = w.point(c.down) else {
                break;
            };
            if !put(
                &mut out.clicks,
                ClickAt {
                    timeline_us,
                    x: c.x,
                    y: c.y,
                },
            ) {
                break;
            }
        }
        let dwell = |(from_us, to_us), d: &Dwell| SpanAt {
            from_us,
            to_us,
            x: d.x,
            y: d.y,
        };
        w.spans(&f.dwells, |d| (d.from, d.to), dwell, &mut out.dwells);
        let region = |(from_us, to_us), r: &RegionSpan| RegionAt {
            from_us,
            to_us,
            rect: r.rect,
        };
        for (regions, into) in [(&f.typing, &mut out.typing), (&f.media, &mut out.media)] {
            if let Some(regions) = regions {
                w.spans(regions, |r| (r.from, r.to), region, into);
            }
        }
        if let (Some(quiet_screen), Some(quiet_sound), Some(_)) =
            (&f.quiet_screen, &f.quiet_sound, min_idle_us)
        {
            let span = |s: (Us, Us), _: &(Us, Us)| s;
            w.spans(&f.still, |s| *s, span, &mut still);
            w.spans(quiet_screen, |s| *s, span, &mut screen);
            w.spans(quiet_sound, |s| *s, span, &mut sound);
        }
    }
    if !any {
        return TimelineFacts::default();
    }
    if let Some(min_idle_us) = min_idle_us {
        let typing: Vec<(Us, Us)> = out.typing.iter().map(|r| (r.from_us, r.to_us)).collect();
        out.idle = spans::idle(&still, &screen, &sound, &typing, min_idle_us);
        out.idle.truncate(MAX_FACTS);
    }
    out
}

/// The raw cursor at timeline `t` (the glide's input): the clip under `t`, its source time minus the screen's
/// nudge, then `track.position`. `starts` is `clip_starts` of the main track, computed once for many calls.
/// O(log clips + log samples), plus `Project::recording` (a scan of the recordings by id) and the caller's `track`
/// lookup each call: a caller sampling one clip many times resolves its track once.
pub fn cursor_at(
    p: &Project,
    starts: &[Us],
    track: &dyn Fn(&Recording) -> Option<Arc<CursorTrack>>,
    t: Us,
) -> Option<(f32, f32)> {
    let i = starts.partition_point(|&s| s <= t).checked_sub(1)?;
    let clip = p.timeline.main.get(i)?;
    if t >= starts[i].saturating_add(clip.duration_us()) {
        return None;
    }
    let (r, w) = window(p, clip, starts[i])?;
    let source = clip.source_in_us + unscaled(t - starts[i], clip.speed);
    track(r)?.position(source.saturating_sub(w.nudge))
}

/// The raw cursor over `[from, to]` of the timeline, for the camera's 60 Hz glide: `cursor_at`, with each clip's
/// take looked up once up front, not on every read. It owns what it reads, so a build off the
/// actor can take it along. O(clips touching) to make, 64 B and a shared track a clip.
pub struct RawCursor {
    /// The clips with a track, in timeline order.
    clips: Vec<RawClip>,
}

struct RawClip {
    start: Us,
    end: Us,
    source_in: Us,
    speed: f64,
    nudge: Us,
    track: Arc<CursorTrack>,
}

impl RawCursor {
    /// The clips touching `[from, to]` whose take `track` gives. `starts` is `clip_starts` of the main track.
    pub fn new(
        p: &Project,
        starts: &[Us],
        track: &dyn Fn(&Recording) -> Option<Arc<CursorTrack>>,
        from: Us,
        to: Us,
    ) -> Self {
        let main = &p.timeline.main;
        let (first, end) = (
            starts.partition_point(|&s| s <= from).saturating_sub(1),
            starts.partition_point(|&s| s <= to),
        );
        let clips = (main[first.min(end)..end]
            .iter()
            .zip(&starts[first.min(end)..end]))
        .filter_map(|(clip, &start)| {
            let (r, w) = window(p, clip, start)?;
            Some(RawClip {
                start,
                end: start.saturating_add(clip.duration_us()),
                source_in: clip.source_in_us,
                speed: clip.speed,
                nudge: w.nudge,
                track: track(r)?,
            })
        })
        .collect();
        Self { clips }
    }

    /// `cursor_at` at timeline `t` (None outside the clips it was made for). O(log clips + log samples), no allocation.
    pub fn at(&self, t: Us) -> Option<(f32, f32)> {
        let c = &self.clips[self
            .clips
            .partition_point(|c| c.start <= t)
            .checked_sub(1)?];
        if t >= c.end {
            return None;
        }
        let source = c.source_in + unscaled(t - c.start, c.speed);
        c.track.position(source.saturating_sub(c.nudge))
    }
}

impl TakeFacts {
    /// These facts with the take's screen activity added: each screen file's typing (read against the cursor's still
    /// runs), playing media and quiet stretches of at least `quiet_min_us`, moved to the recording clock by the file's
    /// offset (`screens`: each file's `SourceRun::offset_us` and track, in order). Runs never overlap, so each list
    /// stays sorted and disjoint. With no file at all the activity stays unknown (None). O(ticks + still runs).
    pub fn with_screens(
        mut self,
        still: &StillRuns,
        screens: &[(Us, &crate::activity::ActivityTrack)],
        quiet_min_us: Us,
    ) -> Self {
        use crate::activity::{media, typing};
        if screens.is_empty() {
            return self;
        }
        let moved = |o: Us, r: RegionSpan| RegionSpan {
            from: r.from.saturating_add(o),
            to: r.to.saturating_add(o),
            ..r
        };
        let (mut typed, mut playing, mut quiet) = (Vec::new(), Vec::new(), Vec::new());
        for &(o, a) in screens {
            typed.extend(typing(a, still, o).into_iter().map(|r| moved(o, r)));
            playing.extend(media(a).into_iter().map(|r| moved(o, r)));
            let shift = |(a, b): (Us, Us)| (a.saturating_add(o), b.saturating_add(o));
            quiet.extend(a.quiet_spans(quiet_min_us).into_iter().map(shift));
        }
        (self.typing, self.media, self.quiet_screen) = (Some(typed), Some(playing), Some(quiet));
        self
    }
}
