//! A sound run's levels are a hint for import: a file that can't be read for them never fails a repair or a resume.
//! Its own test binary, because the failpoint it arms holds for the whole process.
#![cfg(feature = "synthetic")]

mod common;

use klipolo_capture::fail::{self, FailAction};
use klipolo_capture::journal::{self, Entry};

#[test]
fn a_levels_read_error_never_fails_repair_or_resume() {
    let root = tempfile::tempdir().unwrap();
    let crashed = || common::take::Take {
        finalize: false,
        ..common::take::Take::default()
    };
    let repaired = common::take::write_take(root.path(), "repaired", crashed());
    let resumed = common::take::write_take(root.path(), "resumed", crashed());
    fail::set(
        "recover.levels",
        Some(FailAction::Error(std::io::ErrorKind::Other)),
    );

    let r = klipolo_capture::recover::repair(&repaired.dir).expect("the repair goes on");
    assert_eq!(r.manifest.sources.len(), 2, "every durable source imports");
    for s in &r.manifest.sources {
        assert_eq!(s.runs.len(), 1, "{}", s.id);
    }
    assert!(
        (r.manifest.notes.iter()).any(|n| n.code == "capture.note.levels"),
        "a mild note: {:?}",
        r.manifest.notes
    );
    // No levels, matching the note: the running ones cover only part of the run.
    let mic = &r
        .manifest
        .sources
        .iter()
        .find(|s| s.id == "mic")
        .unwrap()
        .runs[0];
    assert_eq!(mic.channel_peaks, None);

    let (_lock, from) =
        klipolo_capture::run::resume::resume(&resumed.dir, 0).expect("the resume goes on");
    assert_eq!(from.done.len(), 2, "both runs finished: {:?}", from.done);
    let entries = journal::read(&resumed.dir.journal()).unwrap().entries;
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Health { code, .. } if code == "capture.note.levels"))
    );
    fail::set("recover.levels", None);
}
