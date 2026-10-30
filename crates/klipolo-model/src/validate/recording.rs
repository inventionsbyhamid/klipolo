//! Recordings in format 4 and the mixes of their sources: what `validate` checks of each recording, and of a
//! source's mix wherever one is stored (its default, a clip's own).

use super::{
    check, check_with, in_range, valid_time, validate_stack, MAX_DRIFT_PPM, MAX_NAME, MAX_NUDGE_US,
    PLAYED_CHANNELS, SOURCE_SLACK_US,
};
use crate::coded::CodedError;
use crate::contrib::ContribKind;
use crate::error::Result;
use crate::messages::{code, msg};
use crate::project::{AssetKind, Project, Recording, RecordingSource, SourceMix, SourceRole};
use crate::query::makes_sound;

/// The code naming `role` in `model.recording.duplicateRole`.
fn role_code(role: SourceRole) -> &'static str {
    match role {
        SourceRole::Screen => code!("model.role.screen"),
        SourceRole::Game => code!("model.role.game"),
        SourceRole::Camera => code!("model.role.camera"),
        SourceRole::Mic => code!("model.role.mic"),
        SourceRole::SystemAudio => code!("model.role.systemAudio"),
        SourceRole::GameAudio => code!("model.role.gameAudio"),
        SourceRole::VoiceChat => code!("model.role.voiceChat"),
        SourceRole::OtherApp => code!("model.role.otherApp"),
    }
}

/// A recording in format 4: sources with distinct ids, one per role except a mic and an app's sound (which may
/// repeat: two lavs), a screen, game or camera as the primary, runs in order on files of the right kind, each
/// inside its file (a run may start inside it), and markers inside the take in time order (what `AddMarker` keeps
/// and import writes; the marker tools look them up by time). O(sources² + runs² + runs × media + markers), with no
/// allocation.
pub(crate) fn validate_recording(p: &Project, r: &Recording) -> Result<()> {
    check(!r.sources.is_empty(), code!("model.recording.noSources"))?;
    check(
        r.primary_source().is_some_and(|s| s.role.is_video()),
        code!("model.recording.noPrimary"),
    )?;
    for (k, s) in r.sources.iter().enumerate() {
        // A recording has a handful of sources, so the earlier ones are scanned (no allocation on every edit's
        // validation).
        let earlier = &r.sources[..k];
        check_with(earlier.iter().all(|e| e.id != s.id), || {
            msg!("model.project.duplicateId", id = s.id.as_str())
        })?;
        check_with(
            s.role.may_repeat() || earlier.iter().all(|e| e.role != s.role),
            || {
                msg!(
                    "model.recording.duplicateRole",
                    role = CodedError::new(role_code(s.role))
                )
            },
        )?;
        check(
            s.label.chars().count() <= MAX_NAME,
            code!("model.recording.label"),
        )?;
        check(
            (-MAX_NUDGE_US..=MAX_NUDGE_US).contains(&s.nudge_us),
            code!("model.recording.nudge"),
        )?;
        validate_mix(p, s, &s.mix)?;
        validate_runs(p, s)?;
    }
    let length = r.duration_us();
    let mut last_at = 0;
    for m in &r.markers {
        check(!m.id.is_empty(), code!("model.marker.id"))?;
        check(
            (0..=length).contains(&m.at_us),
            code!("model.marker.outside"),
        )?;
        check(m.at_us >= last_at, code!("model.marker.order"))?;
        last_at = m.at_us;
        check(
            m.label
                .as_ref()
                .is_none_or(|l| l.chars().count() <= MAX_NAME),
            code!("model.marker.label"),
        )?;
    }
    Ok(())
}

/// A source's runs: in order on the recording clock and never overlapping there, each on a file of the source's kind,
/// each inside its file (`fileInUs` + its length), and two runs of one file (a recorder paused) never over the
/// same stretch of it. O(runs² + runs × media); a source has a handful of runs.
fn validate_runs(p: &Project, s: &RecordingSource) -> Result<()> {
    check(!s.runs.is_empty(), code!("model.recording.noRuns"))?;
    let mut last_end = 0;
    for (k, run) in s.runs.iter().enumerate() {
        check(
            in_range(run.drift_ppm, -MAX_DRIFT_PPM, MAX_DRIFT_PPM),
            code!("model.recording.drift"),
        )?;
        check(
            valid_time(run.offset_us) && valid_time(run.duration_us) && valid_time(run.file_in_us),
            code!("model.recording.noRuns"),
        )?;
        let (from, to) = run.rec_span(0);
        check(from >= last_end, code!("model.recording.runsOverlap"))?;
        last_end = to;
        let asset = p.asset(&run.asset_id);
        if s.role.is_video() {
            check(
                asset.is_some_and(|a| a.kind == AssetKind::Video),
                code!("model.recording.missingVideo"),
            )?;
        } else {
            check(
                asset.is_some_and(|a| a.has_audio),
                code!("model.recording.missingAudio"),
            )?;
        }
        let end = run.file_in_us.saturating_add(run.duration_us);
        check(
            asset.is_some_and(|a| end <= a.duration_us.saturating_add(SOURCE_SLACK_US)),
            code!("model.recording.runInFile"),
        )?;
        check(
            s.runs[..k].iter().all(|e| {
                e.asset_id != run.asset_id
                    || e.file_in_us.saturating_add(e.duration_us) <= run.file_in_us
                    || end <= e.file_in_us
            }),
            code!("model.recording.runsOverlap"),
        )?;
    }
    Ok(())
}

/// A mix of source `s`, its default or a clip's override: 0–200 %, up to eight audio effects, centred from a channel a
/// file plays, and filling gaps only for a picture source whose files carry sound (a video's own sound).
pub(super) fn validate_mix(p: &Project, s: &RecordingSource, mix: &SourceMix) -> Result<()> {
    check(in_range(mix.volume, 0.0, 2.0), code!("model.source.volume"))?;
    check_centre(mix.centre_from)?;
    check(
        !mix.fills_gaps || (s.role.is_video() && makes_sound(p, s)),
        code!("model.source.fillsGapsPicture"),
    )?;
    validate_stack(p, &mix.effects, ContribKind::AudioEffect)
}

/// A sound plays centred from its first or second channel (`PLAYED_CHANNELS`), or as recorded.
pub(super) fn check_centre(channel: Option<u8>) -> Result<()> {
    check(
        channel.is_none_or(|k| k < PLAYED_CHANNELS),
        code!("model.source.centreFrom"),
    )
}
