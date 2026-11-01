//! The core on real threads with the fake backend: its pipes send the fixture's real H.264 and synthetic sound
//! in real time, so each test takes a few seconds. The take finalizes into files FFmpeg decodes; the parent going away
//! orphans the worker, which records on; a resumed worker finishes the crashed runs first and keeps the
//! person's markers. The mic's heals are in `threaded_mic`, and Stop's consent cases in `threaded_stop`.
#![cfg(feature = "synthetic")]

mod common;

use std::io::{BufReader, Write};
use std::time::Duration;

use common::harness::Src;
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest::Manifest;
use klipolo_capture::protocol::Command;
use klipolo_capture::run::threads::{run_session, run_with};
use klipolo_capture::shell::ShellOutput;
use klipolo_capture::synthetic::{FakeBackend, FakeShell, Script, VideoFeed};

fn manifest(dir: &klipolo_capture::session::SessionDir) -> Manifest {
    serde_json::from_slice(&std::fs::read(dir.manifest()).unwrap()).unwrap()
}

#[test]
fn a_threaded_take_finalizes_into_regular_files_that_decode() {
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let code = run_with(
        common::harness::fake_backend(),
        dir.clone(),
        false,
        common::harness::script_start_then_stop_after_ms(3_000),
    );
    assert_eq!(code, 0);
    let m = manifest(&dir);
    let screen = &m.sources.iter().find(|s| s.id == "screen").unwrap().runs[0];
    // Checked against its own file and the take's own Stop, never the host's clock: a busy host may open the gate late
    // or drop frames, and the take is still right.
    let path = dir.path(&screen.file);
    let file = std::fs::File::open(&path).unwrap();
    let walked =
        klipolo_capture::fmp4::read::walk(&mut &file, file.metadata().unwrap().len()).unwrap();
    let samples: u64 = (walked.index.fragments.iter())
        .map(|f| f.samples.len() as u64)
        .sum();
    assert!(
        walked.remuxed && samples >= 30,
        "{samples} frames, remuxed: {}",
        walked.remuxed
    );
    assert_eq!(
        common::decode::decode_all(&path).0,
        samples,
        "every frame the file holds decodes"
    );
    let entries = journal::read(&dir.journal()).unwrap().entries;
    let stop_us = (entries.iter())
        .find_map(|e| match e {
            Entry::StopRequested { at_us, .. } => Some(*at_us),
            _ => None,
        })
        .unwrap();
    let frame_us = 1_000_000 / 30 + 1;
    assert!(
        screen.offset_us + screen.duration_us <= stop_us + frame_us,
        "nothing after the Stop: {screen:?}, Stop at {stop_us}"
    );
    assert!(
        screen.duration_us >= (samples as i64 - 1) * 1_000_000 / 30,
        "its length holds every frame: {screen:?}"
    );
    // A video run carries no drift (a separated camera layer ignores video drift); the mic's run is
    // placed by its own line and keeps its level.
    assert_eq!(screen.drift_ppm, 0.0);
    let mic = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs[0];
    assert!(
        mic.channel_peaks
            .as_ref()
            .is_some_and(|p| p.len() == 1 && p[0] > 0.05),
        "{mic:?}"
    );
    assert!(entries.iter().any(|e| matches!(e, Entry::Finalized { .. })));
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Remux { done: true, .. }))
    );
}

#[test]
fn closing_stdin_orphans_the_worker_and_it_keeps_recording() {
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let (shell, outputs) = FakeShell::new(vec![]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        ..Script::default()
    })
    .with_shell(shell);
    let (reader, mut writer) = std::io::pipe().unwrap();
    let session_dir = dir.clone();
    let worker = std::thread::spawn(move || {
        run_session(
            Box::new(backend),
            session_dir,
            false,
            Box::new(BufReader::new(reader)),
            Box::new(std::io::sink()),
        )
    });
    writer.write_all(b"{\"cmd\":\"start\"}\n").unwrap();
    std::thread::sleep(Duration::from_millis(2_000));
    drop(writer); // the app is gone
    let screen = dir.path("screen/run-01.mp4");
    let at_drop = std::fs::metadata(&screen).unwrap().len();
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(
        std::fs::metadata(&screen).unwrap().len() > at_drop,
        "the take keeps growing"
    );
    let orphan =
        klipolo_capture::english::english(&klipolo_model::coded::CodedError::new("capture.orphan"));
    let said = common::received_within(&outputs, 200)
        .into_iter()
        .any(|o| matches!(o, ShellOutput::Status(v) if v.lines.contains(&orphan)));
    assert!(said, "the shell says the worker is orphaned");
    // The parent's death is never a Stop: nothing finished the take.
    assert!(
        !journal::read(&dir.journal())
            .unwrap()
            .entries
            .iter()
            .any(|e| matches!(e, Entry::StopRequested { .. }))
    );
    drop(worker); // the worker records on until the test process ends
}

