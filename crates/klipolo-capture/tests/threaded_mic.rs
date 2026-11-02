//! The mic's heals on real threads with the fake backend: the person's own mic, back after the
//! fallback, takes over again and never churns runs when it flaps; a blip restarts the same mic, and with nothing to
//! fall back to every return does; a restart that failed keeps watching.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::Src;
use klipolo_capture::heal::MIC_FALLBACK_NS;
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest::Manifest;
use klipolo_capture::run::threads::run_with;
use klipolo_capture::synthetic::{FakeBackend, Script, VideoFeed};

fn manifest(dir: &klipolo_capture::session::SessionDir) -> Manifest {
    serde_json::from_slice(&std::fs::read(dir.manifest()).unwrap()).unwrap()
}

#[test]
fn the_person_s_own_mic_coming_back_takes_over_from_the_fallback() {
    // The USB mic is unplugged for 5 s: the built-in one takes over within the heal budget, and once the USB mic is back
    // the take records on it again, at a new run (a gamer's USB mic beats the built-in one).
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let mut script = Script {
        video: VideoFeed::Fixture(fixture, 30),
        unplugged: vec![("mic-usb".into(), 1_500, 6_500)],
        ..Script::default()
    };
    script.devices.mics = common::harness::mics();
    let code = run_with(
        Box::new(FakeBackend::new(script)),
        dir.clone(),
        false,
        // Back at 6.5 s, and listed for 3 s before the switch (no churning runs).
        common::harness::script_start_then_stop_after_ms(12_000),
    );
    assert_eq!(code, 0);
    let m = manifest(&dir);
    let runs = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs;
    assert_eq!(runs.len(), 3, "own, fallback, own: {runs:?}");
    // Each switch leaves only the swap between the runs (a device's start, here a few ms) beyond the heal budget.
    let swap_us = 500_000;
    let gaps: Vec<i64> = (runs.windows(2))
        .map(|w| w[1].offset_us - (w[0].offset_us + w[0].duration_us))
        .collect();
    println!("the gaps between the mic's runs: {gaps:?} µs");
    assert!(
        gaps[0] <= (MIC_FALLBACK_NS / 1_000) as i64 + swap_us,
        "{gaps:?}"
    );
    assert!(gaps[1] <= swap_us, "nothing lost but the swap: {gaps:?}");
    for run in runs {
        assert!(
            run.channel_peaks.as_ref().is_some_and(|p| p[0] > 0.05),
            "sound in every run: {run:?}"
        );
    }
    // The fallback's run ended and was durable before the person's own mic opened the next.
    let entries = journal::read(&dir.journal()).unwrap().entries;
    let at = |want: &dyn Fn(&Entry) -> bool| entries.iter().position(want).unwrap();
    let ended = at(&|e| matches!(e, Entry::RunEnded { source, run: 2, .. } if source == "mic"));
    let next = at(&|e| matches!(e, Entry::RunStarted { source, run: 3, .. } if source == "mic"));
    let durable = |e: &Entry| match e {
        Entry::Durable { through } => through.get(&runs[1].file).copied(),
        _ => None,
    };
    let last = entries.iter().rev().find_map(durable);
    assert!(ended < next, "the fallback's run ends first");
    assert_eq!(
        entries[ended..next].iter().find_map(durable),
        last,
        "its last length is durable before the own mic's run opens"
    );
    // The person is told both ways (the manifest's notes keep both).
    let said = |code: &str| {
        entries.iter().any(|e| {
            matches!(e, Entry::Health { code: c, params, .. }
                if c == code && params.get("device") == Some(&"MV7".into()))
        })
    };
    assert!(said("capture.health.micFallback") && said("capture.health.micBack"));
}

/// A health note's count in the take's journal.
fn journaled(dir: &klipolo_capture::session::SessionDir, code: &str) -> usize {
    (journal::read(&dir.journal()).unwrap().entries.iter())
        .filter(|e| matches!(e, Entry::Health { code: c, .. } if c == code))
        .count()
}

#[test]
fn a_flapping_usb_mic_never_churns_runs() {
    // The USB mic goes at 1 s; the built-in one records from 4 s. It comes back at 5 s, flaps out again at 5.5 s and
    // settles at 6.6 s: the take switches back once it has been listed for 3 s (about 10 s). It goes again at 11 s for
    // 4 s, and the built-in one takes over at 14 s; back at 15 s, but within 30 s of the last switch back, so the take
    // stays on the built-in mic. One mild note says why.
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let gone = |from, to| ("mic-usb".to_string(), from, to);
    let mut script = Script {
        video: VideoFeed::Fixture(fixture, 30),
        unplugged: vec![gone(1_000, 5_000), gone(5_500, 6_600), gone(11_000, 15_000)],
        ..Script::default()
    };
    script.devices.mics = common::harness::mics();
    let code = run_with(
        Box::new(FakeBackend::new(script)),
        dir.clone(),
        false,
        common::harness::script_start_then_stop_after_ms(20_000),
    );
    assert_eq!(code, 0);
    let m = manifest(&dir);
    let runs = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs;
    assert_eq!(runs.len(), 4, "own, fallback, own, fallback: {runs:?}");
    assert_eq!(
        journaled(&dir, "capture.health.micBack"),
        1,
        "one switch back"
    );
    assert_eq!(
        journaled(&dir, "capture.note.micFlapping"),
        1,
        "one mild note"
    );
}

