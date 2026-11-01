//! A screen that fails before its stream runs fails the sound that shares it at once, and the take
//! with it (no take without its main picture). The fake backend's screen and that screen's own
//! sound are joined by the pure `SharedStream`, as macOS's are on one ScreenCaptureKit stream: on a first game take
//! before Screen Recording is granted, the app hears `capture.permission.screen` within a moment and the worker ends
//! with no take, instead of sitting in "Starting" for 30 s or recording the mic alone.
#![cfg(feature = "synthetic")]

mod common;

use std::io::Write;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use common::harness::Src;
use klipolo_capture::backend::{
    AudioSpec, Backend, CaptureCaps, Devices, Permissions, PreviewPipe, ProbeReport, VideoPipes,
    VideoSpec,
};
use klipolo_capture::media::{AudioFormat, StreamInfo};
use klipolo_capture::pipe::{
    AudioOut, AudioPipe, AudioStats, PipeError, VideoOut, VideoPipe, VideoStats,
};
use klipolo_capture::platform::common::shared_stream::SharedStream;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::threads::run_session;
use klipolo_capture::shell::{Shell, WorkerState};
use klipolo_capture::synthetic::{FakeBackend, Script};

#[test]
fn a_screen_without_permission_ends_the_take_at_once_with_its_waiting_sound() {
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::GameSound, Src::Mic]);
    let mut script = Script::default();
    (script.start_errors).insert("screen".into(), PipeError::PermissionDenied);
    let backend = Joined(FakeBackend::new(script));
    // The app's side: its commands into the worker, the worker's events back.
    let (input, mut commands) = std::io::pipe().unwrap();
    let (events_tx, events) = mpsc::channel();
    let worker_dir = dir.clone();
    let worker = std::thread::spawn(move || {
        let input = Box::new(std::io::BufReader::new(input));
        run_session(
            Box::new(backend),
            worker_dir,
            false,
            input,
            Box::new(Lines(events_tx, vec![])),
        )
    });
    let mut line = serde_json::to_vec(&Command::Start).unwrap();
    line.push(b'\n');
    commands.write_all(&line).unwrap();
    let start = Instant::now();
    let (mut failed, mut asked, mut recorded) = (None, false, false);
    // Within 10 s: a take that went on would send events until its Stop.
    while let Some(left) = Duration::from_secs(10).checked_sub(start.elapsed()) {
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        match event {
            Event::Failed { error } => {
                failed = Some((error.code, start.elapsed()));
                break;
            }
            Event::Gate { .. } => asked = true,
            Event::State {
                state: WorkerState::Recording,
                ..
            } => recorded = true,
            _ => {}
        }
    }
    // A take that went on is stopped, so the test ends either way.
    let mut line = serde_json::to_vec(&Command::Stop).unwrap();
    line.push(b'\n');
    let _ = commands.write_all(&line);
    drop(commands);
    let code = worker.join().unwrap();
    let (error, at) = failed.expect("the app hears why");
    assert_eq!(error, "capture.permission.screen");
    assert!(at < Duration::from_secs(1), "said at once: {at:?}");
    assert!(!asked && !recorded, "no question, nothing recorded");
    assert_eq!(code, 3, "no take");
    assert!(!dir.root.exists(), "the empty take's folder went");
}

/// The worker's lines to the app, as events on a channel.
struct Lines(mpsc::Sender<Event>, Vec<u8>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.1.extend_from_slice(buf);
        while let Some(at) = self.1.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.1.drain(..=at).collect();
            if let Ok(event) = serde_json::from_slice::<Event>(&line) {
                let _ = self.0.send(event);
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The fake backend, its screen's sound waiting on the screen's start as macOS's does.
struct Joined(FakeBackend);

/// The screen's pipe, reporting every outcome of its start.
struct Reporting {
    pipe: Box<dyn VideoPipe>,
    stream: Arc<SharedStream>,
}

/// The screen's own sound, starting once the screen's stream runs.
struct Waiting {
    pipe: Box<dyn AudioPipe>,
    stream: Arc<SharedStream>,
}

impl VideoPipe for Reporting {
    fn start(&mut self, out: VideoOut) -> Result<StreamInfo, PipeError> {
        self.stream.idle();
        let started = self.pipe.start(out);
        match &started {
            Ok(_) => self.stream.running(),
            Err(e) => self.stream.failed(e.clone()),
        }
        started
    }
    fn set_paused(&mut self, paused: bool) {
        self.pipe.set_paused(paused);
    }
    fn request_keyframe(&mut self) {
        self.pipe.request_keyframe();
    }
    fn lower_preset(&mut self) -> bool {
        self.pipe.lower_preset()
    }
    fn stop(&mut self) {
        self.pipe.stop();
        self.stream.idle();
    }
    fn stats(&self) -> VideoStats {
        self.pipe.stats()
    }
}

impl AudioPipe for Waiting {
    fn start(&mut self, out: AudioOut) -> Result<AudioFormat, PipeError> {
        self.stream.wait(&out.stopping, Duration::from_secs(120))?;
        self.pipe.start(out)
    }
    fn stop(&mut self) {
        self.pipe.stop();
    }
    fn stats(&self) -> AudioStats {
        self.pipe.stats()
    }
}

impl Backend for Joined {
    fn caps(&self) -> CaptureCaps {
        self.0.caps()
    }
    fn devices(&self) -> Devices {
        self.0.devices()
    }
    fn permissions(&self) -> Permissions {
        self.0.permissions()
    }
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        let pipes = self.0.video(spec)?;
        let stream = Arc::new(SharedStream::default());
        let sound = (pipes.sound).map(|pipe| {
            let stream = stream.clone();
            Box::new(Waiting { pipe, stream }) as Box<dyn AudioPipe>
        });
        Ok(VideoPipes {
            video: Box::new(Reporting {
                pipe: pipes.video,
                stream,
            }),
            sound,
        })
    }
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        self.0.audio(spec)
    }
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.0.preview(camera)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.0.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        self.0.shell()
    }
}
