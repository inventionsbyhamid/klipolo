//! The previews' fake pipe (Check camera's, the live view's): frames on a thread, never a camera or a screen.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_platform::clock::host_ns;

use super::{Worker, wait_until};
use crate::backend::{PreviewFrame, PreviewPipe};
use crate::live_view_size::{BIG_PIXEL_RATE, LiveFrame, ViewPixels, live_frame};
use crate::pipe::{PipeError, StopFlag};

/// The display the fake live view shows: 2560 × 1600 pixels, so its small picture is 640 × 400. Its big pictures are
/// Apple silicon's (`BIG_PIXEL_RATE`), whatever computer runs the tests.
pub const FAKE_DISPLAY: (u32, u32) = (2560, 1600);

/// A preview's fake frames, each only a JPEG's start and end markers (not a picture): Check camera's 320×180 at a fixed
/// rate; the live view's its picture of `FAKE_DISPLAY` for the view it is told (`live_view_size`), changed as it runs.
pub(super) struct FakePreview {
    start_error: Option<PipeError>,
    /// The frames' size and rate now (the thread reads it before each frame).
    frame: Arc<Mutex<LiveFrame>>,
    /// The live view's: its frames follow the view (`resize`). Check camera's don't.
    follows_view: bool,
    /// The next this many sizes aren't taken (`resize` says false), as a stream that didn't answer in time.
    refuses: u32,
    /// The stream ends by itself this long after its start (its sender dropped), as a stopped one does.
    ends_after_ms: Option<u32>,
    worker: Option<Worker>,
}

impl FakePreview {
    /// Check camera's fake: 320×180 at `fps`, whose start fails with `start_error`.
    pub(super) fn camera(start_error: Option<PipeError>, fps: u32) -> Box<dyn PreviewPipe> {
        let frame = LiveFrame {
            size: (320, 180),
            fps,
        };
        Self::boxed(start_error, frame, false, None)
    }

    /// The live view's fake, in the small view until told otherwise, whose start fails with `start_error`, whose
    /// stream ends `ends_after_ms` in, and whose stream doesn't take its first `refuses` new sizes.
    pub(super) fn live_view(
        start_error: Option<PipeError>,
        ends_after_ms: Option<u32>,
        refuses: u32,
    ) -> Box<dyn PreviewPipe> {
        let small = live_frame(None, FAKE_DISPLAY, BIG_PIXEL_RATE);
        let mut fake = Self::new(start_error, small, true, ends_after_ms);
        fake.refuses = refuses;
        Box::new(fake)
    }

    fn boxed(
        start_error: Option<PipeError>,
        frame: LiveFrame,
        follows_view: bool,
        ends_after_ms: Option<u32>,
    ) -> Box<dyn PreviewPipe> {
        Box::new(Self::new(start_error, frame, follows_view, ends_after_ms))
    }

    fn new(
        start_error: Option<PipeError>,
        frame: LiveFrame,
        follows_view: bool,
        ends_after_ms: Option<u32>,
    ) -> Self {
        Self {
            start_error,
            frame: Arc::new(Mutex::new(frame)),
            follows_view,
            refuses: 0,
            ends_after_ms,
            worker: None,
        }
    }
}

impl PreviewPipe for FakePreview {
    fn start(
        &mut self,
        frames: SyncSender<PreviewFrame>,
        stopping: StopFlag,
    ) -> Result<(), PipeError> {
        if let Some(e) = &self.start_error {
            return Err(e.clone());
        }
        if stopping.is_set() {
            return Err(PipeError::Os("cancelled before the camera opened".into()));
        }
        if self.worker.is_some() {
            return Err(PipeError::Busy);
        }
        let start = host_ns();
        let end = (self.ends_after_ms).map_or(u64::MAX, |ms| start + u64::from(ms) * 1_000_000);
        let now = self.frame.clone();
        self.worker = Some(Worker::spawn("fake-preview", move |stop: &AtomicBool| {
            let mut due = start;
            while due < end && wait_until(due, stop) {
                let LiveFrame { size, fps } = *now.lock().unwrap_or_else(PoisonError::into_inner);
                let frame = PreviewFrame {
                    jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
                    width: size.0,
                    height: size.1,
                };
                let _ = frames.try_send(frame);
                due += 1_000_000_000 / u64::from(fps.max(1));
            }
        }));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
    }

    fn resize(&mut self, view: Option<ViewPixels>) -> bool {
        if !self.follows_view {
            return true;
        }
        // A stream that is running and doesn't answer in time keeps its picture.
        if self.worker.is_some() && self.refuses > 0 {
            self.refuses -= 1;
            return false;
        }
        let frame = live_frame(view, FAKE_DISPLAY, BIG_PIXEL_RATE);
        *self.frame.lock().unwrap_or_else(PoisonError::into_inner) = frame;
        true
    }
}
