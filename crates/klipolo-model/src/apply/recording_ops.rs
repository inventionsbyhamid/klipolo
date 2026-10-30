//! Recording sources and markers: a source's label, default mix and sync nudge; a clip's override of one source's mix;
//! source's mix; markers on the recording clock. None of them moves a clip; a sync change slips the source's separated
//! layers (sound and camera). Ranges (a mix's volume, a nudge, a label's length, a marker inside its recording) are checked by
//! validation after the batch.

use crate::error::{invalid, not_found, Result};
use crate::ids::Id;
use crate::messages::code;
use crate::ops::SourcePatch;
use crate::project::{Marker, Project, Recording, RecordingSource, SourceMix};
use crate::query::makes_sound;
use crate::timeline::ClipSource;

use super::places::Places;
use super::slip::{slip_separated, tied};
use super::sync_ops::retime;

fn recording_mut<'a>(p: &'a mut Project, recording: &str) -> Result<&'a mut Recording> {
    p.recordings
        .iter_mut()
        .find(|r| r.id == recording)
        .ok_or_else(|| not_found("recording", recording))
}

fn source_mut<'a>(
    p: &'a mut Project,
    recording: &str,
    source: &str,
) -> Result<&'a mut RecordingSource> {
    recording_mut(p, recording)?
        .sources
        .iter_mut()
        .find(|s| s.id == source)
        .ok_or_else(|| invalid(code!("model.source.missing")))
}

/// Returns the separated layers a sync change cut back (their starts and anchors moved). Re-timed runs go first
/// (`sync_ops::retime`: the tied layers follow, the nudge back to 0), so a nudge in the same patch slips them once.
pub(super) fn update_source(
    p: &mut Project,
    recording: &str,
    source: &str,
    patch: &SourcePatch,
) -> Result<Vec<Id>> {
    let mut cut = Vec::new();
    if let Some(runs) = &patch.runs {
        cut = retime(p, recording, source, runs, patch.synced)?;
    }
    // The layers tied to the source as it plays now, before its nudge changes.
    let tied_now = match patch.nudge_us {
        Some(_) => p
            .recording(recording)
            .and_then(|r| r.source(source))
            .map(|s| tied(p, recording, s))
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let s = source_mut(p, recording, source)?;
    if patch.runs.is_none() && patch.synced.is_some() {
        s.synced = patch.synced;
    }
    let later = patch.nudge_us.map_or(0, |n| n.saturating_sub(s.nudge_us));
    if let Some(label) = &patch.label {
        s.label = label.clone();
    }
    if let Some(mix) = &patch.mix {
        s.mix = mix.clone();
    }
    if let Some(nudge) = patch.nudge_us {
        s.nudge_us = nudge;
    }
    if later == 0 {
        return Ok(cut);
    }
    let files: Vec<(Id, f64)> = s
        .runs
        .iter()
        .map(|r| (r.asset_id.clone(), r.drift_ppm))
        .collect();
    cut.extend(slip_separated(p, &files, later, &tied_now)?);
    Ok(cut)
}

/// Refused whatever `mix` is: the clip must be a recording clip, and `source` one it is heard from.
pub(super) fn set_clip_source_mix(
    p: &mut Project,
    places: &mut Places,
    clip: &str,
    source: &str,
    mix: Option<&SourceMix>,
) -> Result<()> {
    let i = places
        .clip(&p.timeline.main, clip)
        .ok_or_else(|| not_found("clip", clip))?;
    // A sound source, or a picture source whose files carry sound.
    let sound = match &p.timeline.main[i].source {
        ClipSource::Recording { id } => p
            .recording(id)
            .and_then(|r| r.source(source))
            .is_some_and(|s| makes_sound(p, s)),
        ClipSource::Asset { .. } => false,
    };
    if !sound {
        return Err(invalid(code!("model.clip.sourceMix")));
    }
    let c = &mut p.timeline.main[i];
    match mix {
        Some(mix) => {
            c.source_mix.insert(source.into(), mix.clone());
        }
        None => {
            c.source_mix.remove(source);
        }
    }
    Ok(())
}

pub(super) fn add_marker(p: &mut Project, recording: &str, marker: &Marker) -> Result<()> {
    let r = recording_mut(p, recording)?;
    r.markers.retain(|m| m.id != marker.id);
    r.markers.push(marker.clone());
    // Stable: markers at one moment keep the order they were placed in.
    r.markers.sort_by_key(|m| m.at_us);
    Ok(())
}

pub(super) fn remove_marker(p: &mut Project, recording: &str, id: &str) -> Result<()> {
    let r = recording_mut(p, recording)?;
    let before = r.markers.len();
    r.markers.retain(|m| m.id != id);
    if r.markers.len() < before {
        Ok(())
    } else {
        Err(invalid(code!("model.marker.missing")))
    }
}
