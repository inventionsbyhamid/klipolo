//! The removals, each one `DeleteRanges` on the removals' working copy (the project as the additions and the earlier
//! removals of the answer leave it).
//! - `cutRange`: the range as the evidence snaps it, mapped to where that footage plays now.
//! - `keepOnly`: each part kept, snapped and mapped the same way; everything else on the main track goes
//!   (`kept_parts(0, main_track_duration, merge_ranges(keep))`).
//! - `removeSilences`: the pauses in the sound (`timeline_pauses`, from the loudness levels) longer than the length
//!   asked, `PAUSE_US` when none was.
//! - `removeFillers`: every filler word, each cut as `word_cut` pads it.
//!
//! The pauses and fillers are found on the working copy itself, so they need no mapping. Nothing to remove is said back
//! to the model.

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::Op;
use klipolo_model::query::main_track_duration;
use klipolo_model::time::{RangeUs, kept_parts, merge_ranges};

use super::{Compiler, Edit, Refused, label, to_model, us};
use crate::context::secs;
use crate::tools::ToolCall;

/// A removal call's edit on `work`. Only removals reach here.
pub(super) fn compile(c: &Compiler, work: &Project, call: &ToolCall) -> Result<Edit, Refused> {
    match call {
        ToolCall::CutRange { range } => cut_range(c, work, *range),
        ToolCall::KeepOnly { ranges } => keep_only(c, work, ranges),
        other => Err(to_model(format!("{}: not a removal.", other.name()))),
    }
}

fn delete(ranges: Vec<RangeUs>) -> Vec<Op> {
    vec![Op::DeleteRanges { ranges }]
}

/// Said when every part of `r` (snapshot time) went with an earlier call of the same answer.
fn already_removed(tool: &str, r: RangeUs) -> Refused {
    to_model(format!(
        "{tool}: an earlier part of this answer already removes {} to {} s.",
        secs(r.start),
        secs(r.end)
    ))
}

fn cut_range(c: &Compiler, work: &Project, range: RangeUs) -> Result<Edit, Refused> {
    let snapped = c.checked("cutRange", ("startSec", "endSec"), range)?;
    let now = c.map.range(work, snapped);
    if now.is_empty() {
        return Err(already_removed("cutRange", snapped));
    }
    Ok(Edit {
        ops: delete(now),
        label: label(
            "cutRange",
            vec![("startUs", us(snapped.start)), ("endUs", us(snapped.end))],
        ),
    })
}

fn keep_only(c: &Compiler, work: &Project, ranges: &[RangeUs]) -> Result<Edit, Refused> {
    let mut keep = Vec::new();
    for (k, &r) in ranges.iter().enumerate() {
        let snapped = c.checked(
            "keepOnly",
            (
                &format!("ranges[{k}].startSec"),
                &format!("ranges[{k}].endSec"),
            ),
            r,
        )?;
        keep.extend(c.map.range(work, snapped));
    }
    if keep.is_empty() {
        return Err(to_model(
            "keepOnly: an earlier part of this answer already removes every part to keep."
                .to_owned(),
        ));
    }
    let length = main_track_duration(&work.timeline.main);
    let gone = kept_parts(0, length, &merge_ranges(&keep, length));
    if gone.is_empty() {
        return Err(to_model(
            "keepOnly: the parts to keep cover the whole video, so nothing would go.".to_owned(),
        ));
    }
    Ok(Edit {
        ops: delete(gone),
        label: label("keepOnly", vec![("parts", ErrorParam::from(ranges.len()))]),
    })
}