#[test]
fn a_restart_that_fails_after_device_back_keeps_watching() {
    // A desktop with only the USB mic. It goes at 1 s and is listed again at 2 s a moment before it can stream (the
    // early `DeviceBack`), so the restart fails with NoDevice. The run controller keeps looking, and once the mic has
    // been listed for 3 s (from 4 s) the voice records again.
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let mut script = Script {
        video: VideoFeed::Fixture(fixture, 30),
        unplugged: vec![("mic-usb".into(), 1_000, 4_000)],
        events: vec![(
            2_000,
            "mic".into(),
            klipolo_capture::pipe::PipeEvent::DeviceBack,
        )],
        ..Script::default()
    };
    script.devices.mics = common::harness::mics()
        .into_iter()
        .filter(|m| m.id == "mic-usb")
        .collect();
    let code = run_with(
        Box::new(FakeBackend::new(script)),
        dir.clone(),
        false,
        common::harness::script_start_then_stop_after_ms(10_000),
    );
    assert_eq!(code, 0);
    let m = manifest(&dir);
    let runs = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs;
    assert_eq!(runs.len(), 2, "the voice is back: {runs:?}");
}

#[test]
fn a_one_second_blip_restarts_the_same_mic_with_no_fallback() {
    // The USB mic drops for a second, within the heal window: the take records on it again at once, as a new run, and
    // never moves to the built-in mic (the 3 s presence wait is only for switching back).
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let mut script = Script {
        video: VideoFeed::Fixture(fixture, 30),
        unplugged: vec![("mic-usb".into(), 1_000, 2_000)],
        ..Script::default()
    };
    script.devices.mics = common::harness::mics();
    let code = run_with(
        Box::new(FakeBackend::new(script)),
        dir.clone(),
        false,
        common::harness::script_start_then_stop_after_ms(6_000),
    );
    assert_eq!(code, 0);
    let runs = &manifest(&dir)
        .sources
        .iter()
        .find(|s| s.id == "mic")
        .unwrap()
        .runs
        .clone();
    let told: Vec<String> = (journal::read(&dir.journal()).unwrap().entries.iter())
        .filter_map(|e| match e {
            Entry::Health { code, .. } => Some(code.clone()),
            Entry::RunStarted {
                source, offset_us, ..
            } => Some(format!("{source} run at {offset_us}")),
            Entry::Gate { host_ns } => Some(format!("gate {host_ns}")),
            _ => None,
        })
        .collect();
    assert_eq!(runs.len(), 2, "the same mic, twice: {runs:?} {told:?}");
    assert_eq!(
        journaled(&dir, "capture.health.micFallback"),
        0,
        "no fallback: {told:?}"
    );
}

#[test]
fn a_restart_that_fails_with_an_os_error_keeps_watching() {
    // As above, but the early restart fails with an OS error (cpal's mapping for most backend failures): the mic is
    // never given up, and the voice comes back.
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let mut script = Script {
        video: VideoFeed::Fixture(fixture, 30),
        unplugged: vec![("mic-usb".into(), 1_000, 4_000)],
        unplugged_error: klipolo_capture::pipe::PipeError::Os("the stream couldn't start".into()),
        events: vec![(
            2_000,
            "mic".into(),
            klipolo_capture::pipe::PipeEvent::DeviceBack,
        )],
        ..Script::default()
    };
    script.devices.mics = common::harness::mics()
        .into_iter()
        .filter(|m| m.id == "mic-usb")
        .collect();
    let code = run_with(
        Box::new(FakeBackend::new(script)),
        dir.clone(),
        false,
        common::harness::script_start_then_stop_after_ms(10_000),
    );
    assert_eq!(code, 0);
    let runs = &manifest(&dir)
        .sources
        .iter()
        .find(|s| s.id == "mic")
        .unwrap()
        .runs
        .clone();
    assert_eq!(runs.len(), 2, "the voice is back: {runs:?}");
}

#[test]
fn with_no_fallback_a_mic_that_keeps_dropping_records_between_every_drop() {
    // A desktop with only the USB mic, whose cable drops for 1 s every 4 s: never listed 3 s straight between drops, so
    // a hold waiting for that recorded nothing after the third restart. With nothing to fall back to it is never held:
    // each return restarts it, and it records between every drop, with one mild note.
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let mut script = Script {
        video: VideoFeed::Fixture(fixture, 30),
        unplugged: (0..5)
            .map(|k| ("mic-usb".to_string(), 2_000 + 4_000 * k, 3_000 + 4_000 * k))
            .collect(),
        ..Script::default()
    };
    script.devices.mics = common::harness::mics()
        .into_iter()
        .filter(|m| m.id == "mic-usb")
        .collect();
    let code = run_with(
        Box::new(FakeBackend::new(script)),
        dir.clone(),
        false,
        common::harness::script_start_then_stop_after_ms(21_000),
    );
    assert_eq!(code, 0);
    let runs = manifest(&dir)
        .sources
        .into_iter()
        .find(|s| s.id == "mic")
        .unwrap()
        .runs;
    assert_eq!(
        runs.len(),
        6,
        "the first, then one after each drop: {runs:?}"
    );
    let gaps: Vec<i64> = (runs.windows(2))
        .map(|w| w[1].offset_us - (w[0].offset_us + w[0].duration_us))
        .collect();
    assert!(
        gaps.iter().all(|&g| g < 1_500_000),
        "only the drops are lost: {gaps:?} µs"
    );
    assert_eq!(
        journaled(&dir, "capture.note.micDropping"),
        1,
        "one mild note"
    );
    assert_eq!(journaled(&dir, "capture.note.micFlapping"), 0, "never held");
}
