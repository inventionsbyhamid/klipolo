//! The recorder's live view on the Mac (the one exception to capture only after a press): `MacLiveView`, the `PreviewPipe` over
//! a small ScreenCaptureKit stream of the take's own target with the take's own content filter (`filter`: Klipolo's
//! windows and the notification apps left out of a display, a window by itself, an app's or game's windows on their
//! display).
//!
//! **Never a prompt:** `start` reads Screen Recording's preflight (`CGPreflightScreenCaptureAccess`, which never asks)
//! before anything else, and a refusal calls nothing more. Only then is the content list read and the stream opened
//! (macOS's own periodic "continue to allow" confirmation may appear then: macOS's own
//! reminder, not a request Klipolo makes).
//!
//! **The stream:** scaled by ScreenCaptureKit itself (never captured full size) to the picture `live_view_size` gives
//! for the view that shows it: in the recorder's small view, 640 × 640 in the target's shape at 10 frames a second; in
//! its Large and Full screen views, the view's own pixels within 1080p at up to 30. A queue of 3, BGRA in
//! sRGB, no cursor, no sound. Each `Complete` frame the pacer takes (`pace::PreviewPacer` at the picture's rate)
//! becomes a JPEG through Core Image (`jpeg`), offered with `try_send`: dropped, never waited for. Nothing is written.
//!
//! **A new view size** (`resize`) is applied to the running stream (`updateConfiguration:`), never by opening another:
//! a smaller picture's lower rate at once, a bigger one's higher rate once the stream has taken it. One the stream
//! didn't answer in time leaves its picture unknown, so the worker's next try always applies it
//! never left big while the view is small). The big pictures' pixels a second are this computer's
//! (`live_view_size::this_computer_s_pixel_rate`: about 720p at 15 on an Intel Mac).
//!
//! **Its end:** a stream that stops by itself, or whose target goes, drops the frames' sender (shared through an
//! `Option` that its callbacks take), so the worker's loop ends and the UI never keeps its last frame as live.
//!
//! Memory: ScreenCaptureKit's 3 BGRA buffers, of at most 640 × 640 in the small view (1.6 MB each) and 1920 × 1080 in a
//! big one (8.3 MB each), one Core Image context, and at most two JPEGs in flight (about 20-60 KB small, a few hundred
//! KB big).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cidre::{cg, cm, cv, sc};
use objc2_core_graphics::CGPreflightScreenCaptureAccess;

use super::filter;
use super::host_time::host_ns_of;
use super::jpeg::Jpeg;
use super::screen::{start_error, status_of};
use crate::backend::{PreviewFrame, PreviewPipe, Target};
use crate::guard::assert_real_capture_allowed;
use crate::live_view_size::{LiveFrame, ViewPixels, live_frame, this_computer_s_pixel_rate};
use crate::pace::PreviewPacer;
use crate::pipe::{PipeError, StopFlag};
use crate::vendor::cap::sck::{Capturer, Frame, StreamCfgBuilder};

/// ScreenCaptureKit's buffers (Apple's default, the smallest that keeps a slow stream smooth).
const QUEUE: isize = 3;
const START_WAIT: Duration = Duration::from_secs(10);
const STOP_WAIT: Duration = Duration::from_secs(1);
/// How long a new view size may take to apply (`updateConfiguration:`'s answer).
const UPDATE_WAIT: Duration = Duration::from_secs(1);

/// The worker's frames, until the stream ends by itself.
type Frames = Arc<Mutex<Option<SyncSender<PreviewFrame>>>>;

pub struct MacLiveView {
    target: Target,
    /// The view it shows in (None: the recorder's small view), as the app last said it.
    view: Option<ViewPixels>,
    /// A big picture's most pixels a second on this computer.
    rate: u64,
    running: Option<Running>,
}

/// The open stream, and what it sends.
struct Running {
    capturer: Capturer,
    /// The target's own pixels, and whether it is one window, as it was picked.
    source: (u32, u32),
    window: bool,
    /// The picture the stream is set to send; None after a change it didn't answer in time (it may yet take it).
    frame: Option<LiveFrame>,
    /// The pacer's rate, read on the stream's queue before each frame.
    fps: Arc<AtomicU32>,
}

impl MacLiveView {
    /// The guard first; a screen, window, app or game only. Nothing is opened or read while it is built.
    pub(super) fn new(target: &Target) -> Result<Self, PipeError> {
        assert_real_capture_allowed("the live view");
        if matches!(target, Target::Camera { .. } | Target::Portal { .. }) {
            return Err(PipeError::Unsupported);
        }
        Ok(Self {
            target: target.clone(),
            view: None,
            rate: this_computer_s_pixel_rate(),
            running: None,
        })
    }
}

