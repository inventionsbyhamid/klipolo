//! The recorder tests' world: the fake backend (no device, no prompt), a worker that is a pair of in-memory pipes the
//! test drives (`FakeLaunch`, in `test_launch`), a probe that writes a fixed report, a clock the test moves, a sink and
//! a window that record what the app would do, and a temporary folder for the takes and the recorder's files. No
//! process starts and nothing captures.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_capture::backend::{Backend, EncoderResult, ProbeReport};
use klipolo_capture::media::Codec;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::session::{SessionConfig, SessionDir, read_config};
use klipolo_capture::synthetic::{FakeBackend, Script};
use klipolo_model::coded::{CodedError, Said};
use klipolo_platform::clock::Clock;

use super::env::{Done, Sink};
use super::setup::Files;
use super::state::{Parts, RecorderState};
use super::take::Recorder;
use super::types::{ArmOptions, ArmedTake, WhatToRecord};
use super::window::Window;

pub(super) use super::test_launch::FakeLaunch;

/// A clock the test moves.
#[derive(Default)]
pub(super) struct FakeClock(AtomicU64);

impl Clock for FakeClock {
    fn now_ns(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// What reached the page and the import.
#[derive(Default)]
pub(super) struct FakeSink {
    /// Each event with its take's id.
    pub(super) events: Mutex<Vec<(String, Event)>>,
    pub(super) finished: Mutex<Vec<Done>>,
}

impl Sink for FakeSink {
    fn event(&self, take: &str, ev: &Event) {
        let mut events = self.events.lock().unwrap();
        events.push((take.to_owned(), ev.clone()));
    }
    fn finished(&self, done: &Done) {
        self.finished.lock().unwrap().push(done.clone());
    }
}

/// Klipolo's window, as the recorder left it.
pub(super) struct FakeWindow {
    pub(super) visible: AtomicBool,
    pub(super) hides: AtomicU64,
    pub(super) shows: AtomicU64,
}

impl Window for FakeWindow {
    fn hide(&self) {
        self.visible.store(false, Ordering::SeqCst);
        self.hides.fetch_add(1, Ordering::SeqCst);
    }
    fn show_and_focus(&self) {
        self.visible.store(true, Ordering::SeqCst);
        self.shows.fetch_add(1, Ordering::SeqCst);
    }
}

/// What the test watches besides the recorder: how often each seam was used.
#[derive(Clone)]
pub(super) struct World {
    pub(super) launch: FakeLaunch,
    pub(super) clock: Arc<FakeClock>,
    pub(super) sink: Arc<FakeSink>,
    pub(super) window: Arc<FakeWindow>,
    /// Backends made (the factory's calls).
    pub(super) backends: Arc<AtomicU64>,
    /// Screen Recording requests the fake backend got.
    pub(super) screen_requests: Arc<AtomicU64>,
    /// Encoder probes run.
    pub(super) probes: Arc<AtomicU64>,
    /// What the fake probe writes; None: nothing, as a probe that hung or crashed.
    pub(super) probe_writes: Arc<Mutex<Option<ProbeReport>>>,
    /// The open project's id; None: the start screen.
    pub(super) project: Arc<Mutex<Option<String>>>,
    pub(super) root: PathBuf,
    pub(super) files: Files,
}

/// The recorder's parts over the fake backend `script`, a fake launch and probe, in `dir`, and what the test watches.
pub(super) fn world(dir: &Path, script: Script) -> (Parts, World) {
    let backend = FakeBackend::new(script);
    let screen_requests = backend.screen_requests();
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let world = World {
        launch: FakeLaunch::default(),
        clock: Arc::new(FakeClock(AtomicU64::new(1_000_000_000_000))),
        sink: Arc::default(),
        window: Arc::new(FakeWindow {
            visible: AtomicBool::new(true),
            hides: AtomicU64::new(0),
            shows: AtomicU64::new(0),
        }),
        backends: Arc::default(),
        screen_requests,
        probes: Arc::default(),
        probe_writes: Arc::new(Mutex::new(Some(probe_report()))),
        project: Arc::new(Mutex::new(Some("project-1".into()))),
        root: dir.join("Recordings"),
        files: Files {
            dir: dir.join("recorder"),
        },
    };
    let (made, probes, project) = (
        world.backends.clone(),
        world.probes.clone(),
        world.project.clone(),
    );
    let writes = world.probe_writes.clone();
    let parts = Parts {
        backend: Box::new(move || {
            made.fetch_add(1, Ordering::SeqCst);
            backend.clone()
        }),
        launch: Arc::new(world.launch.clone()),
        // About as long as a real probe takes to start, so two setups at once overlap.
        probe: Box::new(move |out: &Path| {
            probes.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            if let Some(report) = writes.lock().unwrap().clone() {
                std::fs::write(out, serde_json::to_vec(&report).unwrap()).unwrap();
            }
        }),
        clock: world.clock.clone(),
        sink: world.sink.clone(),
        window: world.window.clone(),
        files: Some(world.files.clone()),
        root: Some(world.root.clone()),
        open_project: Box::new(move || project.lock().unwrap().clone()),
        ticker: false,
    };
    (parts, world)
}

/// This Mac's probe: H.264 on the Intel GPU only, HEVC refused.
pub(super) fn probe_report() -> ProbeReport {
    ProbeReport {
        encoders: vec![EncoderResult {
            id: "h264.gva".into(),
            codec: Codec::H264,
            hardware: true,
            dedicated: false,
            engine: "gva".into(),
            fps_1080p60: 60.0,
            fps_1440p60: 60.0,
            fps_1080p120: 0.0,
            max_fps_1440p: 67.0,
            latency_p95_ms: 116.0,
        }],
        took_ms: 2_195,
    }
}

/// A recorder over the fake world, in a temporary folder. Its methods first wait for the recorder to handle everything
/// the fake workers said (`settle`).
pub(super) struct Fixture {
    pub(super) state: RecorderState,
    pub(super) world: World,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for Fixture {
    type Target = World;
    fn deref(&self) -> &World {
        &self.world
    }
}

/// A recorder on the default fake computer: one display, a camera and a mic, everything allowed.
pub(super) fn fixture() -> Fixture {
    fixture_with(Script::default())
}

/// A recorder on the fake computer `script` describes.
pub(super) fn fixture_with(script: Script) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let (parts, world) = world(dir.path(), script);
    Fixture {
        state: RecorderState::build(false, || parts),
        world,
        _dir: dir,
    }
}

impl Fixture {
    pub(super) fn rec(&self) -> &Arc<Recorder> {
        self.state
            .recorder()
            .expect("a recorder outside a test run")
    }

    /// Waits (at most 5 s) until the recorder has handled every line and exit the fake workers sent.
    pub(super) fn settle(&self) {
        let started = Instant::now();
        let handled = &self.rec().handled;
        while handled.load(Ordering::SeqCst) < self.launch.0.sent.load(Ordering::SeqCst) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the recorder handled every line"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub(super) fn arm(&self, options: ArmOptions) -> Result<ArmedTake, CodedError> {
        self.settle();
        self.state.arm(&options)
    }

    /// `cmd` to the current take, by its id.
    pub(super) fn send(&self, cmd: Command) -> Result<(), CodedError> {
        self.settle();
        let take = self.rec().take_id().expect("a take");
        self.state.send(&take, cmd)
    }

    /// What reached the page.
    pub(super) fn events(&self) -> Vec<Event> {
        self.tagged_events().into_iter().map(|(_, ev)| ev).collect()
    }

    /// What reached the page, each with its take's id.
    pub(super) fn tagged_events(&self) -> Vec<(String, Event)> {
        self.settle();
        self.sink.events.lock().unwrap().clone()
    }

    /// The takes handed to the import.
    pub(super) fn finished(&self) -> Vec<Done> {
        self.settle();
        self.sink.finished.lock().unwrap().clone()
    }

    pub(super) fn visible(&self) -> bool {
        self.settle();
        self.window.visible.load(Ordering::SeqCst)
    }

    /// Take `id`'s folder, as the worker's argument.
    pub(super) fn take_dir(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.klipolorec"))
    }

    /// Take `id`'s `session.json`.
    pub(super) fn config(&self, id: &str) -> SessionConfig {
        read_config(&SessionDir::at(&self.take_dir(id))).unwrap()
    }

    /// The clock moves `s` seconds, ticking the supervisor every half second as the app's ticker does.
    pub(super) fn advance_s(&self, s: u64) {
        self.settle();
        for _ in 0..s * 2 {
            self.clock.0.fetch_add(500_000_000, Ordering::SeqCst);
            self.rec().tick();
            self.settle();
        }
    }
}

/// The main display with the built-in mic, no camera.
pub(super) fn options_screen() -> ArmOptions {
    ArmOptions {
        what: WhatToRecord::Screen {
            display: "display-1".into(),
        },
        mic: Some("mic-1".into()),
        camera: None,
        camera_bubble: false,
        mirror_camera: true,
    }
}

/// A state change from the worker.
pub(super) fn state(state: klipolo_capture::shell::WorkerState) -> Event {
    Event::State {
        state,
        elapsed_us: 0,
    }
}

/// The code a refusal carries.
pub(super) fn code_of(said: &Said) -> &str {
    match said {
        Said::Coded(e) => &e.code,
        Said::Text(text) => panic!("a coded refusal, not {text:?}"),
    }
}
