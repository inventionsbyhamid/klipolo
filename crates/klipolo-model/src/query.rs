//! Read-only questions about a project's timeline.

use std::cmp::Reverse;
use std::collections::HashMap;

use crate::project::{Project, Recording, RecordingSource, SourceRun};
use crate::time::{scaled, unscaled, Us, OVERLAP_TOLERANCE_US};
use crate::timeline::{Attach, Clip, ClipSource, Item, Timeline, Track};

pub fn clip_starts(main: &[Clip]) -> Vec<Us> {
    let mut t: Us = 0;
    main.iter()
        .map(|c| {
            let start = t;
            t = t.saturating_add(c.duration_us());
            start
        })
        .collect()
}

/// The main track's own length: its clips back to back. Where main-track edits and the speech map stop (inserting at
/// its end, cuts, the transcript, silences, [`clip_at`]); the video itself may run longer ([`video_duration`]).
pub fn main_track_duration(main: &[Clip]) -> Us {
    main.iter()
        .map(Clip::duration_us)
        .fold(0, Us::saturating_add)
}

/// How long the video is: until the main track or the latest layer with a picture of its own ends, whichever is later
/// A project of layers alone (a blank canvas) is as long as its layers. Sound layers never make
/// it longer (a long song is cut at the video's end), nor do zooms or adjustment boxes, which only change what is below
/// them (`ItemContent::has_own_picture`); captions follow the clips. Layers on a hidden or soloed-away track count:
/// hiding is a view switch, and the length mustn't jump with it. O(clips + layers), no allocation; the per-frame paths
/// read it once per revision (`PlanContext`).
pub fn video_duration(timeline: &Timeline) -> Us {
    timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .filter(|i| i.content.has_own_picture())
        .map(Item::end_us)
        .fold(main_track_duration(&timeline.main), Us::max)
}

/// Index and start of the main clip with `start <= at < end`; None past the main track's end, where the video may
/// still go on (layers alone).
pub fn clip_at(main: &[Clip], at: Us) -> Option<(usize, Us)> {
    let mut start: Us = 0;
    for (i, c) in main.iter().enumerate() {
        let end = start.saturating_add(c.duration_us());
        if at >= start && at < end {
            return Some((i, start));
        }
        start = end;
    }
    None
}

/// Timeline position of an attachment, if its source moment is still on the main track.
///
/// Candidates, in order:
/// 1. the anchor's own clip (`clip_id`) covering `source_us` (end exclusive);
/// 2. the anchor's own clip ending exactly at `source_us`;
/// 3. another clip with the same source covering `source_us` (end exclusive): the one nearest
///    by index to the own clip, ties going to the later clip (where a split puts the new
///    piece), or the first such clip when the own clip is no longer on the track.
pub fn resolve_attach(main: &[Clip], attach: &Attach) -> Option<Us> {
    locate_attach(main, attach).map(|(_, at)| at)
}

/// Index of the clip that plays `attach` (by the rules of [`resolve_attach`]) and its timeline
/// position.
pub(crate) fn locate_attach(main: &[Clip], attach: &Attach) -> Option<(usize, Us)> {
    let own = |c: &Clip| c.id == attach.clip_id;
    let i = main
        .iter()
        .position(|c| own(c) && covers(c, attach, false))
        .or_else(|| main.iter().position(|c| own(c) && covers(c, attach, true)))
        .or_else(|| nearest_copy(main, attach, main.iter().position(own)))?;
    Some((
        i,
        position(&main[i], main_track_duration(&main[..i]), attach),
    ))
}

/// Whether `c` plays `attach`'s source moment (end exclusive unless `inclusive_end`).
fn covers(c: &Clip, attach: &Attach, inclusive_end: bool) -> bool {
    c.source == attach.source
        && attach.source_us >= c.source_in_us
        && (attach.source_us < c.source_out_us
            || (inclusive_end && attach.source_us == c.source_out_us))
}

/// Rule 3 of [`resolve_attach`]: another clip with the same source covering the moment, nearest
/// by index to the own clip at `own` (ties to the later clip), else the first such clip.
fn nearest_copy(main: &[Clip], attach: &Attach, own: Option<usize>) -> Option<usize> {
    let mut copies = main
        .iter()
        .enumerate()
        .filter(|(_, c)| covers(c, attach, false))
        .map(|(i, _)| i);
    match own {
        Some(k) => copies.min_by_key(|&i| (i.abs_diff(k), Reverse(i))),
        None => copies.next(),
    }
}

