//! Import: a finished or recovered take becomes format 4's `Recording`, one asset per run,
//! and one main-track clip (`clip_for`). It only reads: the take's files stay where they are, whatever happens.
//!
//! - **Identity.** `Recording.id` is the session id, so a take imported twice is seen (the app skips
//!   it). Each source keeps its manifest id and its label (a `capture.source.*` code, copied as it is).
//! - **Assets.** Each run's file goes through the app's probe, named as in the take (`screen/run-01.mp4`).
//! - **Nothing goes silently, and no note hides a kept file.** A run whose file is there but doesn't probe, or probes
//!   as the wrong kind, is left out with `capture.import.unreadable`, which says its file is kept in the take's folder
//!   (as recovery's `capture.recovered.partKept` does); a run with no file gets a `capture.note.gap` note for its time.
//!   A run the manifest left out for having no measured length is named as a gap when nothing else
//!   names it: the manifest notes gaps between kept runs, and recovery names what it couldn't repair. A take with no
//!   picture left is refused: `capture.import.videoUnreadable` when its picture's file is there but can't be read (it
//!   is kept), else `capture.import.noVideo`.
//! - **Valid by construction** (the app applies it as one transaction, which validation would refuse whole):
//!   - each run stays inside its file (the shorter of the manifest's length and the probe's; a file shorter by 50 ms
//!     or more is noted as a gap);
//!   - video runs carry 0 ppm (the recorder fits drift on sound only; a separated camera layer ignores video drift);
//!   - a sound line fitted a hair before the start gate starts at 0, and runs of one source never overlap (a fitted
//!     line may reach a few µs into the next run, which then wins);
//!   - markers are sorted by time and kept inside the recording.
//! - **`primary`** is the screen or game source when it has runs, else the camera.
//! - **A one-sided mic plays centred**: a mic source whose other channel stayed silent the whole take
//!   gets `SourceMix::centre_from` (`centred_channel`), from each run's peaks (`peaks`). Its files keep both channels.
//!   Other sounds play as recorded (a hard-panned demo was heard one-sided while recording); chat's `centreSource`
//!   centres any of them on request.
//!
//! Cost: one probe per run, one pass over the journal (streamed; about 1 MB an hour), O(runs log runs + markers log
//! markers) otherwise.

mod cursor;
mod left_out;
mod runs;

use std::path::Path;

use klipolo_model::coded::CodedError;
use klipolo_model::project::{Asset, Recording, RecordingSource, SourceMix, SourceRole, SourceRun};
use klipolo_model::time::Us;
use klipolo_model::timeline::{CameraLayout, Clip, ClipSource};

use crate::english::msg;
use crate::manifest::{Manifest, ManifestRun, source_param};
use crate::session::SessionDir;
use left_out::{left_out, unmeasured};
use runs::{apart, markers, run_from};

/// Gaps and shortfalls under this aren't noted (as the manifest's gaps).
const MIN_GAP_US: Us = 50_000;
/// A channel whose loudest sample stays at or under −60 dBFS carries nothing (`centred_channel`).
const SILENT_PEAK: f32 = 0.001;

#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    pub recording: Recording,
    pub assets: Vec<Asset>,
}

