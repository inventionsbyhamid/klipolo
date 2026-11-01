//! A take's runs as format 4 takes them (`recording_from`'s steps): each run with its probed asset, a source's runs kept
//! apart, and the markers inside the recording.

use std::path::Path;

use klipolo_model::coded::CodedError;
use klipolo_model::project::{Asset, AssetKind, Marker, SourceRun};
use klipolo_model::time::Us;
use klipolo_model::validate::MAX_DRIFT_PPM;

use super::{MIN_GAP_US, gap, rec_len};
use crate::english::msg;
use crate::manifest::{ManifestRun, ManifestSource, source_param};
use crate::session::SessionDir;

/// Run `run` of `s` as format 4 takes it, with its probed asset: None when its file can't be part of the take.
pub(super) fn run_from(
    dir: &SessionDir,
    s: &ManifestSource,
    run: &ManifestRun,
    probe: &mut dyn FnMut(&Path, &str) -> Result<Asset, String>,
    notes: &mut Vec<CodedError>,
) -> Option<(SourceRun, Asset)> {
    let path = dir
        .key_path(&run.file)
        .inspect_err(
            |e| tracing::warn!(file = %run.file, error = %e, "import: a run outside the take"),
        )
        .ok()?;
    let mut asset = probe(&path, &run.file)
        .inspect_err(
            |e| tracing::warn!(file = %run.file, error = %e, "import: a run that doesn't probe"),
        )
        .ok()?;
    let fits = if s.role.is_video() {
        asset.kind == AssetKind::Video
    } else {
        asset.has_audio
    };
    if !fits || asset.duration_us <= 0 {
        tracing::warn!(file = %run.file, kind = ?asset.kind, len = asset.duration_us, "import: a run whose file holds no media of its kind");
        return None;
    }
    asset.name = run.file.clone();
    let drift_ppm = if s.role.is_video() {
        0.0
    } else {
        run.drift_ppm.clamp(-MAX_DRIFT_PPM, MAX_DRIFT_PPM)
    };
    let r = SourceRun {
        asset_id: asset.id.clone(),
        offset_us: run.offset_us.max(0),
        duration_us: run.duration_us.min(asset.duration_us),
        drift_ppm,
        file_in_us: 0,
    };
    let short = run.duration_us - r.duration_us;
    if short >= MIN_GAP_US {
        let (from, end) = r.rec_span(0);
        notes.push(gap(&s.label, end, rec_len(run) - (end - from)));
        tracing::warn!(file = %run.file, short_us = short, "import: a file shorter than its run");
    }
    Some((r, asset))
}

/// `runs` in time order, each ending where the next begins at the latest. A run the next one covers whole is left
/// out, and named (its file stays in the take's folder).
pub(super) fn apart(runs: &mut Vec<SourceRun>, label: &str, notes: &mut Vec<CodedError>) {
    runs.sort_by_key(|r| r.offset_us);
    let mut i = 0;
    while i + 1 < runs.len() {
        let next = runs[i + 1].offset_us;
        let r = &mut runs[i];
        if r.rec_span(0).1 > next {
            let room = (next - r.offset_us) as f64 * (1.0 + r.drift_ppm * 1e-6);
            r.duration_us = room.floor() as Us;
        }
        if r.duration_us <= 0 {
            let note = msg!(
                "capture.recovered.partKept",
                source = source_param(label),
                atUs = r.offset_us
            );
            tracing::warn!(asset = %r.asset_id, "import: a run another one covers whole");
            notes.push(note);
            runs.remove(i);
        } else {
            i += 1;
        }
    }
}

/// The markers sorted by time (stable: one moment keeps its order) and inside [0, `len`]; ids must be set.
pub(super) fn markers(from: &[Marker], len: Us) -> Vec<Marker> {
    let mut out: Vec<Marker> = from
        .iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| Marker {
            at_us: m.at_us.clamp(0, len),
            ..m.clone()
        })
        .collect();
    out.sort_by_key(|m| m.at_us);
    out
}