/// Timeline position of `attach` on `clip`, which starts at `start`.
pub(crate) fn position(clip: &Clip, start: Us, attach: &Attach) -> Us {
    let offset = attach.source_us.saturating_sub(clip.source_in_us);
    start.saturating_add(scaled(offset, clip.speed))
}

/// A main track's clip starts and ids, computed once to resolve many attachments.
pub(crate) struct MainIndex<'a> {
    main: &'a [Clip],
    starts: Vec<Us>,
    ids: HashMap<&'a str, usize>,
}

impl<'a> MainIndex<'a> {
    pub(crate) fn new(main: &'a [Clip]) -> Self {
        let mut ids = HashMap::with_capacity(main.len());
        for (i, c) in main.iter().enumerate() {
            ids.entry(c.id.as_str()).or_insert(i);
        }
        Self {
            main,
            starts: clip_starts(main),
            ids,
        }
    }

    /// [`locate_attach`] without scanning the track when the anchor's own clip plays it. Clip ids
    /// are unique in a valid project; with duplicates the first clip with the id is the own clip.
    pub(crate) fn locate(&self, attach: &Attach) -> Option<(usize, Us)> {
        let own = self.ids.get(attach.clip_id.as_str()).copied();
        let i = pick(self.main, attach, own)?;
        Some((i, position(&self.main[i], self.starts[i], attach)))
    }
}

/// [`MainIndex::locate`] for one anchor without building the index: O(clips), no allocation.
pub(crate) fn locate_one(main: &[Clip], attach: &Attach) -> Option<(usize, Us)> {
    let own = main.iter().position(|c| c.id == attach.clip_id);
    let i = pick(main, attach, own)?;
    Some((
        i,
        position(&main[i], main_track_duration(&main[..i]), attach),
    ))
}

/// The clip playing `attach` when `own` is the index of the first clip with its id.
pub(crate) fn pick(main: &[Clip], attach: &Attach, own: Option<usize>) -> Option<usize> {
    let plays = |inclusive_end| own.filter(|&k| covers(&main[k], attach, inclusive_end));
    plays(false)
        .or_else(|| plays(true))
        .or_else(|| nearest_copy(main, attach, own))
}

/// Attachment for timeline position `at`. The main track's very end anchors to the last clip's end; past it (where
/// only layers play) nothing anchors.
pub fn anchor_at(main: &[Clip], at: Us) -> Option<Attach> {
    if let Some((i, start)) = clip_at(main, at) {
        let c = &main[i];
        let offset = unscaled(at.saturating_sub(start), c.speed)
            .min(c.source_out_us.saturating_sub(c.source_in_us));
        return Some(Attach {
            clip_id: c.id.clone(),
            source: c.source.clone(),
            source_us: c.source_in_us.saturating_add(offset),
        });
    }
    if at == main_track_duration(main) {
        return main.last().map(|c| Attach {
            clip_id: c.id.clone(),
            source: c.source.clone(),
            source_us: c.source_out_us,
        });
    }
    None
}

/// The clip that was there before a batch which `id` is, or was cut or split from. A split's second piece and a cut's
/// later pieces get `<base>-<n>` ids (`IdPool::derive`), so suffixes are dropped one at a time until an id that `was`
/// there is reached. An id that `added` names (an `InsertClip`'s: a new clip, a copy) derives from nothing, and neither
/// do its pieces; nor does an id without such a suffix. A few hash lookups per suffix; ids carry one or two.
pub fn piece_of(
    id: &str,
    was: impl Fn(&str) -> bool,
    added: impl Fn(&str) -> bool,
) -> Option<&str> {
    let mut at = id;
    loop {
        if added(at) {
            return None;
        }
        if was(at) {
            return Some(at);
        }
        let (base, n) = at.rsplit_once('-')?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        at = base;
    }
}

pub fn source_duration_us(project: &Project, source: &ClipSource) -> Option<Us> {
    match source {
        ClipSource::Asset { id } => project.asset(id).map(|a| a.duration_us),
        ClipSource::Recording { id } => project.recording(id).map(Recording::duration_us),
    }
}