#[test]
fn a_resumed_worker_finishes_the_crashed_runs() {
    let root = tempfile::tempdir().unwrap();
    let crashed = common::take::Take {
        finalize: false,
        cut_last_fragment: true,
        ..common::take::Take::default()
    };
    let t = common::take::write_take(root.path(), "take", crashed);
    let code = run_with(
        common::harness::fake_backend(),
        t.dir.clone(),
        true,
        vec![(1_500, Command::Stop)],
    );
    assert_eq!(code, 0);
    let walk = |key: &str| {
        let file = std::fs::File::open(t.dir.path(key)).unwrap();
        klipolo_capture::fmp4::read::walk(&mut &file, file.metadata().unwrap().len()).unwrap()
    };
    let first = walk("screen/run-01.mp4");
    assert!(first.remuxed, "the crashed run is remuxed");
    let entries = journal::read(&t.dir.journal()).unwrap().entries;
    let salvaged_end = first.index.duration_us(90_000);
    let ended = entries.iter().find_map(|e| match e {
        Entry::RunEnded {
            source,
            run: 1,
            end_us,
        } if source == "screen" => Some(*end_us),
        _ => None,
    });
    assert_eq!(
        ended,
        Some(salvaged_end),
        "RunEnded sits at the salvaged end"
    );
    let m = manifest(&t.dir);
    for s in &m.sources {
        assert_eq!(s.runs.len(), 2, "{}: run 2 exists", s.id);
    }
    for run in &m.sources.iter().find(|s| s.id == "screen").unwrap().runs {
        let frames = common::decode::decode_all(&t.dir.path(&run.file)).0 as i64;
        assert!(
            (frames - run.duration_us * 30 / 1_000_000).abs() <= 1,
            "{}: {frames} frames",
            run.file
        );
    }
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Health { code, .. } if code == "capture.note.gap"))
    );
}

#[test]
fn markers_placed_before_a_crash_are_kept_after_the_resume() {
    // Three marks, a kill, a resume and two more marks: five markers, none overwritten.
    let root = tempfile::tempdir().unwrap();
    let crashed = common::take::Take {
        finalize: false,
        ..common::take::Take::default()
    };
    let t = common::take::write_take(root.path(), "take", crashed);
    let mut journal_file = journal::open_append(&t.dir.journal()).unwrap();
    for n in 1..=3 {
        let mark = Entry::Marker {
            id: format!("m{n}"),
            at_us: n * 400_000,
            kind: klipolo_model::project::MarkerKind::Hotkey,
        };
        journal_file.write_all(&journal::line(&mark)).unwrap();
    }
    drop(journal_file);
    let code = run_with(
        common::harness::fake_backend(),
        t.dir.clone(),
        true,
        vec![
            (400, Command::Marker),
            (800, Command::Marker),
            (1_500, Command::Stop),
        ],
    );
    assert_eq!(code, 0);
    let m = manifest(&t.dir);
    let ids: std::collections::BTreeSet<&str> = m.markers.iter().map(|k| k.id.as_str()).collect();
    assert_eq!(m.markers.len(), 5, "{:?}", m.markers);
    assert_eq!(ids.len(), 5, "every marker keeps its own id");
}

#[test]
fn a_take_recovery_finished_is_never_recorded_again_on_resume() {
    // Home's scan repaired the take between the worker's death and the app's `--resume`: it is finished, so nothing
    // opens.
    let root = tempfile::tempdir().unwrap();
    let crashed = common::take::Take {
        finalize: false,
        ..common::take::Take::default()
    };
    let t = common::take::write_take(root.path(), "take", crashed);
    klipolo_capture::recover::repair(&t.dir).unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        ..Script::default()
    });
    let opened = backend.opened();
    let code = run_with(
        Box::new(backend),
        t.dir.clone(),
        true,
        vec![(1_500, Command::Stop)],
    );
    assert_eq!(code, 0);
    assert_eq!(opened.load(std::sync::atomic::Ordering::Relaxed), 0);
    for s in &manifest(&t.dir).sources {
        assert_eq!(s.runs.len(), 1, "{}: only what was recorded", s.id);
    }
}
