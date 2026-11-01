//! A display, an app, a game or a window, captured with ScreenCaptureKit and encoded with VideoToolbox:
//! `MacScreenPipe`, the `VideoPipe` that owns the stream, our frame pool and the
//! encoder thread. Its sound, when asked for, comes on the same stream to a `MacSoundPipe` (`SoundLink`).
//!
//! **The stream** (made at each `start`, after Record): the preset's size fitted to the captured area's shape (scaled
//! on the GPU by ScreenCaptureKit), a minimum frame interval of 1 / (fps × 1.1), a queue of 6 buffers (8 at 100 fps or
//! more), `420v` with the BT.709 matrix and sRGB, the system cursor only where it is burned in (`showsCursor`: a
//! separate cursor is its own source, `cursor::MacCursorSource`; a game records none), the target's sound at
//! 48 kHz stereo without this process's, and Presenter Overlay's privacy alert off (the pipe warns when the overlay
//! shows). Its clock is the host clock; the skew is checked and logged at each start.
//!
//! **The cursor's live fallback**: `set_cursor_in_frames` applies the same settings with `showsCursor`
//! on through `updateConfiguration:` and answers once the stream acknowledged it (up to 1 s), so the session journals
//! the burn-in at that time; the pipe's next start shows the cursor whatever the answer, and it never switches back.
//!
//! **The frame path:** `feed` copies each new picture into our pool on the stream's queue and hands it to the encoder
//! thread (`encode_thread`, through `handoff`), which encodes it with the take's pinned encoder, falling back along `VideoSpec::encoders`
//! (`encoder_pick`). Stops and the target's comings and goings come as `PipeEvent`s.
//!
//! **Stop** stops the stream (waiting up to 1 s), then has the encoder thread encode what was queued and finish, so
//! nothing is sent once it returns.

mod damage;
mod feed;
mod setup;

pub(super) use feed::status_of;
pub use setup::frame_interval;
pub(super) use setup::{encoder, start_error};

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use self::damage::{OwnDamage, PictureArea};
use self::feed::ScreenFeed;
use self::setup::{Running, finish, stream_config};
use super::encode_thread::{EncoderThread, FRAMES_IN_FLIGHT};
use super::filter;
use super::handoff::{Shared, ToEncoder};
use super::host_time::host_clock_skew_ns;
use super::pool::SurfacePool;
use super::sound::SoundLink;
use crate::backend::{PresetId, VideoSpec};
use crate::encoder_pick::start_first;
use crate::guard::assert_real_capture_allowed;
use crate::media::StreamInfo;
use crate::pipe::{PipeError, PipeEvent, VideoOut, VideoPipe, VideoStats, first_or_stop};
use crate::platform::common::sck::pipe_event_for;
use crate::preflight::cursor::CursorPlan;
use crate::preset::lighter;
use crate::vendor::cap::sck::{Capturer, Frame};

const START_WAIT: Duration = Duration::from_secs(10);
/// The first picture's packet, which describes the stream for `start`'s answer.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(5);
const STOP_WAIT: Duration = Duration::from_secs(1);

pub struct MacScreenPipe {
    spec: VideoSpec,
    preset: PresetId,
    link: Arc<SoundLink>,
    /// The display this pipe records (`OsBackend::display_awake`), by `CGDirectDisplayID`; 0 before a start.
    display: Arc<AtomicU32>,
    shared: Arc<Shared>,
    running: Option<Running>,
}

impl MacScreenPipe {
    /// Opens nothing: the content list and the stream wait for `start`.
    pub(super) fn new(spec: &VideoSpec, link: Arc<SoundLink>, display: Arc<AtomicU32>) -> Self {
        assert_real_capture_allowed("the screen");
        Self {
            preset: spec.preset,
            spec: spec.clone(),
            link,
            display,
            shared: Arc::default(),
            running: None,
        }
    }