/// `probe` turns a run file into an `Asset` (the app's media probe, which gives a new id). A run whose file doesn't
/// probe is skipped with a note (`left_out`); a source with no runs left is dropped. `peaks` gives a run's loudest sample per
/// channel when the take has them (`manifest_peaks`), and a mic whose kept runs say it is one-sided plays centred
/// (`centred_channel`). The notes are import's own; the manifest's (`m.notes`) come before them. When no picture is
/// left: Err(`capture.import.videoUnreadable`) if a picture's file is there but can't be read (it is kept), else
/// Err(`capture.import.noVideo`).
pub fn recording_from(
    m: &Manifest,
    dir: &SessionDir,
    peaks: &dyn Fn(&ManifestRun) -> Option<&[f32]>,
    probe: &mut dyn FnMut(&Path, &str) -> Result<Asset, String>,
) -> Result<(Imported, Vec<CodedError>), CodedError> {
    let mut notes = Vec::new();
    let mut assets = Vec::new();
    let mut sources = Vec::new();
    // A picture file that is there but couldn't be read (kept in the take's folder).
    let mut picture_kept = false;
    for s in &m.sources {
        let mut runs = Vec::new();
        // Each kept run's file and peaks.
        let mut levels = Vec::new();
        for run in &s.runs {
            match run_from(dir, s, run, probe, &mut notes) {
                Some((r, asset)) => {
                    levels.push((asset.id.clone(), peaks(run)));
                    runs.push(r);
                    assets.push(asset);
                }
                None => {
                    let (note, kept) = left_out(dir, s, run);
                    picture_kept |= kept && s.role.is_video();
                    notes.push(note);
                }
            }
        }
        apart(&mut runs, &s.label, &mut notes);
        if !runs.is_empty() {
            levels.retain(|(file, _)| runs.iter().any(|r| r.asset_id == *file));
            let centre_from = (s.role == SourceRole::Mic)
                .then(|| centred_channel(levels.iter().map(|&(_, p)| p)))
                .flatten();
            // A picture file with sound (a recorder muxing it in) plays muted: it never doubles the mic.
            let heard = |r: &SourceRun| {
                assets
                    .iter()
                    .any(|a: &Asset| a.id == r.asset_id && a.has_audio)
            };
            let muted = s.role.is_video() && runs.iter().any(heard);
            sources.push(RecordingSource {
                id: s.id.clone(),
                role: s.role,
                label: s.label.clone(),
                runs,
                nudge_us: 0,
                mix: SourceMix {
                    centre_from,
                    muted,
                    ..SourceMix::default()
                },
                synced: None,
            });
        }
    }
    for note in unmeasured(m, dir) {
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    notes.retain(|n| !m.notes.contains(n));
    let with = |roles: &[SourceRole]| sources.iter().find(|s| roles.contains(&s.role));
    let primary = with(&[SourceRole::Screen, SourceRole::Game])
        .or_else(|| with(&[SourceRole::Camera]))
        .map(|s| s.id.clone())
        .ok_or_else(|| {
            if picture_kept {
                msg!("capture.import.videoUnreadable")
            } else {
                msg!("capture.import.noVideo")
            }
        })?;
    let mut recording = Recording {
        id: m.session.clone(),
        kind: m.kind,
        sources,
        primary,
        cursor: cursor::recording_cursor(m, dir),
        markers: Vec::new(),
        game: m.game.clone(),
    };
    recording.markers = markers(&m.markers, recording.duration_us());
    Ok((Imported { recording, assets }, notes))
}

/// The whole take as one main-track clip, its camera (a bubble, or the main footage) mirrored as the recorder's "Mirror
/// my camera" was; framing left to the defaults.
pub fn clip_for(rec: &Recording, mirror: bool) -> Clip {
    let mut clip = Clip::new(
        ClipSource::Recording { id: rec.id.clone() },
        0,
        rec.duration_us(),
    );
    if rec.camera().is_some() || rec.camera_only() {
        let camera = CameraLayout::default();
        clip.camera = Some(CameraLayout { mirror, ..camera });
    }
    clip
}

/// The channel a sound source plays from, centred (`SourceMix::centre_from`), when its other channel stayed silent the
/// whole take (a mic on input 1 of a two-input interface records [voice, silence], heard from the
/// left only). `runs`: each run's loudest sample per channel, linear 0-1 (the manifest's per-channel peaks),
/// None where a run has none. Some only when every two-channel run with sound has one channel at or under −60 dBFS and
/// the same other one live, and there is one. A one-channel run (a heal onto the built-in mic) plays centred either way,
/// and a run silent on both sides (a short reconnect, a quiet stretch) doesn't say otherwise, so neither counts.
/// Anything else plays as recorded. The files always keep both channels: this only says how they play.
pub fn centred_channel<'a>(runs: impl IntoIterator<Item = Option<&'a [f32]>>) -> Option<u8> {
    let mut live = None;
    for peaks in runs {
        let one = match *peaks? {
            [_] => continue,
            [a, b] if a <= SILENT_PEAK && b <= SILENT_PEAK => continue,
            [a, b] if a > SILENT_PEAK && b <= SILENT_PEAK => 0,
            [a, b] if b > SILENT_PEAK && a <= SILENT_PEAK => 1,
            _ => return None,
        };
        if live.replace(one).is_some_and(|before| before != one) {
            return None;
        }
    }
    live
}

/// A run's loudest sample per channel as the take's manifest keeps them (`ManifestRun::channel_peaks`, the recorder's
/// whole-run `Levels`), for `recording_from`'s `peaks`. None where the recorder couldn't measure a run (recovery's
/// `capture.note.levels`) or for video: that run then plays as recorded.
pub fn manifest_peaks(run: &ManifestRun) -> Option<&[f32]> {
    run.channel_peaks.as_deref()
}

/// A manifest run's length on the recording clock.
pub(super) fn rec_len(run: &ManifestRun) -> Us {
    (run.duration_us as f64 / (1.0 + run.drift_ppm * 1e-6)).round() as Us
}

/// `capture.note.gap`: `label`'s source has nothing for `len_us` from `at_us` (as the manifest words its gaps).
fn gap(label: &str, at_us: Us, len_us: Us) -> CodedError {
    msg!(
        "capture.note.gap",
        source = source_param(label),
        seconds = seconds(len_us),
        atUs = at_us
    )
}

/// A length in seconds as the notes give it, to a tenth.
pub(super) fn seconds(len_us: Us) -> f64 {
    (len_us as f64 / 100_000.0).round() / 10.0
}
