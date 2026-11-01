//! A system alert on real threads with the fake backend: the recorder reads the backend's alerts from the start until
//! the gate, and a take whose start raised one opens its gate only a whole countdown after it went.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Src, cfg};
use klipolo_capture::protocol::Command;
use klipolo_capture::run::threads::run_with;
use klipolo_capture::session::{self, SessionDir};
use klipolo_capture::synthetic::{FakeBackend, Script, VideoFeed};
use klipolo_platform::clock::host_ns;

const S: u64 = 1_000_000_000;

#[test]
fn a_take_waits_for_the_system_s_alert_and_records_a_whole_countdown_after_it() {
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "take").unwrap();
    session::write_config(&dir, &cfg(&[Src::Screen, Src::Mic], 3)).unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let made = host_ns();
    // The take's first stream raises macOS's question as Record starts it; the person answers it 2 s later.
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        system_alerts: vec![(0, 2_000)],
        ..Script::default()
    });
    let reads = backend.alert_reads();
    let code = run_with(
        Box::new(backend),
        dir.clone(),
        false,
        vec![(0, Command::Start), (8_000, Command::Stop)],
    );
    assert_eq!(code, 0, "the take was recorded");
    let gate = common::harness::gate_host_ns(&dir).expect("the gate opened");
    // Without the alert the gate opens about 3 s after the start; with it, a whole countdown after its answer.
    assert!(
        gate >= made + 5 * S,
        "the gate opened {:.2} s after the start",
        (gate - made) as f64 / 1e9
    );
    let reads = reads.lock().unwrap().clone();
    assert!(
        reads.iter().any(|&at| at < made + 2 * S) && reads.iter().any(|&at| at >= made + 2 * S),
        "read while it was up and after it went: {reads:?}"
    );
    assert!(
        reads.iter().all(|&at| at <= gate + S / 2),
        "never read once the take records"
    );
}

#[test]
fn a_take_without_an_alert_starts_after_its_countdown() {
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "take").unwrap();
    session::write_config(&dir, &cfg(&[Src::Screen, Src::Mic], 2)).unwrap();
    let made = host_ns();
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4"),
            30,
        ),
        ..Script::default()
    });
    let reads = backend.alert_reads();
    let code = run_with(
        Box::new(backend),
        dir.clone(),
        false,
        vec![(0, Command::Start), (4_000, Command::Stop)],
    );
    assert_eq!(code, 0);
    let gate = common::harness::gate_host_ns(&dir).expect("the gate opened");
    assert!(gate < made + 3 * S, "{:.2} s", (gate - made) as f64 / 1e9);
    assert!(
        !reads.lock().unwrap().is_empty(),
        "the countdown asked the backend"
    );
}
