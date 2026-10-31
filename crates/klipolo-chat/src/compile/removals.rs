//! The removals, each one `DeleteRanges` on the removals' working copy (the project as the additions and the earlier
//! removals of the answer leave it).
//! - `cutRange`: the range as the evidence snaps it, mapped to where that footage plays now.
//! - `keepOnly`: each part kept, snapped and mapped the same way; everything else on the main track goes
//!   (`kept_parts(0, main_track_duration, merge_ranges(keep))`).
//! - `removeSilences`: the pauses in the sound (`timeline_pauses`, from the loudness levels) longer than the length
//!   asked, `PAUSE_US` when none was.
//! - `removeFillers`: every filler word, each cut as `word_cut` pads it. None is cut while any is only roughly timed
//!   (its word's confidence 0: spread over its turn when word timing failed): the person is told to time the words
//!   again (`ROUGH_FILLERS`), naming the clip.
//!
//! The pauses and fillers are found on the working copy itself, so they need no mapping. Nothing to remove is told to
//! the person (`NO_FILLERS`, "There are no pauses longer than 1.0 s to remove."): asking the model again could not
//! change it.

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::Op;
use klipolo_model::query::main_track_duration;
use klipolo_model::speech::silence::timeline_pauses;
use klipolo_model::speech::{PAUSE_US, Speech, Spoken, SpokenWord, WordKind, spoken, word_cut};
use klipolo_model::time::{RangeUs, Us, kept_parts, merge_ranges};

use super::{
    Audience, Compiler, Edit, NO_FILLERS, ROUGH_FILLERS, Refused, label, to_model, to_person, us,
};
use crate::context::secs;
use crate::tools::ToolCall;

/// A removal call's edit on `work`. Only removals reach here.
pub(super) fn compile(c: &Compiler, work: &Project, call: &ToolCall) -> Result<Edit, Refused> {
    match call {
        ToolCall::CutRange { range } => cut_range(c, work, *range),
        ToolCall::KeepOnly { ranges } => keep_only(c, work, ranges),
        ToolCall::RemoveSilences { min_pause_us } => {
            remove_silences(c, work, min_pause_us.unwrap_or(PAUSE_US))
        }
        ToolCall::RemoveFillers => remove_fillers(c, work),
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

fn remove_silences(c: &Compiler, work: &Project, min_pause_us: Us) -> Result<Edit, Refused> {
    let pauses = timeline_pauses(work, &c.inputs.speech, min_pause_us, None);
    if pauses.is_empty() {
        return Err(to_person(format!(
            "There are no pauses longer than {} s to remove.",
            secs(min_pause_us)
        )));
    }
    let count = pauses.len();
    Ok(Edit {
        ops: delete(pauses),
        label: label(
            "removeSilences",
            vec![
                ("count", ErrorParam::from(count)),
                ("minPauseDurationUs", us(min_pause_us)),
            ],
        ),
    })
}

fn remove_fillers(c: &Compiler, work: &Project) -> Result<Edit, Refused> {
    let speech = &c.inputs.speech;
    let words = spoken(work, speech);
    let fps = work.settings.fps;
    let fillers: Vec<usize> = (0..words.words.len())
        .filter(|&i| words.words[i].kind == WordKind::Filler)
        .collect();
    if let Some(rough) =
        (fillers.iter().map(|&i| &words.words[i])).find(|w| guessed(&words, w, speech))
    {
        return Err(Refused {
            reason: ROUGH_FILLERS.to_owned(),
            audience: Audience::Person,
            clip: c.inputs.refs.ref_of(&words.spans[rough.span as usize].clip),
        });
    }
    let cuts: Vec<RangeUs> = (fillers.iter())
        .map(|&i| word_cut(&words, i, i, fps))
        .collect();
    if cuts.is_empty() {
        return Err(to_person(NO_FILLERS.to_owned()));
    }
    let count = cuts.len();
    Ok(Edit {
        ops: delete(merge_ranges(
            &cuts,
            main_track_duration(&work.timeline.main),
        )),
        label: label("removeFillers", vec![("count", ErrorParam::from(count))]),
    })
}

/// Whether `w`'s time is only a guess: its transcript word was spread over its turn by length, not timed (confidence
/// 0), so a cut there could take the words around it.
fn guessed(words: &Spoken, w: &SpokenWord, speech: &Speech) -> bool {
    (speech.transcripts.get(words.asset(w)))
        .and_then(|t| t.words.get(w.word as usize))
        .is_some_and(|word| word.confidence == 0.0)
}
