#![cfg(feature = "synthetic")]
//! More recovery cases: progress and cancel, one bad run never
//! stopping the take, a head checked against the journal's stream, and a crashed sound run's levels (read only after
//! The last tick).
// No test in this file may spawn a process (klipolo-platform's SPAWN; see recover_rules.rs).
mod common;

use common::take::{Take, write_take};
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::recover::{RecoverError, RepairWatch, repair, repair_watched};

struct Watch {
    seen: Vec<(u64, u64)>,
    cancel_after: Option<usize>,
}

impl RepairWatch for Watch {
    fn progress(&mut self, done: u64, total: u64) {
        self.seen.push((done, total));
    }
    fn cancelled(&self) -> bool {
        self.cancel_after.is_some_and(|n| self.seen.len() >= n)
    }
}

#[test]
fn a_repair_reports_its_progress_and_can_be_cancelled_then_goes_on() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "p",
        Take {
            seconds: 4.0,
            ..Take::default()
        },
    );
    // Without its journal, every sample is checked: the long repair the app runs as a background job.
    std::fs::remove_file(t.dir.journal()).unwrap();
    let screen = std::fs::read(t.dir.path("screen/run-01.mp4")).unwrap();
    let mut watch = Watch {
        seen: vec![],
        cancel_after: Some(2),
    };
    let r = repair_watched(&t.dir, &klipolo_capture::io::OsIo, &mut watch);
    assert_eq!(r.unwrap_err(), RecoverError::Cancelled);
    assert_eq!(
        std::fs::read(t.dir.path("screen/run-01.mp4")).unwrap(),
        screen,
        "nothing written before the cancel"
    );
    let mut watch = Watch {
        seen: vec![],
        cancel_after: None,
    };
    let m = repair_watched(&t.dir, &klipolo_capture::io::OsIo, &mut watch)
        .unwrap()
        .manifest;
    assert_eq!(m.sources.len(), 2);
    let (done, total) = *watch.seen.last().unwrap();
    assert!(total > 0 && done == total, "{:?}", watch.seen.last());
    assert!(
        watch.seen.windows(2).all(|w| w[1].0 >= w[0].0),
        "progress only moves forward"
    );
}

#[test]
fn one_run_that_fails_is_left_out_named_and_the_rest_imports() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(root.path(), "b", Take::default());
    // A file the OS refuses to open as one (a bad sector or a permission behaves alike: an I/O error on that run only).
    let mic = t.dir.path("mic/run-01.caf");
    std::fs::remove_file(&mic).unwrap();
    std::fs::create_dir(&mic).unwrap();
    let m = repair(&t.dir).unwrap().manifest;
    assert_eq!(
        m.sources.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["screen"]
    );
    let note = m
        .notes
        .iter()
        .find(|n| n.code == "capture.recovered.sourceKept")
        .expect("the mic is named");
    // Its only run: the whole source, never "Part of" it.
    assert_eq!(
        klipolo_capture::english::english(note),
        "Mic couldn't be repaired automatically. Its files are kept in the take's folder."
    );
    assert!(mic.is_dir(), "kept as it was");
}

#[test]
fn a_head_that_reads_but_not_as_the_journal_says_is_rebuilt() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(root.path(), "h", Take::default());
    let path = t.dir.path("screen/run-01.mp4");
    let mut bytes = std::fs::read(&path).unwrap();
    // A byte inside the head's avcC (its SPS) garbled: the head still parses, but describes another stream.
    let config = common::take::fixture().info.config;
    let at = bytes
        .windows(config.len())
        .position(|w| w == config.as_slice())
        .expect("the avcC in the head");
    bytes[at + 12] ^= 0x5a;
    std::fs::write(&path, &bytes).unwrap();
    let m = repair(&t.dir).unwrap().manifest;
    let run = &m.sources.iter().find(|s| s.id == "screen").unwrap().runs[0];
    let fixed = std::fs::read(&path).unwrap();
    assert_eq!(
        &fixed[at..at + config.len()],
        config.as_slice(),
        "the head the journal describes"
    );
    let frames = common::decode::decode_all(&path).0 as i64;
    assert_eq!(frames, run.duration_us * 30 / 1_000_000);
}

#[test]
fn a_sound_run_a_crash_ended_gets_its_levels_once() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(root.path(), "l", Take::default());
    let m = repair(&t.dir).unwrap().manifest;
    let mic = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs[0];
    let peaks = mic.channel_peaks.clone().expect("measured from the file");
    // The take's mic is a −12 dBFS tone, mono.
    assert!(
        peaks.len() == 1 && (0.24..0.26).contains(&peaks[0]),
        "{peaks:?}"
    );
    repair(&t.dir).unwrap();
    let levels = journal::read(&t.dir.journal())
        .unwrap()
        .entries
        .iter()
        .filter(|e| matches!(e, Entry::Levels { through: None, .. }))
        .count();
    assert_eq!(levels, 1, "journaled once, however often repair runs");
}

#[test]
fn levels_are_read_only_after_the_last_tick() {
    // Each tick journals the sound run's levels so far, so recovery reads only what came after the last one:
    // a full-scale stretch planted a second in, long before the last tick, never shows.
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "tail",
        Take {
            seconds: 3.0,
            ..Take::default()
        },
    );
    let path = t.dir.path("mic/run-01.caf");
    let mut bytes = std::fs::read(&path).unwrap();
    let at = 68 + 48_000 * 3;
    bytes[at..at + 3 * 480].copy_from_slice(&[0xff, 0xff, 0x7f].repeat(480));
    std::fs::write(&path, &bytes).unwrap();
    let m = repair(&t.dir).unwrap().manifest;
    let mic = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs[0];
    let peaks = mic.channel_peaks.clone().unwrap();
    assert!(
        (0.24..0.26).contains(&peaks[0]),
        "only the tail was read: {peaks:?}"
    );
}

/// Home's scan skips an imported take by its marker alone, before its config, its lock
/// or its journal, so imported takes neither cost a 64 KB read each nor push an unfinished one off the list.
#[test]
fn an_imported_take_is_skipped_by_its_marker_alone() {
    use klipolo_capture::recover::{FoundState, SCAN_LIMIT, scan};
    use klipolo_capture::session::mark_imported;
    let root = tempfile::tempdir().unwrap();
    let take = |id: &str| {
        write_take(
            root.path(),
            id,
            Take {
                finalize: true,
                ..Take::default()
            },
        )
    };
    let (imported, _ready) = (take("imported"), take("ready"));
    // Its journal has no `Imported` line: only the marker says so.
    mark_imported(&imported.dir).unwrap();
    let found = scan(root.path(), SCAN_LIMIT);
    assert_eq!(
        found
            .iter()
            .map(|f| (f.id.as_str(), f.state))
            .collect::<Vec<_>>(),
        [("ready", FoundState::Ready)]
    );
}
