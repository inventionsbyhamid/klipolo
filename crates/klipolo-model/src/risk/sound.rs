//! A recording's recorded sound covering less of it: a Sync again, or a hand line-up, re-times a
//! sound source's runs (`updateRecordingSource`'s `runs`), and a placement that lines its file up over less of the
//! recording takes recorded sound away there (the camera's sound, or silence, plays instead). Review lists the
//! recording, so chat and other apps propose it. A picture source can't be re-timed (validation), so its footage is
//! `footage.rs`'s.
//!
//! Measured where it is heard: inside the recording's pictures (its picture sources' runs), so a run
//! slid past the picture's end counts though its clock length is the same; and with the drift's own slack, so a re-fit
//! that only changes a run's drift (its clock length shrinks by its length × the change: 1.2 s over 20 minutes at
//! 1,000 ppm) takes nothing away.

use crate::ids::Id;
use crate::project::{Project, Recording, RecordingSource};
use crate::time::{merge_ranges, RangeUs, Us};

/// Less than this much less recorded sound (µs), beyond the drift's slack, is a re-placement's rounding, not a
/// removal.
const SHRINK_US: Us = 1_000_000;

/// The recordings in both projects with a sound source (by id) whose runs cover more than `SHRINK_US` (plus the drift's
/// slack) less of the recording's pictures after than before, in `before`'s order. O(recordings × sources × runs ×
/// picture runs).
pub(super) fn shrunk(before: &Project, after: &Project) -> Vec<Id> {
    before
        .recordings
        .iter()
        .filter(|r| {
            let Some(now) = after.recording(&r.id) else {
                return false;
            };
            let (was, is) = (pictures(r), pictures(now));
            r.audio_sources().any(|s| {
                now.source(&s.id).is_some_and(|n| {
                    let lost = heard(s, was.as_deref()).saturating_sub(heard(n, is.as_deref()));
                    lost > SHRINK_US.saturating_add(slack(s, n))
                })
            })
        })
        .map(|r| r.id.clone())
        .collect()
}

/// Where the recording has pictures on its clock: its picture sources' runs, merged; None without a picture source
/// (then the whole clock counts).
fn pictures(r: &Recording) -> Option<Vec<RangeUs>> {
    let spans: Vec<RangeUs> = (r.sources.iter())
        .filter(|s| s.role.is_video())
        .flat_map(|s| {
            s.runs.iter().map(|run| {
                let (start, end) = run.rec_span(s.nudge_us);
                RangeUs { start, end }
            })
        })
        .collect();
    (!spans.is_empty()).then(|| merge_ranges(&spans, Us::MAX))
}

/// How much of `pictures` (None: the whole clock) a source's runs cover (µs): runs never overlap on the clock, and
/// merged pictures never overlap each other.
fn heard(s: &RecordingSource, pictures: Option<&[RangeUs]>) -> Us {
    s.runs
        .iter()
        .map(|run| {
            let (from, to) = run.rec_span(s.nudge_us);
            match pictures {
                None => to.saturating_sub(from),
                Some(p) => p
                    .iter()
                    .map(|p| to.min(p.end).saturating_sub(from.max(p.start)).max(0))
                    .sum(),
            }
        })
        .sum()
}

/// What a re-fit of the drift alone can take from a source's clock length (µs): its files' time × the largest change
/// of rate between a run before and a run after.
fn slack(before: &RecordingSource, after: &RecordingSource) -> Us {
    let ppm = |s: &RecordingSource| s.runs.iter().map(|r| r.drift_ppm).collect::<Vec<_>>();
    let (was, is) = (ppm(before), ppm(after));
    let change = (was.iter())
        .flat_map(|a| is.iter().map(move |b| (a - b).abs()))
        .fold(0.0, f64::max);
    let length: Us = before.runs.iter().map(|r| r.duration_us).sum();
    (length as f64 * change * 1e-6).ceil() as Us
}