impl PreviewPipe for MacLiveView {
    fn start(
        &mut self,
        frames: SyncSender<PreviewFrame>,
        stopping: StopFlag,
    ) -> Result<(), PipeError> {
        assert_real_capture_allowed("the live view's stream");
        self.stop();
        // Only while Screen Recording is allowed already: the preflight never prompts, and a refusal reads no content.
        if !CGPreflightScreenCaptureAccess() {
            return Err(PipeError::PermissionDenied);
        }
        if stopping.is_set() {
            return Err(PipeError::Os("stopped before the live view opened".into()));
        }
        let content = filter::content()?;
        let picked = filter::pick(&content, &self.target)?;
        drop(content);
        let (source, window) = (picked.pixels, picked.window);
        let frame = live_frame(self.view, source, self.rate);
        let cfg = stream_config(frame, window);
        let jpeg = Jpeg::new()?;
        let fps = Arc::new(AtomicU32::new(frame.fps));
        let (rate, mut pacer) = (fps.clone(), PreviewPacer::at(frame.fps));
        let frames: Frames = Arc::new(Mutex::new(Some(frames)));
        let (stopped, gone) = (frames.clone(), frames.clone());
        let capturer = Capturer::builder(picked.filter, cfg)
            .with_output_sample_buf_cb(move |sample| {
                if let Frame::Screen(f) = sample {
                    pacer.retime(rate.load(Ordering::Relaxed));
                    offer(f.sample_buf(), &mut pacer, &jpeg, &frames);
                }
            })
            .with_stop_with_err_cb(move |_, error| {
                tracing::warn!(code = error.code(), "live view: the stream stopped");
                end(&stopped);
            })
            .with_active_cb(move |active| {
                if !active {
                    tracing::info!("live view: the target went");
                    end(&gone);
                }
            })
            .build()
            .map_err(|e| PipeError::Os(format!("the live view's stream: {}", e.code())))?;
        // The content list can wait on macOS: the person may have stopped meanwhile.
        if stopping.is_set() {
            return Err(PipeError::Os("stopped before the live view opened".into()));
        }
        if let Err(e) = capturer.start_within(START_WAIT) {
            // One that didn't answer in time may still start: it is stopped, never left running unowned.
            if e.is_none() && !capturer.stop_within(STOP_WAIT) {
                tracing::warn!("live view: a start that timed out didn't stop within 1 s either");
            }
            return Err(
                e.map_or(PipeError::Os("the live view didn't start".into()), |e| {
                    start_error(e.code())
                }),
            );
        }
        tracing::info!(?frame, "live view: running");
        self.running = Some(Running {
            capturer,
            source,
            window,
            frame: Some(frame),
            fps,
        });
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            if !running.capturer.stop_within(STOP_WAIT) {
                tracing::warn!("live view: the stream didn't stop within 1 s");
            }
            tracing::info!("live view: stopped");
        }
    }

    /// The view it shows in: before the start, the one it opens for; after, applied to the running stream. A smaller
    /// picture's lower rate applies at once; a bigger picture's higher rate once the stream has taken it. One the
    /// stream didn't take (no answer within `UPDATE_WAIT`, or an error) leaves its picture unknown at the lower of the
    /// two rates, and says false, so the worker tries it again.
    fn resize(&mut self, view: Option<ViewPixels>) -> bool {
        self.view = view;
        let Some(running) = self.running.as_mut() else {
            return true;
        };
        let next = live_frame(view, running.source, self.rate);
        if running.frame == Some(next) {
            return true;
        }
        let lower = next.fps.min(running.fps.load(Ordering::Relaxed));
        running.fps.store(lower, Ordering::Relaxed);
        let cfg = stream_config(next, running.window);
        match running.capturer.update_within(&cfg, UPDATE_WAIT) {
            Ok(()) => {
                running.fps.store(next.fps, Ordering::Relaxed);
                running.frame = Some(next);
                tracing::info!(?next, "live view: resized");
                true
            }
            Err(code) => {
                running.frame = None;
                tracing::warn!(?code, "live view: the new size wasn't taken yet");
                false
            }
        }
    }
}

impl Drop for MacLiveView {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One screen buffer, on the stream's queue: a `Complete` picture the pacer takes becomes a JPEG for the worker; the
/// rest are never encoded.
fn offer(sample: &cm::SampleBuf, pacer: &mut PreviewPacer, jpeg: &Jpeg, frames: &Frames) {
    if !matches!(status_of(sample), sc::FrameStatus::Complete) {
        return;
    }
    let Some(image) = sample.image_buf() else {
        return;
    };
    // The stream ended: nothing more goes out.
    let Some(to) = frames
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    else {
        return;
    };
    if !pacer.take(host_ns_of(sample.pts())) {
        return;
    }
    if let Some(bytes) = jpeg.encode(image) {
        let _ = to.try_send(PreviewFrame {
            jpeg: bytes,
            width: image.width() as u32,
            height: image.height() as u32,
        });
    }
}

/// The stream ended by itself: its frames' sender goes, so the worker's loop ends.
fn end(frames: &Frames) {
    frames.lock().unwrap_or_else(PoisonError::into_inner).take();
}

/// The live view's stream: `frame`'s size, at most its frames a second, 3 buffers, BGRA in sRGB, no cursor and no
/// sound; a window's picture scaled into it as the window changes size, as a take's is.
fn stream_config(frame: LiveFrame, window: bool) -> cidre::arc::R<sc::StreamCfg> {
    let mut c = StreamCfgBuilder::default();
    c.set_width(frame.size.0 as usize);
    c.set_height(frame.size.1 as usize);
    c.set_frame_interval(cm::Time::new(1, frame.fps as i32));
    c.set_queue_depth(QUEUE);
    c.set_pixel_format(cv::PixelFormat::_32_BGRA);
    c.set_color_space_name(cg::color_space::names::srgb());
    c.set_shows_cursor(false);
    c.set_presenter_overlay_alert(sc::PresenterOverlayAlertSetting::Never);
    if window {
        c.set_scales_to_fit(true);
        c.set_preserves_aspect_ratio(true);
    }
    c.build()
}
