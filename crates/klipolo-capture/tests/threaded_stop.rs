//! Stop and capture consent on real threads with the fake backend: once the person's Stop is read,
//! nothing opens, whether a resumed worker is preparing, the take's pipes are being built, or a heal's restart is
//! queued behind a Stop from the menu.
#![cfg(feature = "synthetic")]

mod common;

use std::io::{BufReader, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use common::harness::Src;
use klipolo_capture::backend::{
    AudioSpec, Backend, CaptureCaps, Devices, Permissions, PreviewPipe, ProbeReport, VideoPipes,
    VideoSpec,
};
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest::Manifest;
use klipolo_capture::media::AudioFormat;
use klipolo_capture::pipe::{AudioOut, AudioPipe, AudioStats, PipeError, PipeEvent};
use klipolo_capture::protocol::Command;
use klipolo_capture::run::threads::{run_session, run_with};
use klipolo_capture::shell::{MenuAction, Shell, ShellInput};
use klipolo_capture::synthetic::{FakeBackend, FakeShell, Script, VideoFeed};

fn manifest(dir: &klipolo_capture::session::SessionDir) -> Manifest {
    serde_json::from_slice(&std::fs::read(dir.manifest()).unwrap()).unwrap()
}

#[test]
fn a_resumed_worker_never_records_after_the_person_s_stop() {
    // The person pressed Stop and the worker died in Stop's finishing: the journal's last line is `StopRequested`. A
    // resume opens no screen, camera or mic; it finishes the take (capture consent).
    let root = tempfile::tempdir().unwrap();
    let crashed = common::take::Take {
        finalize: false,
        cut_last_fragment: true,
        ..common::take::Take::default()
    };
    let t = common::take::write_take(root.path(), "take", crashed);
    let stop = Entry::StopRequested {
        at_us: 2_000_000,
        reason: None,
    };
    journal::open_append(&t.dir.journal())
        .unwrap()
        .write_all(&journal::line(&stop))
        .unwrap();
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
    assert_eq!(code, 0, "the take is finished");
    assert_eq!(
        opened.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "no device opened"
    );
    let entries = journal::read(&t.dir.journal()).unwrap().entries;
    let stopped = (entries.iter())
        .position(|e| matches!(e, Entry::StopRequested { .. }))
        .unwrap();
    assert!(
        !entries[stopped..]
            .iter()
            .any(|e| matches!(e, Entry::RunStarted { .. }))
    );
    assert!(entries.iter().any(|e| matches!(e, Entry::Finalized { .. })));
    let m = manifest(&t.dir);
    assert_eq!(m.sources.len(), 2);
    for s in &m.sources {
        assert_eq!(s.runs.len(), 1, "{}: only what was recorded", s.id);
    }
}

#[test]
fn a_stop_sent_while_a_resumed_worker_prepares_opens_nothing() {
    // The worker crashed and the person pressed Stop: the app's Stop is waiting on the resuming worker's stdin before
    // it has prepared. Nothing opens, and the take is finished (capture consent).
    let root = tempfile::tempdir().unwrap();
    let crashed = common::take::Take {
        finalize: false,
        cut_last_fragment: true,
        ..common::take::Take::default()
    };
    let t = common::take::write_take(root.path(), "take", crashed);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        ..Script::default()
    });
    let opened = backend.opened();
    let mut stop = serde_json::to_vec(&Command::Stop).unwrap();
    stop.push(b'\n');
    let code = run_session(
        Box::new(backend),
        t.dir.clone(),
        true,
        Box::new(std::io::Cursor::new(stop)),
        Box::new(std::io::sink()),
    );
    assert_eq!(code, 0, "the take is finished");
    assert_eq!(
        opened.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "no device opened"
    );
    let entries = journal::read(&t.dir.journal()).unwrap().entries;
    let stopped = (entries.iter()).any(|e| matches!(e, Entry::StopRequested { .. }));
    let finalized = (entries.iter()).any(|e| matches!(e, Entry::Finalized { .. }));
    assert!(stopped && finalized);
    for s in &manifest(&t.dir).sources {
        assert_eq!(s.runs.len(), 1, "{}: only what was recorded", s.id);
    }
}

