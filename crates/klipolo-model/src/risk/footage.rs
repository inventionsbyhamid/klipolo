//! Played-footage equality: review's lost-footage check compares the files a clip shows and their file spans,
//! not its `ClipSource`, so `useRecording`, which moves an imported clip onto the recording that holds its file, keeps
//! every frame. A clip whose source didn't change never comes here: `lost_footage` compares its source times as before,
//! with nothing allocated. A converted clip costs two small vectors (one entry per run it plays).

use std::collections::HashMap;

use crate::project::Project;
use crate::recording::chapters;
use crate::time::{scaled, unscaled, RangeUs, Us};
use crate::timeline::{Clip, ClipSource};

use super::SLACK_US;

/// One stretch of footage a clip shows: its file, the file span, and the timeline span it plays at.
pub(crate) type Shown<'a> = (&'a str, RangeUs, RangeUs);

/// The picture footage a main clip shows: its asset and source span, or for a recording clip each run of its primary
/// picture source that it plays, as (file, file span, timeline span). Drift 0 runs map 1:1. A held frame across a gap
/// between runs is no footage of its own. A clip that shows a chapter before its join and plays on into the next
/// chapter's first moments, up to where the earlier file ends (`chapters::tail`), shows the earlier file's footage
/// there: the same moments of the take, so a clip moved onto the recording keeps all of it.
pub(crate) fn shown<'a>(p: &'a Project, clip: &'a Clip, start: Us) -> Vec<Shown<'a>> {
    let timeline = |from: Us, to: Us| RangeUs {
        start: from,
        end: to,
    };
    match &clip.source {
        ClipSource::Asset { id } => vec![(
            id.as_str(),
            timeline(clip.source_in_us, clip.source_out_us),
            timeline(start, start.saturating_add(clip.duration_us())),
        )],
        ClipSource::Recording { id } => {
            let Some(source) = p.recording(id).and_then(|r| r.primary_source()) else {
                return Vec::new();
            };
            let (runs, nudge) = (&source.runs, source.nudge_us);
            let at = |x: Us| start.saturating_add(scaled(x - clip.source_in_us, clip.speed));
            // Where the run before stopped showing its own file on the clock (past its end at a chapter's tail).
            let mut taken = Us::MIN;
            let mut out = Vec::with_capacity(runs.len());
            for (k, run) in runs.iter().enumerate() {
                let (from, end) = run.rec_span(nudge);
                let file_us = p.asset(&run.asset_id).map_or(0, |a| a.duration_us);
                let tail = if clip.source_in_us < end {
                    chapters::tail(runs, k, file_us, SLACK_US)
                } else {
                    0
                };
                let (from, to) = (from.max(taken), end.saturating_add(tail));
                taken = to;
                let (from, to) = (from.max(clip.source_in_us), to.min(clip.source_out_us));
                if from < to {
                    let file = timeline(run.local_us(from, nudge), run.local_us(to, nudge));
                    out.push((run.asset_id.as_str(), file, timeline(at(from), at(to))));
                }
            }
            out
        }
    }
}

fn near(a: Us, b: Us) -> bool {
    a.abs_diff(b) <= SLACK_US.unsigned_abs()
}

fn near_span(a: RangeUs, b: RangeUs) -> bool {
    near(a.start, b.start) && near(a.end, b.end)
}

/// Whether `after`'s clip shows the same files over the same file spans at the same timeline spans as `before`'s:
/// lost footage is compared by played footage, not by `ClipSource`. Equal clips of one source return at once.
pub(crate) fn same_shown(
    before: &Project,
    b: &Clip,
    b_start: Us,
    after: &Project,
    a: &Clip,
    a_start: Us,
) -> bool {
    if b == a && b_start == a_start && matches!(b.source, ClipSource::Asset { .. }) {
        return true;
    }
    let (was, now) = (shown(before, b, b_start), shown(after, a, a_start));
    was.len() == now.len()
        && was
            .iter()
            .zip(&now)
            .all(|(w, n)| w.0 == n.0 && near_span(w.1, n.1) && near_span(w.2, n.2))
}

/// The stretches of `was`'s source that `now`, a clip of another source, shows the same frames of, in `was`'s source
/// time: per run segment `now` plays, the part of its file span that `was` shows of the same file, mapped back through
/// `was`'s own segment. Unsorted; the caller merges them. Speed and place on the timeline don't matter here, as a new
/// speed on a piece keeps its footage.
pub(crate) fn kept_spans(
    before: &Project,
    was: &Clip,
    after: &Project,
    now: &Clip,
) -> Vec<RangeUs> {
    let ours = shown(before, was, 0);
    let mut out = Vec::new();
    for (file, theirs, _) in shown(after, now, 0) {
        for (_, span, at) in ours.iter().filter(|(f, ..)| *f == file) {
            let (lo, hi) = (theirs.start.max(span.start), theirs.end.min(span.end));
            if lo >= hi || span.end <= span.start {
                continue;
            }
            // `was`'s source times over its segment, from the segment's place in the clip (it starts at 0 here).
            let from = was
                .source_in_us
                .saturating_add(unscaled(at.start, was.speed));
            let to = was.source_in_us.saturating_add(unscaled(at.end, was.speed));
            let per = (to - from) as f64 / (span.end - span.start) as f64;
            let map = |x: Us| from.saturating_add(((x - span.start) as f64 * per).round() as Us);
            out.push(RangeUs {
                start: map(lo),
                end: map(hi),
            });
        }
    }
    out
}

/// The clips of `before` still there on another source (`clips_after`, by id): what each shows the same goes into its
/// `pieces` entry, so `lost_footage` counts it kept as a piece's. The whole clip when it plays the same footage at the
/// same place ([`same_shown`]), else the stretches it still shows ([`kept_spans`]).
pub(crate) fn converted<'a>(
    before: &'a Project,
    after: &Project,
    clips_after: &HashMap<&str, &Clip>,
    pieces: &mut HashMap<&'a str, Vec<RangeUs>>,
) {
    for c in &before.timeline.main {
        let Some(now) = clips_after.get(c.id.as_str()) else {
            continue;
        };
        if now.source == c.source {
            continue;
        }
        let kept = if same_shown(before, c, 0, after, now, 0) {
            vec![RangeUs {
                start: c.source_in_us,
                end: c.source_out_us,
            }]
        } else {
            kept_spans(before, c, after, now)
        };
        pieces.entry(c.id.as_str()).or_default().extend(kept);
    }
}
