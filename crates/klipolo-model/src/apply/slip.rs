//! A source's separated layers follow its sync (sync must hold): each sound or video layer tied to the
//! source is slipped in its file, keeping its place on the timeline, and cut back where the file ends. A nudge slips
//! them by its change (`slip_separated`); Sync's re-time moves each to where the new runs play the recording moment it
//! played (`follow_runs`), keeping its cuts, fades, extensions and volume.
//!
//! **Tied:** the layer's own record, `ItemContent::tie`, set when `DetachSource` separates it or Sync
//! lines it up, kept by splits and edge trims, dropped by a hand slip of its file time. Never time proximity: a J-cut
//! lead over the previous clip and the pieces Sync keeps over B-roll follow; a reused stretch or a voice-over of the
//! file, which nothing tied, is left alone.

use crate::error::{invalid, invalid_with, Result};
use crate::ids::Id;
use crate::messages::{code, msg};
use crate::project::{Project, RecordingSource, SourceRun};
use crate::time::{drifted, unscaled, Us, MIN_DURATION_US};
use crate::timeline::{Item, ItemContent, Tie};

use super::item_ops::{fit_fades, fit_keys, shifted_anchor, start_moved};

/// How a layer's file time runs against the timeline: a sound layer's at its device's rate, a video layer's at its
/// speed.
#[derive(Clone, Copy)]
enum Pace {
    Drift(f64),
    Speed(f64),
}

impl Pace {
    /// File µs per timeline µs.
    fn per_us(self) -> f64 {
        match self {
            Self::Drift(ppm) => 1.0 + ppm * 1e-6,
            Self::Speed(speed) => speed,
        }
    }

    /// The file time `us` timeline µs of the layer play.
    fn footage(self, us: Us) -> Us {
        match self {
            Self::Drift(ppm) => drifted(us, ppm),
            Self::Speed(speed) => unscaled(us, speed),
        }
    }
}

/// A sound or video layer: its file, where it plays from, and its pace.
fn playing(item: &Item) -> Option<(&Id, Us, Pace)> {
    match &item.content {
        ItemContent::Audio {
            asset_id,
            source_in_us,
            drift_ppm,
            ..
        } => Some((asset_id, *source_in_us, Pace::Drift(*drift_ppm))),
        ItemContent::Video {
            asset_id,
            source_in_us,
            speed,
            ..
        } => Some((asset_id, *source_in_us, Pace::Speed(*speed))),
        _ => None,
    }
}

/// A layer tied to a source: where it is (track, index).
pub(super) struct Tied {
    at: (usize, usize),
}

/// The layers tied to `source` of recording `recording` (see the module), found before anything changes. O(layers).
pub(super) fn tied(p: &Project, recording: &str, source: &RecordingSource) -> Vec<Tied> {
    let tie = Tie {
        recording: recording.to_owned(),
        source: source.id.clone(),
    };
    let mut out = Vec::new();
    for (t, track) in p.timeline.tracks.iter().enumerate() {
        for (i, item) in track.items.iter().enumerate() {
            if item.content.tie() == Some(&tie) {
                out.push(Tied { at: (t, i) });
            }
        }
    }
    out
}

/// A layer's slip: where it is (track, index), its new start, length and file time, and a sound layer's new drift.
struct Slip {
    at: (usize, usize),
    start_us: Us,
    duration_us: Us,
    source_in_us: Us,
    drift_ppm: Option<f64>,
}

/// Layer `at` playing its file from `from` at its current start, at `pace`: where the file starts later, its start
/// moves to where the file begins; where the file doesn't reach the layer's end, the layer is cut back (it doesn't
/// grow back when the sync returns). Refused on a locked track, or when less than the shortest length would be left.
fn plan(p: &Project, at: (usize, usize), from: Us, pace: Pace) -> Result<Slip> {
    let track = &p.timeline.tracks[at.0];
    let item = &track.items[at.1];
    if track.locked {
        // Which layer can't follow.
        let named = msg!("model.source.followLocked", atUs = item.start_us);
        return Err(invalid_with(named));
    }
    let length = playing(item)
        .and_then(|(file, _, _)| p.asset(file))
        .map_or(Us::MAX, |a| a.duration_us);
    let (mut start, mut duration, mut from) = (item.start_us, item.duration_us, from);
    if from < 0 {
        // Rounded as the clip's own sound rounds its run's start (the file time there is 0 within a µs).
        let cut = ((from.saturating_neg() as f64) / pace.per_us()).round() as Us;
        start = start.saturating_add(cut);
        duration = duration.saturating_sub(cut);
        from = from.saturating_add(pace.footage(cut)).max(0);
    }
    if from.saturating_add(pace.footage(duration)) > length {
        duration = (length.saturating_sub(from) as f64 / pace.per_us()).floor() as Us;
    }
    if duration < MIN_DURATION_US {
        return Err(invalid(code!("model.source.syncSeparated")));
    }
    Ok(Slip {
        at,
        start_us: start,
        duration_us: duration,
        source_in_us: from,
        drift_ppm: match pace {
            Pace::Drift(ppm) => Some(ppm),
            Pace::Speed(_) => None,
        },
    })
}