    /// The content list, the encoder, the pool and the stream, started, up to its first picture's description.
    fn open(&mut self, out: VideoOut) -> Result<StreamInfo, PipeError> {
        let content = filter::content()?;
        let picked = filter::pick(&content, &self.spec.target)?;
        drop(content);
        self.display.store(picked.display.0, Ordering::Release);
        let pixels = picked.pixels;
        let ((encoder, size, fps), _) = start_first(&self.spec.encoders, &out.events, |pick| {
            setup::encoder(self.preset, pick, picked.pixels, &out)
        })?;
        let pool = SurfacePool::for_preset(size.0, size.1, fps)?;
        let shared = Arc::new(Shared::default());
        let burned = self.spec.cursor == CursorPlan::Burned;
        shared.cursor_in_frames.store(burned, Ordering::Release);
        let (to_encoder, frames) = sync_channel(FRAMES_IN_FLIGHT);
        let (info_tx, info_rx) = sync_channel(1);
        let thread = EncoderThread {
            name: "klipolo-screen-encode",
            refeeds: true,
            encoder,
            frames,
            shared: shared.clone(),
            info: info_tx,
        };
        let encoder = thread
            .spawn()
            .map_err(|e| PipeError::Os(format!("the encoder's thread: {e}")))?;
        let mut feed = ScreenFeed {
            pool,
            encoder: to_encoder.clone(),
            shared: shared.clone(),
            events: out.events.clone(),
            blank: false,
            overlay_noted: false,
            overlay_logged: false,
            damage: OwnDamage::new(
                (!picked.window).then(|| PictureArea::display(picked.display)),
                burned,
            ),
        };
        let (link, gate) = (self.link.clone(), shared.clone());
        let (stopped, became) = (out.events.clone(), out.events.clone());
        let window = picked.window;
        let cfg = stream_config(&self.spec, window, size, fps);
        let again = cfg.retained();
        let built = Capturer::builder(picked.filter, cfg)
            .with_output_sample_buf_cb(move |frame| match frame {
                Frame::Screen(f) => feed.screen(f.sample_buf()),
                Frame::Audio(a) if !gate.stopped.load(Ordering::Acquire) => {
                    link.audio(a.sample_buf())
                }
                Frame::Audio(_) => {}
            })
            .with_stop_with_err_cb(move |_, error| {
                tracing::warn!(code = error.code(), "screen: the stream stopped");
                let _ = stopped.try_send(pipe_event_for(error.code()));
            })
            .with_active_cb(move |active| {
                let event = if active {
                    PipeEvent::TargetBack
                } else {
                    PipeEvent::TargetGone
                };
                let _ = became.try_send(event);
            })
            .build();
        let running = Running {
            capturer: built.map_err(|e| PipeError::Os(format!("the stream: {}", e.code())))?,
            to_encoder,
            encoder,
            settings: (window, size, fps),
        };
        let answers = shared.clone();
        let nudge = running
            .capturer
            .nudger(again, move |error| answers.answered(error));
        shared.set_nudge(Some(Box::new(nudge)));
        tracing::info!(
            ?size,
            fps,
            pixels = ?pixels,
            skew_ns = host_clock_skew_ns(),
            "screen: starting, on the host clock"
        );
        // The content list can wait on macOS's prompt: the person may have stopped meanwhile.
        if out.stopping.is_set() {
            finish(&shared, running);
            return Err(PipeError::Os("stopped before the screen opened".into()));
        }
        if let Err(e) = running.capturer.start_within(START_WAIT) {
            // One that didn't answer in time may still start: it is stopped, never left running unowned.
            if e.is_none() && !running.capturer.stop_within(STOP_WAIT) {
                tracing::warn!("screen: a start that timed out didn't stop within 1 s either");
            }
            finish(&shared, running);
            return Err(
                e.map_or(PipeError::Os("the stream didn't start".into()), |e| {
                    start_error(e.code())
                }),
            );
        }
        let info = match first_or_stop(&info_rx, FIRST_FRAME_WAIT, &out.stopping) {
            Ok(info) => info,
            Err(why) => {
                tracing::warn!(?why, "screen: no picture after the start");
                running.capturer.stop_within(STOP_WAIT);
                finish(&shared, running);
                return Err(PipeError::Os(format!("no picture came ({why:?})")));
            }
        };
        // The first frame went out before the session heard of this start: the next one opens the run.
        shared.key_wanted.store(true, Ordering::Release);
        (self.shared, self.running) = (shared, Some(running));
        Ok(info)
    }
}

impl VideoPipe for MacScreenPipe {
    /// Every outcome reaches the stream's sound (`SharedStream`), so a screen that fails before its stream runs fails
    /// its sound at once.
    fn start(&mut self, out: VideoOut) -> Result<StreamInfo, PipeError> {
        assert_real_capture_allowed("ScreenCaptureKit");
        self.stop();
        self.link.stream.idle();
        let started = self.open(out);
        match &started {
            Ok(_) => self.link.stream.running(),
            Err(e) => self.link.stream.failed(e.clone()),
        }
        started
    }

    fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
        if !paused {
            self.shared.key_wanted.store(true, Ordering::Release);
            // The encoder thread may be asleep until its next nudge: it re-feeds at once (a full channel is awake).
            if let Some(running) = &self.running {
                let _ = running.to_encoder.try_send(ToEncoder::Wake);
            }
        }
    }

    fn request_keyframe(&mut self) {
        self.shared.key_wanted.store(true, Ordering::Release);
    }

    fn lower_preset(&mut self) -> bool {
        lighter(self.preset).is_some_and(|p| {
            self.preset = p;
            true
        })
    }

    fn stop(&mut self) {
        let Some(running) = self.running.take() else {
            return;
        };
        self.shared.stopped.store(true, Ordering::Release);
        if !running.capturer.stop_within(STOP_WAIT) {
            tracing::warn!("screen: the stream didn't stop within 1 s");
        }
        self.link.stream.idle();
        let (copies, wall_us, cpu_us) = self.shared.copy_cost();
        let nudges = self.shared.nudges.load(Ordering::Relaxed);
        let stats = self.shared.stats();
        tracing::info!(
            copies,
            wall_us,
            cpu_us,
            nudges,
            answered = stats.answered,
            ?stats,
            "screen: stopped"
        );
        finish(&self.shared, running);
    }

    fn stats(&self) -> VideoStats {
        self.shared.stats()
    }

    /// The live fallback (see the module notes): Ok once the stream acknowledged it; Err when it isn't running, or
    /// didn't answer within 1 s or refused (the session heals: the next start shows the cursor).
    fn set_cursor_in_frames(&mut self, on: bool) -> Result<(), PipeError> {
        if !on {
            return Err(PipeError::Unsupported);
        }
        self.spec.cursor = CursorPlan::Burned;
        let running =
            (self.running.as_ref()).ok_or(PipeError::Os("the screen isn't running".into()))?;
        // From here the frames may show the cursor, even if the stream answers late: the damage rule counts its moves.
        self.shared.cursor_in_frames.store(true, Ordering::Release);
        let shown = setup::show_cursor(&self.spec, running, &self.shared);
        match &shown {
            Ok(()) => tracing::info!("screen: the stream shows the cursor from here"),
            Err(e) => {
                tracing::warn!(error = %e, "screen: the stream couldn't show the cursor live")
            }
        }
        shown
    }
}

impl Drop for MacScreenPipe {
    fn drop(&mut self) {
        self.stop();
    }
}