/// Whether a recording clip can be heard from source `s`: a sound role, or a picture source whose files carry sound (an
/// imported camera's own sound; Klipolo's picture files have none). One asset lookup per run, no allocation.
pub fn makes_sound(p: &Project, s: &RecordingSource) -> bool {
    !s.role.is_video()
        || s.runs
            .iter()
            .any(|run| p.asset(&run.asset_id).is_some_and(|a| a.has_audio))
}

/// The sources a recording clip can be heard from: every sound role, and a picture source whose files carry sound
/// (an imported camera's own sound; Klipolo's picture files have none). In source order. O(sources × runs) asset lookups;
/// a recording has a handful of sources, so nothing is allocated.
pub fn sound_sources<'a>(
    p: &'a Project,
    r: &'a Recording,
) -> impl Iterator<Item = &'a RecordingSource> + 'a {
    r.sources.iter().filter(move |s| makes_sound(p, s))
}

/// Where one run of a recording's source plays inside a clip: its timeline span and the run's own time at its start.
#[derive(Debug, PartialEq, Eq)]
pub struct RunInClip {
    pub start_us: Us,
    pub end_us: Us,
    pub run_in_us: Us,
}

/// Where `run` of `source` plays inside `clip` (which starts at `clip_start_us` on the timeline): its timeline span and
/// the run's own time at the span's start. None when the run doesn't overlap the clip's source range.
pub fn run_in_clip(
    clip: &Clip,
    clip_start_us: Us,
    source: &RecordingSource,
    run: &SourceRun,
) -> Option<RunInClip> {
    run_within(
        clip,
        clip_start_us,
        source,
        run,
        (clip.source_in_us, clip.source_out_us),
    )
}

/// `run_in_clip` over the clip's recording-time stretch `[from, to)` only: a true gap the camera's sound fills. None
/// when the run has nothing there.
pub fn run_within(
    clip: &Clip,
    clip_start_us: Us,
    source: &RecordingSource,
    run: &SourceRun,
    (from, to): (Us, Us),
) -> Option<RunInClip> {
    let (run_from, run_to) = run.rec_span(source.nudge_us);
    let from = run_from.max(from).max(clip.source_in_us);
    let to = run_to.min(to).min(clip.source_out_us);
    (from < to).then(|| RunInClip {
        start_us: clip_start_us.saturating_add(scaled(from - clip.source_in_us, clip.speed)),
        end_us: clip_start_us.saturating_add(scaled(to - clip.source_in_us, clip.speed)),
        run_in_us: run.local_us(from, source.nudge_us),
    })
}

/// (track index, item index) of an item.
pub fn find_item(project: &Project, id: &str) -> Option<(usize, usize)> {
    project
        .timeline
        .tracks
        .iter()
        .enumerate()
        .find_map(|(t, track)| {
            track
                .items
                .iter()
                .position(|item| item.id == id)
                .map(|i| (t, i))
        })
}

/// True when `[start, end)` is non-empty and overlaps no item on `track` (optionally ignoring one
/// item) by more than [`OVERLAP_TOLERANCE_US`]: of the range and an item, whichever starts later
/// may begin up to that much before the other ends — the same rule validation applies.
pub fn fits(track: &Track, start: Us, end: Us, ignore: Option<&str>) -> bool {
    end > start
        && track
            .items
            .iter()
            .filter(|item| Some(item.id.as_str()) != ignore)
            .all(|item| {
                if start <= item.start_us {
                    end <= item.start_us.saturating_add(OVERLAP_TOLERANCE_US)
                } else {
                    start.saturating_add(OVERLAP_TOLERANCE_US) >= item.end_us()
                }
            })
}

/// Whether sound separated from `clip`'s recording (its own sound layers, `DetachSource`) plays over `[start, end)` of
/// the timeline: the clip's speed can't change there, as its sound layers have none (sync must hold, which
/// `UpdateClip` enforces). O(layers × the recording's sound files).
pub fn sound_separated(p: &Project, clip: &Clip, start: Us, end: Us) -> bool {
    crate::apply::sound_separated(p, clip, start, end)
}
