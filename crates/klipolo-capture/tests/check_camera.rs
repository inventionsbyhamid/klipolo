//! Check camera with the fake backend: a Cancel the app sends right after the press opens nothing,
//! whether it lands while the preview is built or while the camera's start waits on the system (macOS's prompt); and
//! every frame the preview pipe paces reaches the app.
#![cfg(feature = "synthetic")]

#[path = "common/preview_io.rs"]
#[allow(dead_code, reason = "Check camera reads the worker's text")]
mod preview_io;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use klipolo_capture::backend::{
    AudioSpec, Backend, CaptureCaps, Devices, Permissions, PreviewFrame, PreviewPipe, ProbeReport,
    VideoPipes, VideoSpec,
};
use klipolo_capture::pipe::{AudioPipe, PipeError, StopFlag};
use klipolo_capture::protocol::Command;
use klipolo_capture::run::check::check_camera_with;
use klipolo_capture::shell::Shell;
use klipolo_capture::synthetic::{FakeBackend, Script};
use preview_io::{SharedOut, sent_after};

/// Check camera with the app's Cancel sent `cancel_ms` after the press, on a preview that takes `build_ms` to build
/// and whose start waits `prompt_ms` on the system first. The exit code, what the app was sent, and how often the
/// camera opened.
fn cancelled_check(build_ms: u64, prompt_ms: u64, cancel_ms: u64) -> (i32, String, u64) {
    let input = sent_after(&Command::Cancel, cancel_ms);
    let backend = SlowPreview {
        inner: FakeBackend::new(Script::default()),
        opened: Arc::default(),
        build_ms,
        prompt_ms,
    };
    let opened = backend.opened.clone();
    let out = SharedOut::default();
    let code = check_camera_with(Box::new(backend), "camera-1", input, Box::new(out.clone()));
    (code, out.text(), opened.load(Ordering::SeqCst))
}

#[test]
fn a_cancel_sent_right_after_check_camera_opens_nothing() {
    // The Cancel lands while the preview is built (capture consent).
    let (code, _, opened) = cancelled_check(200, 0, 0);
    assert_eq!(code, 0);
    assert_eq!(opened, 0, "the camera never opened");
}

#[test]
fn a_cancel_during_the_camera_s_prompt_opens_nothing_and_ends_cleanly() {
    // The Cancel lands while the camera's start waits on macOS's prompt: the start checks the
    // person's Stop right before it opens, and Check camera ends as cancelled (Ready, 0), not as a failure.
    let (code, sent, opened) = cancelled_check(0, 300, 100);
    assert_eq!(code, 0);
    assert_eq!(opened, 0, "the camera never opened");
    assert!(!sent.contains(r#""ev":"failed""#), "{sent}");
    assert!(sent.contains(r#""state":"ready""#), "{sent}");
}

#[test]
fn every_frame_the_preview_paces_reaches_the_app() {
    // The preview pipe paces its frames about 15 a second; they arrive a couple of ms early or late (the camera's
    // spacing, the JPEG's time). Check camera sends each one on: none is dropped after it was encoded.
    const FRAMES: u64 = 24;
    let backend = WithPreview(FakeBackend::new(Script::default()), Jittered(FRAMES));
    let input = sent_after(&Command::Cancel, FRAMES * 67 + 400);
    let out = SharedOut::default();
    let code = check_camera_with(Box::new(backend), "camera-1", input, Box::new(out.clone()));
    let sent = out.text();
    assert_eq!(code, 0);
    assert_eq!(sent.matches(r#""ev":"preview""#).count() as u64, FRAMES);
}

/// A preview that sends `.0` frames 66.7 ms apart, alternately 2 ms early and 2 ms late.
#[derive(Clone, Copy)]
struct Jittered(u64);

impl PreviewPipe for Jittered {
    fn start(&mut self, frames: SyncSender<PreviewFrame>, _: StopFlag) -> Result<(), PipeError> {
        let n = self.0;
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            for k in 0..n {
                let jitter = if k % 2 == 0 { -2.0 } else { 2.0 };
                let due = Duration::from_secs_f64((k as f64 * 1_000.0 / 15.0 + 2.0 + jitter) / 1e3);
                std::thread::sleep(due.saturating_sub(start.elapsed()));
                let frame = PreviewFrame {
                    jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
                    width: 320,
                    height: 180,
                };
                let _ = frames.send(frame);
            }
        });
        Ok(())
    }
    fn stop(&mut self) {}
}

/// The fake backend with its preview replaced.
struct WithPreview(FakeBackend, Jittered);

impl Backend for WithPreview {
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
        self.0.video(spec)
    }
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        self.0.audio(spec)
    }
    fn preview(&self, _camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Ok(Box::new(self.1))
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.0.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        self.0.shell()
    }
}

/// The fake backend, its preview slow to build or to get past the system, and its camera's opens counted.
struct SlowPreview {
    inner: FakeBackend,
    opened: Arc<AtomicU64>,
    build_ms: u64,
    prompt_ms: u64,
}

/// A preview whose `start` waits `prompt_ms` (the system's prompt), then opens unless the person stopped.
struct Counted {
    pipe: Box<dyn PreviewPipe>,
    opened: Arc<AtomicU64>,
    prompt_ms: u64,
}

impl PreviewPipe for Counted {
    fn start(
        &mut self,
        frames: SyncSender<PreviewFrame>,
        stopping: StopFlag,
    ) -> Result<(), PipeError> {
        std::thread::sleep(Duration::from_millis(self.prompt_ms));
        if stopping.is_set() {
            return Err(PipeError::Os("cancelled before the camera opened".into()));
        }
        self.opened.fetch_add(1, Ordering::SeqCst);
        self.pipe.start(frames, stopping)
    }
    fn stop(&mut self) {
        self.pipe.stop();
    }
}

impl Backend for SlowPreview {
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
        self.inner.audio(spec)
    }
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        std::thread::sleep(Duration::from_millis(self.build_ms));
        let pipe = self.inner.preview(camera)?;
        Ok(Box::new(Counted {
            pipe,
            opened: self.opened.clone(),
            prompt_ms: self.prompt_ms,
        }))
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.inner.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        self.inner.shell()
    }
}
