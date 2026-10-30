//! Sync's ops: an imported clip moved onto the recording that holds its file (`useRecording`), a synced source
//! added to a recording (`addRecordingSource`), and a source's files re-timed (`updateRecordingSource`'s `runs`).
//! None of them takes anything away (a clip keeps its footage, a re-time keeps every file), so all three are Additive.

use crate::error::{invalid, not_found, Result};
use crate::ids::Id;
use crate::messages::code;
use crate::project::{Project, RecordingSource, SourceRun, Synced};
use crate::query::pick;
use crate::recording::chapters;
use crate::time::Us;
use crate::timeline::{ClipSource, Timeline};
use crate::validate::validate_recording;

use super::places::Places;
use super::slip::{follow_runs, tied};

const NOT_THIS_FILE: &str = code!("model.recording.notThisFile");
/// Rounding slack where a clip's file times meet its run's ends (validation's `SOURCE_SLACK_US`).
const SLACK_US: Us = 1_000;

/// Moves main-track clip `clip_id`, a clip of an imported file, onto recording `recording_id`, whose primary picture
/// source holds that file as a run: its source in and out map through the run (`rec_of`; the identity for the first
/// picture file, which is the recording's clock), and so do the anchors of the layers it plays: an anchor moves exactly
/// when it resolves to this clip now (`query::pick`), so a batch that converts every clip of a file, in any
/// order, keeps each layer on the clip that showed it. The clip keeps its id and every other field, so nothing on the
/// timeline moves. A clip of another file, one already playing a recording, or one whose stretch of its file isn't
/// inside the run's is refused (`notThisFile`). O(layers), plus a clip lookup (`Places`) per anchor of
/// this file and a scan of the track for one that resolves to a copy.
pub(super) fn use_recording(
    p: &mut Project,
    clip_id: &str,
    recording_id: &str,
    places: &mut Places,
) -> Result<()> {
    let i = places
        .clip(&p.timeline.main, clip_id)
        .ok_or_else(|| not_found("clip", clip_id))?;
    let recording = p
        .recording(recording_id)
        .ok_or_else(|| not_found("recording", recording_id))?;
    let clip = &p.timeline.main[i];
    let ClipSource::Asset { id: file } = &clip.source else {
        return Err(invalid(NOT_THIS_FILE));
    };
    let primary = recording
        .primary_source()
        .ok_or_else(|| invalid(NOT_THIS_FILE))?;
    // The run of this file whose stretch holds the clip's whole span (rounding slack at each end; a chapter's clip may
    // reach a little past its run, into the next chapter: `chapters::reach`).
    let (from, to) = (clip.source_in_us, clip.source_out_us);
    let runs = &primary.runs;
    let run = (0..runs.len())
        .find(|&k| {
            let r = &runs[k];
            r.asset_id == *file
                && from >= r.file_in_us.saturating_sub(SLACK_US)
                && to <= chapters::reach(runs, k, SLACK_US).saturating_add(SLACK_US)
        })
        .ok_or_else(|| invalid(NOT_THIS_FILE))
        .map(|k| runs[k].clone())?;
    let nudge = primary.nudge_us;
    let was = clip.source.clone();
    let now = ClipSource::Recording {
        id: recording_id.to_owned(),
    };
    // The layers whose anchor plays on this clip now, found before anything changes.
    let mut moving = Vec::new();
    let Timeline { main, tracks, .. } = &mut p.timeline;
    for (t, track) in tracks.iter().enumerate() {
        for (k, item) in track.items.iter().enumerate() {
            let Some(a) = item.attach.as_ref().filter(|a| a.source == was) else {
                continue;
            };
            let own = places.clip(main, &a.clip_id);
            if pick(main, a, own) == Some(i) {
                moving.push((t, k));
            }
        }
    }
    for (t, k) in moving {
        if let Some(a) = tracks[t].items[k].attach.as_mut() {
            a.source = now.clone();
            a.source_us = run.rec_of(a.source_us, nudge);
        }
    }
    let c = &mut main[i];
    c.source_in_us = run.rec_of(from, nudge);
    c.source_out_us = run.rec_of(to, nudge);
    c.source = now;
    Ok(())
}

/// Adds `source` to recording `recording_id`: its id unused there, its role one the recording may hold again (a mic,
/// an app's sound) or doesn't hold yet. The recording is then validated as a whole, as the batch's end would.
pub(super) fn add_recording_source(
    p: &mut Project,
    recording_id: &str,
    source: &RecordingSource,
) -> Result<()> {
    let index = p
        .recordings
        .iter()
        .position(|r| r.id == recording_id)
        .ok_or_else(|| not_found("recording", recording_id))?;
    p.recordings[index].sources.push(source.clone());
    validate_recording(p, &p.recordings[index])
}

/// The files of `runs` in order, a file's consecutive runs counted once: a re-time keeps this sequence.
fn files(runs: &[SourceRun]) -> impl Iterator<Item = &str> {
    runs.iter()
        .enumerate()
        .filter(|&(k, r)| k == 0 || runs[k - 1].asset_id != r.asset_id)
        .map(|(_, r)| r.asset_id.as_str())
}

/// Sets the source's runs and its `synced`, and resets its nudge to 0. The new runs name exactly the old
/// runs' files, every one of them, in file order (a file may gain or lose a split at a pause), so no sound is
/// lost and the patch stays Additive. The layers tied to the source follow (`slip::follow_runs`
/// 1–2, I2 and N2: their cuts, fades and volume kept; a layer nothing tied, such as a reused stretch, left alone). Refused for a picture
/// source (the clock), and when a tied layer can't follow (a locked track, nothing left to play). Returns the layers
/// cut at their start.
pub(super) fn retime(
    p: &mut Project,
    recording_id: &str,
    source_id: &str,
    runs: &[SourceRun],
    synced: Option<Synced>,
) -> Result<Vec<Id>> {
    let recording = p
        .recording(recording_id)
        .ok_or_else(|| not_found("recording", recording_id))?;
    let source = recording
        .source(source_id)
        .ok_or_else(|| invalid(code!("model.source.missing")))?;
    // The pictures are the clock: Sync re-times sound only.
    if source.role.is_video() {
        return Err(invalid(code!("model.recording.retimePicture")));
    }
    if !files(&source.runs).eq(files(runs)) {
        return Err(invalid(code!("model.recording.retimeFiles")));
    }
    let tied = tied(p, recording_id, source);
    let old = source.clone();
    let cut = follow_runs(p, &old, runs, &tied)?;
    let (r, s) = (recording_id, source_id);
    let source = p
        .recordings
        .iter_mut()
        .find(|x| x.id == r)
        .and_then(|x| x.sources.iter_mut().find(|x| x.id == s))
        .ok_or_else(|| invalid(code!("model.source.missing")))?;
    source.runs = runs.to_vec();
    source.synced = synced;
    source.nudge_us = 0;
    Ok(cut)
}
