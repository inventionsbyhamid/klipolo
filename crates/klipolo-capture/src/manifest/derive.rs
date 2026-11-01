//! `derive`: the manifest from the config, the journal, the anchors and the files' measured lengths (the rules are the
//! module doc of `manifest`).

use std::collections::BTreeMap;

use klipolo_model::coded::CodedError;
use klipolo_model::project::Marker;
use klipolo_model::time::Us;

use super::{Manifest, ManifestRun, ManifestSource, source_param};
use crate::drift::{self, Anchor, Fit};
use crate::english::msg;
use crate::journal::{Entry, JournalRead};
use crate::media::SourceId;
use crate::session::{FileKey, SessionConfig};

/// A fit drifting more than this is refused: real devices drift ±10-100 ppm.
const MAX_DRIFT_PPM: f64 = 1_000.0;
/// An anchor this far off the fitted line belongs to another line.
const OFF_LINE_US: f64 = 40_000.0;
/// A fit with this share of its anchors off the line or more is refused...
const MAX_OFF_LINE: f64 = 0.10;
/// ...when at least this many are off: one stray anchor in a short run isn't a second line.
const MIN_OFF_LINE: usize = 2;
/// Gaps between runs shorter than this aren't noted.
const MIN_GAP_US: Us = 50_000;

/// A run as the journal tells it.
struct Told<'a> {
    file: &'a str,
    offset_us: Us,
    rate: Option<u32>,
    ended_us: Option<Us>,
}

/// The manifest of a take. `durations`: each run file's recorded length (fMP4 index or CAF frames), measured by the
/// caller.
pub fn derive(
    cfg: &SessionConfig,
    journal: &JournalRead,
    anchors: &BTreeMap<(SourceId, u32), Vec<Anchor>>,
    durations: &BTreeMap<FileKey, Us>,
) -> Manifest {
    let mut told: BTreeMap<(&str, u32), Told> = BTreeMap::new();
    let mut levels: BTreeMap<&str, &Vec<f32>> = BTreeMap::new();
    let mut markers: BTreeMap<&str, Marker> = BTreeMap::new();
    let mut notes = Vec::new();
    let (mut finalized, mut recovered) = (None, false);
    for entry in &journal.entries {
        match entry {
            Entry::RunStarted {
                source,
                run,
                file,
                offset_us,
                audio,
                ..
            } => {
                told.insert(
                    (source, *run),
                    Told {
                        file,
                        offset_us: *offset_us,
                        rate: audio.map(|a| a.rate),
                        ended_us: None,
                    },
                );
            }
            Entry::RunEnded {
                source,
                run,
                end_us,
            } => {
                if let Some(t) = told.get_mut(&(source.as_str(), *run)) {
                    t.ended_us = Some(*end_us);
                }
            }
            Entry::Marker { id, at_us, kind } => {
                let marker = Marker {
                    id: id.clone(),
                    at_us: *at_us,
                    kind: *kind,
                    label: None,
                };
                markers.insert(id, marker);
            }
            Entry::Health {
                code,
                critical: true,
                params,
                ..
            } => {
                let note = CodedError {
                    code: code.clone(),
                    params: params.clone(),
                };
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
            // Only levels for the whole run: a running level (`through`) covers part of it.
            Entry::Levels {
                file,
                peaks,
                through: None,
            } => {
                levels.insert(file, peaks);
            }
            Entry::Finalized { duration_us } => finalized = Some(*duration_us),
            Entry::Recovered { .. } => recovered = true,
            _ => {}
        }
    }

    let mut refused = false;
    let mut sources = Vec::new();
    let mut latest_end = 0;
    for source in &cfg.sources {
        let mut runs: Vec<ManifestRun> = Vec::new();
        let mut last_end: Option<Us> = None;
        for ((_, run), t) in told.range((source.id.as_str(), 0)..=(source.id.as_str(), u32::MAX)) {
            let Some(&duration_us) = durations.get(t.file).filter(|&&d| d > 0) else {
                tracing::warn!(
                    file = t.file,
                    "manifest: a run without a measured length is left out"
                );
                continue;
            };
            // A sound run's own line: its anchors and its rate, fitted.
            let fit = match (
                source.role.is_video(),
                anchors.get(&(source.id.clone(), *run)),
                t.rate,
            ) {
                (false, Some(anchors), Some(rate)) => {
                    drift::fit(anchors, rate).map(|fit| (fit, anchors, rate))
                }
                _ => None,
            };
            let trusted = fit.filter(|&(fit, anchors, rate)| is_one_line(&fit, anchors, rate));
            refused |= fit.is_some() && trusted.is_none();
            let channel_peaks = levels.get(t.file).map(|&p| p.clone());
            let r = match trusted {
                Some((fit, ..)) => ManifestRun {
                    file: t.file.to_owned(),
                    offset_us: fit.offset_us,
                    duration_us,
                    drift_ppm: fit.drift_ppm,
                    approximate: false,
                    channel_peaks,
                },
                None => ManifestRun {
                    file: t.file.to_owned(),
                    offset_us: t.offset_us,
                    duration_us,
                    drift_ppm: 0.0,
                    approximate: !source.role.is_video(),
                    channel_peaks,
                },
            };
            if let Some(end) = last_end.filter(|&end| r.offset_us - end >= MIN_GAP_US) {
                notes.push(gap(&source.label, end, r.offset_us - end));
            }
            // A gap is measured from where the media ends. `RunEnded` is later than that exactly when the source
            // stopped delivering before its run was ended (a lost device is noticed up to 3 s on), so it may lengthen
            // the take but never shortens the hole.
            let end = r.end_us();
            latest_end = latest_end.max(end.max(t.ended_us.unwrap_or(0)));
            last_end = Some(end);
            runs.push(r);
        }
        if !runs.is_empty() {
            sources.push(ManifestSource {
                id: source.id.clone(),
                role: source.role,
                label: source.label.clone(),
                runs,
            });
        }
    }
    if refused {
        notes.push(msg!("capture.recovered.approximate"));
    }
    let cursor = super::cursor::cursor_of(cfg, journal, &sources);
    let mut markers: Vec<Marker> = markers.into_values().collect();
    markers.sort_by_key(|m| m.at_us);
    Manifest {
        session: cfg.id.clone(),
        kind: cfg.kind,
        duration_us: finalized.unwrap_or(0).max(latest_end),
        sources,
        markers,
        cursor,
        game: cfg.game.clone(),
        notes,
        recovered,
        target: cfg.target.clone(),
    }
}

/// Whether `fit` is one plausible line through `anchors` (the module doc's checks).
fn is_one_line(fit: &Fit, anchors: &[Anchor], rate: u32) -> bool {
    if fit.drift_ppm.abs() > MAX_DRIFT_PPM {
        return false;
    }
    let us_per_frame = 1e6 / (f64::from(rate) * (1.0 + fit.drift_ppm * 1e-6));
    let off = anchors
        .iter()
        .filter(|a| {
            let on_line = fit.offset_us as f64 + a.frame as f64 * us_per_frame;
            (a.session_us as f64 - on_line).abs() >= OFF_LINE_US
        })
        .count();
    off < MIN_OFF_LINE || (off as f64) < MAX_OFF_LINE * anchors.len() as f64
}

/// `capture.note.gap` for a source labelled `label` that stopped at `at_us` for `len_us`.
fn gap(label: &str, at_us: Us, len_us: Us) -> CodedError {
    let source = source_param(label);
    let seconds = (len_us as f64 / 100_000.0).round() / 10.0;
    msg!(
        "capture.note.gap",
        source = source,
        seconds = seconds,
        atUs = at_us
    )
}
