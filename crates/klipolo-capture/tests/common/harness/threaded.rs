//! The harness's helpers for the threaded and kill tests: a take's folder, the fake backend with the fixture's real
//! H.264, a command script, the gate's host time, and a resumed take's starting point.

use std::collections::BTreeMap;
use std::path::Path;

use klipolo_capture::backend::Backend;
use klipolo_capture::clock::SessionClock;
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::protocol::Command;
use klipolo_capture::run::resume::Resumed;
use klipolo_capture::session::{self, SessionDir};
use klipolo_capture::synthetic::{FakeBackend, FixtureVideo, Script, VideoFeed};

use super::{Src, cfg};

fn fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4")
}

pub fn fixture() -> FixtureVideo {
    FixtureVideo::load(&fixture_path())
}

/// `<root>/take.klipolorec` with these sources and no countdown, its config written.
pub fn session(root: &Path, srcs: &[Src]) -> SessionDir {
    let dir = SessionDir::create(root, "take").unwrap();
    session::write_config(&dir, &cfg(srcs, 0)).unwrap();
    dir
}

/// The fake backend, its video the fixture's real H.264 at 30 fps.
pub fn fake_backend() -> Box<dyn Backend> {
    Box::new(FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture_path(), 30),
        ..Script::default()
    }))
}

pub fn script_start_then_stop_after_ms(ms: u32) -> Vec<(u32, Command)> {
    vec![(0, Command::Start), (ms, Command::Stop)]
}

/// The host time the take's gate opened at (its journal's `Gate`).
pub fn gate_host_ns(dir: &SessionDir) -> Option<u64> {
    journal::read(&dir.journal())
        .ok()?
        .entries
        .iter()
        .find_map(|e| match e {
            Entry::Gate { host_ns } => Some(*host_ns),
            _ => None,
        })
}

/// A crashed take's journal as resume reads it: the gate at host `gate_s`, these pauses (host seconds), these runs
/// (each source's highest), and where each source's media ended (session seconds).
pub fn resumed_from(
    runs: &[(&str, u32)],
    gate_s: f64,
    pauses: &[(f64, f64)],
    ended: &[(&str, f64)],
) -> Resumed {
    let ns = |s: f64| (s * 1e9) as u64;
    let mut clock = SessionClock::new();
    clock.start(ns(gate_s));
    for &(from, to) in pauses {
        clock.pause(ns(from));
        clock.resume(ns(to));
    }
    Resumed {
        clock,
        next_run: runs.iter().map(|(s, n)| (s.to_string(), n + 1)).collect(),
        ended_at: ended
            .iter()
            .map(|(s, at)| (s.to_string(), (at * 1e6) as i64))
            .collect(),
        done: BTreeMap::new(),
        stopped: false,
        markers: 0,
        cursor: Default::default(),
    }
}