/// A nudge: each tied layer plays its file `later` µs later (earlier when negative), as the clip's own sound or camera
/// would, so every moment stays in sync with the picture, at a cut too. A sound layer moves by its own drift; a video
/// layer, which plays at its speed, by its run's (`later × rate` of file time either way). `files` are the source's,
/// each with its run's drift. Returns the layers cut at their start. O(tied layers × files).
pub(super) fn slip_separated(
    p: &mut Project,
    files: &[(Id, f64)],
    later: Us,
    tied: &[Tied],
) -> Result<Vec<Id>> {
    let mut slips = Vec::with_capacity(tied.len());
    for t in tied {
        let item = &p.timeline.tracks[t.at.0].items[t.at.1];
        let Some((file, from, pace)) = playing(item) else {
            continue;
        };
        let Some(&(_, run_ppm)) = files.iter().find(|(f, _)| f == file) else {
            continue;
        };
        let step = match pace {
            Pace::Drift(ppm) => drifted(later, ppm),
            Pace::Speed(_) => drifted(later, run_ppm),
        };
        slips.push(plan(p, t.at, from.saturating_sub(step), pace)?);
    }
    Ok(apply(p, slips))
}

/// Sync's re-time: each tied layer plays, from its start, the recording moment it played before: its file time mapped
/// onto the recording through `old` (the source's runs and nudge before the change), then back into the file through
/// `runs` (nudge 0), a sound layer at its new run's drift; everything else about it kept. Returns the layers cut at
/// their start.
pub(super) fn follow_runs(
    p: &mut Project,
    old: &RecordingSource,
    runs: &[SourceRun],
    tied: &[Tied],
) -> Result<Vec<Id>> {
    // The run of `file` whose span (in its file, or on the clock) holds `at`, else its nearest.
    fn nearest<'r>(
        runs: &'r [SourceRun],
        file: &str,
        at: Us,
        span: impl Fn(&SourceRun) -> (Us, Us),
    ) -> Option<&'r SourceRun> {
        runs.iter().filter(|r| r.asset_id == file).min_by_key(|r| {
            let (a, b) = span(r);
            if at < a {
                a - at
            } else {
                (at - b).max(0)
            }
        })
    }
    let mut slips = Vec::with_capacity(tied.len());
    for t in tied {
        let item = &p.timeline.tracks[t.at.0].items[t.at.1];
        let Some((file, from, pace)) = playing(item) else {
            continue;
        };
        let in_file = |r: &SourceRun| (r.file_in_us, r.file_in_us.saturating_add(r.duration_us));
        let Some(was) = nearest(&old.runs, file, from, in_file) else {
            continue;
        };
        let rec = was.rec_of(from, old.nudge_us);
        let Some(run) = nearest(runs, file, rec, |r| r.rec_span(0)) else {
            continue;
        };
        let pace = match pace {
            Pace::Drift(_) => Pace::Drift(run.drift_ppm),
            speed => speed,
        };
        slips.push(plan(p, t.at, run.local_us(rec, 0), pace)?);
    }
    Ok(apply(p, slips))
}

/// The slips applied, computed before anything changed. Returns the layers cut at their start (their anchors move).
fn apply(p: &mut Project, slips: Vec<Slip>) -> Vec<Id> {
    let mut cut = Vec::new();
    for slip in slips {
        let (t, i) = slip.at;
        let Project { timeline, .. } = p;
        let item = &mut timeline.tracks[t].items[i];
        let offset = slip.start_us.saturating_sub(item.start_us);
        if offset > 0 {
            item.attach = item
                .attach
                .take()
                .and_then(|a| shifted_anchor(&timeline.main, &a, offset, slip.start_us));
            cut.push(item.id.clone());
        }
        item.start_us = slip.start_us;
        item.duration_us = slip.duration_us;
        match &mut item.content {
            ItemContent::Audio {
                source_in_us,
                drift_ppm,
                ..
            } => {
                *source_in_us = slip.source_in_us;
                if let Some(ppm) = slip.drift_ppm {
                    *drift_ppm = ppm;
                }
            }
            ItemContent::Video { source_in_us, .. } => {
                *source_in_us = slip.source_in_us;
            }
            _ => {}
        }
        // Keys keep their place on the timeline (as a trim keeps them); those past the end go.
        if offset > 0 {
            start_moved(item, offset);
        }
        fit_keys(item);
        fit_fades(item);
    }
    cut
}