#[test]
fn a_stop_read_while_the_pipes_are_built_opens_nothing() {
    // The narrowest moment of the N1 window: the person's Stop reaches the resuming worker after its prepare read the
    // queue, while the take's pipes are being built. No pipe starts (capture consent).
    let root = tempfile::tempdir().unwrap();
    let crashed = common::take::Take {
        finalize: false,
        ..common::take::Take::default()
    };
    let t = common::take::write_take(root.path(), "take", crashed);
    let (reader, writer) = std::io::pipe().unwrap();
    let writer = std::sync::Mutex::new(Some(writer));
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        ..Script::default()
    })
    .on_first_build(move || {
        let mut stop = serde_json::to_vec(&Command::Stop).unwrap();
        stop.push(b'\n');
        if let Some(mut w) = writer.lock().unwrap().take() {
            w.write_all(&stop).unwrap();
        }
        // The command reader takes it well before the pipes are told to start.
        std::thread::sleep(Duration::from_millis(200));
    });
    let starts = backend.starts();
    let code = run_session(
        Box::new(backend),
        t.dir.clone(),
        true,
        Box::new(BufReader::new(reader)),
        Box::new(std::io::sink()),
    );
    assert_eq!(code, 0, "the take is finished");
    assert_eq!(
        starts.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "no pipe started"
    );
    let entries = journal::read(&t.dir.journal()).unwrap().entries;
    let stopped = (entries.iter()).any(|e| matches!(e, Entry::StopRequested { .. }));
    let finalized = (entries.iter()).any(|e| matches!(e, Entry::Finalized { .. }));
    assert!(stopped && finalized);
    for s in &manifest(&t.dir).sources {
        assert_eq!(s.runs.len(), 1, "{}: only what was recorded", s.id);
    }
}

#[test]
fn a_restart_queued_behind_a_stop_from_the_menu_opens_nothing() {
    // The mic blips at 1 s and is back at 1.1 s: the heal restarts it, and its controller first closes the old stream,
    // which takes 600 ms here (a slow device teardown). The menu's Stop and Save comes at 1.4 s, meanwhile. The
    // restart queued ahead of that Stop must not open the mic again (the Stops the command
    // reader never sees set the flag too).
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let script = Script {
        video: VideoFeed::Fixture(fixture, 30),
        events: vec![
            (1_000, "mic".into(), PipeEvent::DeviceLost),
            (1_100, "mic".into(), PipeEvent::DeviceBack),
        ],
        ..Script::default()
    };
    let (shell, _outputs) = FakeShell::new(vec![(1_400, ShellInput::Menu(MenuAction::Stop))]);
    let backend = SlowMicClose {
        inner: FakeBackend::new(script).with_shell(shell),
        mic_starts: Arc::default(),
    };
    let mic_starts = backend.mic_starts.clone();
    let code = run_with(
        Box::new(backend),
        dir.clone(),
        false,
        vec![(0, Command::Start)],
    );
    assert_eq!(code, 0, "the take is saved");
    // The run controller may still be closing the old stream: give it time to open the mic, if it would.
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        mic_starts.load(Ordering::SeqCst),
        1,
        "the mic opened once, at Record"
    );
}

/// The fake backend, its mic slow to close (600 ms) and its mic starts counted.
struct SlowMicClose {
    inner: FakeBackend,
    mic_starts: Arc<AtomicU64>,
}

struct SlowClose {
    pipe: Box<dyn AudioPipe>,
    starts: Arc<AtomicU64>,
}

impl AudioPipe for SlowClose {
    fn start(&mut self, out: AudioOut) -> Result<AudioFormat, PipeError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.pipe.start(out)
    }
    fn stop(&mut self) {
        self.pipe.stop();
        std::thread::sleep(Duration::from_millis(600));
    }
    fn stats(&self) -> AudioStats {
        self.pipe.stats()
    }
}

impl Backend for SlowMicClose {
    fn caps(&self) -> CaptureCaps {
        self.inner.caps()
    }
    fn devices(&self) -> Devices {
        self.inner.devices()
    }
    fn permissions(&self) -> Permissions {
        self.inner.permissions()
    }
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        self.inner.video(spec)
    }
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        let pipe = self.inner.audio(spec)?;
        Ok(Box::new(SlowClose {
            pipe,
            starts: self.mic_starts.clone(),
        }))
    }
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.inner.preview(camera)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.inner.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        self.inner.shell()
    }
}
