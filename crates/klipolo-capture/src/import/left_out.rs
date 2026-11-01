//! What import leaves out of the edit, named: a run whose file can't be read (kept in the take's folder), and the runs a
//! take's journal opened that its manifest left out for having no measured length.

use std::collections::BTreeMap;

use klipolo_model::coded::CodedError;
use klipolo_model::time::Us;

use super::{MIN_GAP_US, gap, rec_len, seconds};
use crate::english::msg;
use crate::journal::{self, Entry};
use crate::manifest::{Manifest, ManifestRun, ManifestSource, source_param};
use crate::session::{self, SessionDir};

/// The note for run `run` of `s`, left out of the edit, and whether its file is kept: a file that is there but can't be
/// read (it doesn't probe, probes as the wrong kind or has no length) stays in the take's folder, and the note says so
/// (`capture.import.unreadable`); with no file there, the source has nothing for that time (`capture.note.gap`).
pub(super) fn left_out(
    dir: &SessionDir,
    s: &ManifestSource,
    run: &ManifestRun,
) -> (CodedError, bool) {
    let kept = dir.key_path(&run.file).is_ok_and(|path| path.is_file());
    let note = if kept {
        msg!(
            "capture.import.unreadable",
            source = source_param(&s.label),
            seconds = seconds(rec_len(run)),
            atUs = run.offset_us
        )
    } else {
        gap(&s.label, run.offset_us, rec_len(run))
    };
    (note, kept)
}

/// Notes for the runs the journal opened but the manifest left out, having no measured length,
/// where nothing else names them: a source's leading or trailing runs, or all of them. One between two kept runs lies
/// in a gap the manifest notes, and recovery names what it couldn't repair. A take without a journal has none.
pub(super) fn unmeasured(m: &Manifest, dir: &SessionDir) -> Vec<CodedError> {
    let Ok(read) = journal::read(&dir.journal()) else {
        return Vec::new();
    };
    // (source, run) → (file, offset, end): the end from `RunEnded`, else the source's next run, else the take's end.
    let mut opened: BTreeMap<(&str, u32), (&str, Us, Option<Us>)> = BTreeMap::new();
    for e in &read.entries {
        match e {
            Entry::RunStarted {
                source,
                run,
                file,
                offset_us,
                ..
            } => {
                opened.insert((source.as_str(), *run), (file.as_str(), *offset_us, None));
            }
            Entry::RunEnded {
                source,
                run,
                end_us,
            } => {
                if let Some(o) = opened.get_mut(&(source.as_str(), *run)) {
                    o.2 = Some(*end_us);
                }
            }
            _ => {}
        }
    }
    let kept = |file: &str| {
        m.sources
            .iter()
            .flat_map(|s| &s.runs)
            .any(|r| r.file == file)
    };
    let mut labels: Option<BTreeMap<String, String>> = None;
    let mut notes = Vec::new();
    let keys: Vec<_> = opened.keys().copied().collect();
    for (k, &(source, run)) in keys.iter().enumerate() {
        let (file, offset, ended) = opened[&(source, run)];
        if kept(file) {
            continue;
        }
        let next = keys
            .get(k + 1)
            .filter(|n| n.0 == source)
            .map(|n| opened[n].1);
        let end = ended.or(next).unwrap_or(m.duration_us).max(offset);
        let ms = m.sources.iter().find(|s| s.id == source);
        // Runs follow one another, so a kept run that starts earlier is before it, and one that starts later after it.
        let spans = || ms.into_iter().flat_map(|s| &s.runs);
        let between =
            spans().any(|r| r.offset_us < offset) && spans().any(|r| r.offset_us > offset);
        let label = match ms {
            Some(s) => s.label.clone(),
            None => labels
                .get_or_insert_with(|| config_labels(dir))
                .get(source)
                .cloned()
                .unwrap_or_else(|| source.to_owned()),
        };
        let named = m.notes.iter().any(|n| {
            matches!(
                n.code.as_str(),
                "capture.recovered.partKept" | "capture.recovered.sourceKept"
            ) && n.params.get("source") == Some(&source_param(&label))
        });
        if !between && !named && end - offset >= MIN_GAP_US {
            tracing::warn!(file, "import: a run without a measured length");
            notes.push(gap(&label, offset, end - offset));
        }
    }
    notes
}

/// Each configured source's label by id (a source the manifest dropped whole is named from the config).
fn config_labels(dir: &SessionDir) -> BTreeMap<String, String> {
    session::read_config(dir)
        .map(|cfg| cfg.sources.into_iter().map(|s| (s.id, s.label)).collect())
        .unwrap_or_default()
}
