//! Pauses from loudness (silence removal is acoustic and works without any model). Levels are the
//! per-channel mean K-weighted power of each 100 ms window of a voice file, in dB (`10·log10(power / 2)` of the PCM
//! cache's energies: one definition on every path, so the constants below mean the same everywhere). The
//! threshold adapts to each file, so a quiet microphone and a noisy room both work.
//!
//! Cost: O(windows) per file (two selections and one scan; one copy of the levels for the selections, 288 KB for two
//! hours), then O(spans × log pauses + pauses shown) to put them on the timeline.

use std::collections::{HashMap, HashSet};

use super::{run_spans, Levels, Speech, LEVEL_WINDOW_US};
use crate::project::Project;
use crate::query::main_track_duration;
use crate::time::{merge_ranges, RangeUs, Us};

/// What each end of a pause keeps, so the speech around it keeps a breath.
pub const KEEP_US: Us = 150_000;
const FLOOR_DB: f32 = -55.0;
const CEILING_DB: f32 = -30.0;
/// A source whose loud windows (90th percentile) stay under this has no speech, so nothing in it is a pause.
const NO_SPEECH_DB: f32 = -50.0;
/// What a window without a finite level (digital silence's −∞, a NaN) counts as: quiet.
const SILENT_DB: f32 = -120.0;

fn level(db: f32) -> f32 {
    if db.is_finite() {
        db
    } else {
        SILENT_DB
    }
}

fn percentile(values: &mut [f32], p: usize) -> f32 {
    let i = (values.len() - 1) * p / 100;
    *values.select_nth_unstable_by(i, f32::total_cmp).1
}

/// The level under which a window is quiet: the middle of the source's 10th and 90th percentile, kept within
/// −55…−30 dBFS. None when the source has no speech at all.
pub fn threshold(levels: &Levels) -> Option<f32> {
    if levels.db.is_empty() {
        return None;
    }
    let mut v: Vec<f32> = levels.db.iter().copied().map(level).collect();
    let (p10, p90) = (percentile(&mut v, 10), percentile(&mut v, 90));
    (p90 >= NO_SPEECH_DB).then(|| ((p10 + p90) / 2.0).clamp(FLOOR_DB, CEILING_DB))
}

/// Quiet runs of at least `min_pause_us` (and at least 0.4 s: a breath kept at each end and a window cut) in the
/// source's own time, each shortened by `KEEP_US` at both ends. Sorted, never touching.
pub fn pauses(levels: &Levels, min_pause_us: Us) -> Vec<RangeUs> {
    let Some(th) = threshold(levels) else {
        return Vec::new();
    };
    let min = min_pause_us.max(2 * KEEP_US + LEVEL_WINDOW_US);
    let mut out = Vec::new();
    let mut run: Option<usize> = None;
    // A loud window after the last closes a run that reaches the end.
    for (i, db) in levels.db.iter().copied().chain([0.0]).enumerate() {
        if level(db) < th {
            run.get_or_insert(i);
        } else if let Some(s) = run.take() {
            let (start, end) = (s as Us * LEVEL_WINDOW_US, i as Us * LEVEL_WINDOW_US);
            if end - start >= min {
                out.push(RangeUs {
                    start: start + KEEP_US,
                    end: end - KEEP_US,
                });
            }
        }
    }
    out
}

/// The timeline ranges a pause removal cuts: each (clip, run) span's share of its file's pauses, mapped through the
/// span (its offset, rate and speed, so drift is followed), merged. `within`: only those clips (ids). Clips without
/// levels (no sound, or not read yet) give none: the caller tells that apart. Each file's pauses are found once.
pub fn timeline_pauses(
    p: &Project,
    speech: &Speech,
    min_pause_us: Us,
    within: Option<&HashSet<&str>>,
) -> Vec<RangeUs> {
    let mut by_asset: HashMap<&str, Vec<RangeUs>> = HashMap::new();
    let mut out = Vec::new();
    for span in run_spans(p) {
        if within.is_some_and(|w| !w.contains(span.clip.as_str())) {
            continue;
        }
        let Some((asset, levels)) = speech.levels.get_key_value(span.asset.as_str()) else {
            continue;
        };
        let ps = by_asset
            .entry(asset.as_str())
            .or_insert_with(|| pauses(levels, min_pause_us));
        let first = ps.partition_point(|r| r.end <= span.asset_in_us);
        for r in ps[first..]
            .iter()
            .take_while(|r| r.start < span.asset_out_us)
        {
            let (a, b) = (span.at(r.start), span.at(r.end));
            if b > a {
                out.push(RangeUs { start: a, end: b });
            }
        }
    }
    merge_ranges(&out, main_track_duration(&p.timeline.main))
}
