//! A camera's chapters: Sync ends a chapter's run where the next one starts on the clock,
//! so runs never overlap, but a camera's chapter files can overlap a little (a frame or two; the generated ones by
//! 67 ms). A clip showing such a chapter to its file's end still lies on the recording: its last moments play from the
//! next chapter, the same moment on the clock, and review counts them as the earlier file's footage (`tail`).

use super::SourceRun;
use crate::time::Us;

/// The most of a chapter's file the next one may take over with a clip to the file's end still on its run (µs).
pub const CHAPTER_OVERLAP_US: Us = 500_000;

/// Whether the next run starts where `runs[k]` ends on the clock (within `slack_us`): a chapter join. Runs are in clock
/// order.
pub fn joins(runs: &[SourceRun], k: usize, slack_us: Us) -> bool {
    let to = runs[k].rec_span(0).1;
    (runs.get(k + 1)).is_some_and(|next| (next.rec_span(0).0 - to).abs() <= slack_us)
}

/// How far into its file a clip on `runs[k]` may reach (µs): the run's end, or up to `CHAPTER_OVERLAP_US` past it at a
/// chapter join (`joins`).
pub fn reach(runs: &[SourceRun], k: usize, slack_us: Us) -> Us {
    let run = &runs[k];
    let end = run.file_in_us.saturating_add(run.duration_us);
    if joins(runs, k, slack_us) {
        end.saturating_add(CHAPTER_OVERLAP_US)
    } else {
        end
    }
}

/// How far past its run's end on the clock (µs) `runs[k]`'s own file, `file_us` long, still holds the take at a chapter
/// join: up to its file's end, at most `CHAPTER_OVERLAP_US`; 0 elsewhere. A clip reaching there shows the earlier
/// file's footage, played from the next chapter.
pub fn tail(runs: &[SourceRun], k: usize, file_us: Us, slack_us: Us) -> Us {
    if !joins(runs, k, slack_us) {
        return 0;
    }
    let run = &runs[k];
    let past = file_us.saturating_sub(run.file_in_us.saturating_add(run.duration_us));
    let on_clock = (past as f64 / (1.0 + run.drift_ppm * 1e-6)).round() as Us;
    on_clock.clamp(0, CHAPTER_OVERLAP_US)
}
