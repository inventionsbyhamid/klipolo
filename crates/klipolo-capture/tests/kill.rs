//! Worker `kill -9`: the synthetic take in its own process (`synthetic-take`), killed at
//! seeded moments. Every source keeps all but ≤ 0.7 s of video and ≤ 0.2 s of audio, and recovers; the cursor file
//! keeps all but ≤ 1.2 s; killed then resumed, the take goes on as run 2 of each source with a capture gap of
//! at most 3 s.
#![cfg(feature = "synthetic")]

mod common;

/// Worker `kill -9`: every source keeps all but ≤ 0.7 s of video and ≤ 0.2 s of audio, and recovers.
fn sweep(runs: u64) {
    let mut worst = std::collections::BTreeMap::<String, i64>::new();
    for seed in 0..runs {
        let root = tempfile::tempdir().unwrap();
        let after_ms = 1_500 + (seed * 7_919) % 2_500;
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_synthetic-take"))
            .arg(root.path())
            .env("KLIPOLO_TEST", "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(after_ms));
        let killed_ns = klipolo_platform::clock::host_ns();
        child.kill().unwrap(); // SIGKILL on Unix
        child.wait().unwrap();
        let dir = klipolo_capture::session::SessionDir::at(&root.path().join("take.klipolorec"));
        let gate_ns = common::harness::gate_host_ns(&dir).expect("the gate opened");
        let wall_us = ((killed_ns - gate_ns) / 1_000) as i64;
        let m = klipolo_capture::recover::repair(&dir).unwrap().manifest;
        assert_eq!(m.sources.len(), 2, "seed {seed}: both sources recovered");
        for s in &m.sources {
            let got: i64 = s.runs.iter().map(|r| r.duration_us).sum();
            let allowed = if s.id == "screen" { 700_000 } else { 200_000 };
            let lost = worst.entry(s.id.clone()).or_insert(i64::MIN);
            *lost = (*lost).max(wall_us - got);
            assert!(
                got >= wall_us - allowed,
                "seed {seed}: {} kept {got} of {wall_us} µs",
                s.id
            );
        }
    }
    println!("the most each source lost over {runs} kills, µs: {worst:?}");
}

#[test]
fn kill_sweep_loses_at_most_700_ms() {
    sweep(20)
}

/// Worker `kill -9`: the cursor file's durable length (a power cut's bound) covers the take
/// but its last 1.2 s at most (the 50 ms reorder window, the 100 ms gather and the 1 s tick); recovered after the crash,
/// it covers all but 0.3 s (the window and the last gather).
#[test]
fn kill_sweep_keeps_the_cursor_within_1_2_s() {
    let (mut worst, mut worst_crash) = (i64::MIN, i64::MIN);
    for seed in 0..8 {
        let root = tempfile::tempdir().unwrap();
        let after_ms = 1_700 + (seed * 3_301) % 2_300;
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_synthetic-take"))
            .arg(root.path())
            .env("KLIPOLO_TEST", "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(after_ms));
        let killed_ns = klipolo_platform::clock::host_ns();
        child.kill().unwrap();
        child.wait().unwrap();
        let dir = klipolo_capture::session::SessionDir::at(&root.path().join("take.klipolorec"));
        let gate_ns = common::harness::gate_host_ns(&dir).expect("the gate opened");
        let wall_us = ((killed_ns - gate_ns) / 1_000) as i64;
        let durable = klipolo_capture::journal::read(&dir.journal())
            .unwrap()
            .durable_through()
            .get(klipolo_capture::session::CURSOR)
            .copied()
            .unwrap_or(0);
        let bytes = std::fs::read(dir.cursor()).unwrap();
        let kept = &bytes[..(durable as usize).min(bytes.len())];
        let track =
            klipolo_cursor::CursorTrack::read(kept, klipolo_cursor::Limits::for_file_len(durable));
        let last = track.span().map_or(0, |s| s.1);
        worst = worst.max(wall_us - last);
        assert!(
            last >= wall_us - 1_200_000,
            "seed {seed}: the durable cursor ends at {last} of {wall_us} µs"
        );
        // What a worker crash really costs: recovery keeps every whole line past the durable length, so only the
        // window and the last gather are lost.
        klipolo_capture::recover::repair(&dir).unwrap();
        let bytes = std::fs::read(dir.cursor()).unwrap();
        let limits = klipolo_cursor::Limits::for_file_len(bytes.len() as u64);
        let kept = klipolo_cursor::CursorTrack::read(&bytes[..], limits);
        let last = kept.span().map_or(0, |s| s.1);
        worst_crash = worst_crash.max(wall_us - last);
        assert!(
            last >= wall_us - 300_000,
            "seed {seed}: the recovered cursor ends at {last} of {wall_us} µs"
        );
    }
    println!("the most cursor lost over the kills, µs: durable {worst}, recovered {worst_crash}");
}

/// Kill, then resume at once: the take continues as run 2 of each source, with a capture gap of at most 3 s.
#[test]
fn kill_then_resume_leaves_a_gap_of_at_most_3_s() {
    let root = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_synthetic-take"))
        .arg(root.path())
        .env("KLIPOLO_TEST", "1")
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2_500));
    child.kill().unwrap();
    child.wait().unwrap();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_synthetic-take"))
        .args([
            root.path().to_str().unwrap(),
            "--resume",
            "--stop-after-ms",
            "2000",
        ])
        .env("KLIPOLO_TEST", "1")
        .status()
        .unwrap();
    assert!(status.success());
    let dir = klipolo_capture::session::SessionDir::at(&root.path().join("take.klipolorec"));
    let m: klipolo_capture::manifest::Manifest =
        serde_json::from_slice(&std::fs::read(dir.manifest()).unwrap()).unwrap();
    for s in &m.sources {
        assert_eq!(s.runs.len(), 2, "{}", s.id);
        let gap = s.runs[1].offset_us - (s.runs[0].offset_us + s.runs[0].duration_us);
        assert!((0..=3_000_000).contains(&gap), "{}: gap {gap} µs", s.id);
    }
}

/// A drive that stops answering at Stop: the finishing gives up after 10 s without progress, the take
/// ends with `capture.error.driveStuck` and exit 1, and recovery finishes it from what is durable.
#[test]
fn a_drive_that_stops_answering_ends_the_take_for_recovery() {
    let root = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_synthetic-take"))
        .args([root.path().to_str().unwrap(), "--stop-after-ms", "2500"])
        .env("KLIPOLO_TEST", "1")
        // Every flush tick's full flush hangs: the drive stops answering from the first tick on.
        .env("KLIPOLO_FAIL", "writer.beforeBarrier=sleep:30000")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "the take couldn't be finished");
    let took = started.elapsed().as_secs_f64();
    assert!(
        (10.0..20.0).contains(&took),
        "gave up after 10 s without progress: {took:.1} s"
    );
    let dir = klipolo_capture::session::SessionDir::at(&root.path().join("take.klipolorec"));
    let entries = klipolo_capture::journal::read(&dir.journal())
        .unwrap()
        .entries;
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, klipolo_capture::journal::Entry::Finalized { .. }))
    );
    // The Stop line reached the journal all the same, so a resume never records again.
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, klipolo_capture::journal::Entry::StopRequested { .. }))
    );
    let m = klipolo_capture::recover::repair(&dir).unwrap().manifest;
    assert_eq!(m.sources.len(), 2, "recovery finishes the take");
}

#[test]
#[ignore]
fn kill_sweep_200_runs() {
    sweep(200)
}
