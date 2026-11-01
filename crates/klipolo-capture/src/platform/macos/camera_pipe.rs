//! The camera as its own file and lane (camera recordings stay separate so overlays stay editable):
//! `MacCameraPipe`, the `VideoPipe` over AVFoundation (`camera`), our frame pool and the encoder thread the screen
//! uses (`encode_thread`, without re-feeds: a camera's file holds only the device's own frames).
//!
//! **Start**, after Record: camera access (the prompt, the first time), the device by its unique id, its format for
//! the preset (`pick_format`, `frame_durations`), the take's encoders tried in order on another engine than the screen's where
//! there is one (`VideoSpec::encoders`, from `probe::camera_encoders_for`), the pool and the encoder thread; then the
//! session, NV12 (`420v`) out, late frames discarded. The person's Stop is checked before every step that asks for or
//! opens the camera. `start` answers once the first frame is encoded.
//!
//! **Each buffer**, on the session's queue: counted, its time moved from the session's synchronization clock to the host
//! clock, copied into the pool on the GPU and handed to the encoder thread; nothing while paused; a late frame
//! AVFoundation discarded counts as dropped.
//!
//! **The camera bubble**: a take that shows it (`VideoSpec::bubble`) also gets a preview layer on its
//! session, made before the device's format is set and the session runs (`Want::preview`, so it never resets the
//! chosen format or rate), handed to the bubble (`BubbleFeed::set`) and let go as it stops. No
//! second open, no frame copied.
//!
//! **Events** (`camera_notices`): the camera's own disconnect is `DeviceLost` and its return `DeviceBack` (the heal
//! policy waits for it, then restarts the pipe), the session's runtime errors and interruptions `Stalled`, and a
//! suspended camera (a closed lid, a shutter) `Blank`. While suspended it counts as delivering blank pictures twice a
//! second (`suspended_blanks`), as a sleeping display does, so the stall rule doesn't restart a covered camera.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use cidre::cv;
use klipolo_platform::clock::host_ns;

use super::bubble::BubbleFeed;
use super::camera::{self, CameraSession, Want};
use super::camera_feed::{Suspension, feed};
use super::camera_notices::{DeviceNotices, SessionNotices};
use super::encode_thread::{EncoderThread, FRAMES_IN_FLIGHT};
use super::encoders;
use super::handoff::{Shared, ToEncoder};
use super::pool::SurfacePool;
use crate::backend::{EncoderPick, PresetId, Target, VideoSpec};
use crate::encoder_pick::start_first;
use crate::guard::assert_real_capture_allowed;
use crate::media::{Codec, StreamInfo};
use crate::pipe::{PipeError, PipeEvent, VideoOut, VideoPipe, VideoStats, first_or_stop};
use crate::platform::common::camera::{BubbleMirroring, FormatFacts, frame_durations, pick_format};
use crate::platform::common::vt_ids::camera_own_order;
use crate::preset::{lighter, preset};

/// The first frame's packet, which describes the stream for `start`'s answer (a camera can take a second to expose).
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(5);

pub struct MacCameraPipe {
    /// The camera's unique id (`DeviceInfo::id`).
    id: String,
    spec: VideoSpec,
    preset: PresetId,
    shared: Arc<Shared>,
    suspension: Arc<Suspension>,
    /// The camera's disconnect and return, kept across starts (a return must reach a pipe whose start failed).
    notices: Option<DeviceNotices>,
    running: Option<Running>,
    /// The take's camera bubble, which this camera's session feeds when `spec.bubble`.
    bubble: Arc<BubbleFeed>,
}

struct Running {
    _notices: SessionNotices,
    session: CameraSession,
    to_encoder: SyncSender<ToEncoder>,
    encoder: JoinHandle<()>,
}

impl MacCameraPipe {
    /// The guard first, then the camera by its unique id: nothing is opened or asked for until `start`.
    pub(super) fn new(spec: &VideoSpec, bubble: Arc<BubbleFeed>) -> Result<Self, PipeError> {
        assert_real_capture_allowed("the camera");
        let Target::Camera { id } = &spec.target else {
            return Err(PipeError::Unsupported);
        };
        if camera::device(id).is_none() {
            return Err(PipeError::NoDevice);
        }
        Ok(Self {
            id: id.clone(),
            spec: spec.clone(),
            preset: spec.preset,
            shared: Arc::default(),
            suspension: Arc::default(),
            notices: None,
            running: None,
            bubble,
        })
    }

    /// The take's encoders (`probe::camera_encoders_for`); with none named, this Mac's shared hardware H.264, then a
    /// software one, then a dedicated engine the screen may be on, last (`camera_own_order`).
    fn encoders(&self) -> Result<Vec<EncoderPick>, PipeError> {
        if !self.spec.encoders.is_empty() {
            return Ok(self.spec.encoders.clone());
        }
        let arch = klipolo_platform::caps::current().os.cpu_arch;
        let listed = encoders::listed();
        let all: Vec<_> = (listed.iter())
            .map(|(e, hardware)| (e.id.as_str(), e.codec, *hardware))
            .collect();
        let own: Vec<EncoderPick> = (camera_own_order(&all, &arch).into_iter())
            .map(|id| EncoderPick {
                codec: Codec::H264,
                id,
            })
            .collect();
        if own.is_empty() {
            tracing::warn!("camera: no H.264 encoder to record on");
            return Err(PipeError::Unsupported);
        }
        Ok(own)
    }

    /// The device's notices, made at the first start, now sending to `events`.
    fn notices(&mut self, events: &SyncSender<PipeEvent>) -> &DeviceNotices {
        let (id, suspension) = (self.id.clone(), self.suspension.clone());
        let notices = self.notices.get_or_insert_with(|| {
            DeviceNotices::new(&id, move |event| {
                if let PipeEvent::Blank(suspended) = event {
                    suspension.set(*suspended, host_ns());
                }
            })
        });
        notices.send_to(events.clone());
        notices
    }
}

impl VideoPipe for MacCameraPipe {
    fn start(&mut self, out: VideoOut) -> Result<StreamInfo, PipeError> {
        assert_real_capture_allowed("AVFoundation's camera");
        self.stop();
        self.notices(&out.events);
        camera::access(&out.stopping)?;
        let device = camera::device(&self.id).ok_or(PipeError::NoDevice)?;
        let formats = camera::formats(&device);
        let facts: Vec<FormatFacts> = formats.iter().map(|f| f.facts).collect();
        let p = preset(self.preset, Codec::H264);
        let i = pick_format(&facts, (p.width, p.height, p.fps)).ok_or(PipeError::Unsupported)?;
        let durations = frame_durations(&formats[i].ranges, p.fps);
        let pixels = (facts[i].width, facts[i].height);
        let picks = self.encoders()?;
        let ((encoder, size, fps), _) = start_first(&picks, &out.events, |pick| {
            super::screen::encoder(self.preset, pick, pixels, &out)
        })?;
        let pool = SurfacePool::for_preset(size.0, size.1, fps)?;
        let shared = Arc::new(Shared::default());
        let (to_encoder, frames) = sync_channel(FRAMES_IN_FLIGHT);
        let (info, info_rx) = sync_channel(1);
        let thread = EncoderThread {
            name: "klipolo-camera-encode",
            refeeds: false,
            encoder,
            frames,
            shared: shared.clone(),
            info,
        };
        let encoder = thread
            .spawn()
            .map_err(|e| PipeError::Os(format!("the encoder's thread: {e}")))?;
        let clock = Arc::new(OnceLock::new());
        let callback = feed(shared.clone(), pool, to_encoder.clone(), clock.clone());
        let want = Want {
            format: i,
            frame_durations: durations,
            pixels: cv::PixelFormat::_420V,
            size: None,
            preview: BubbleMirroring::of(&self.spec),
        };
        // Access and the encoder can take a while: the person may have stopped meanwhile.
        let opened = if out.stopping.is_set() {
            Err(PipeError::Os("stopped before the camera opened".into()))
        } else {
            CameraSession::open(device, &want, callback, &out.stopping)
        };
        let session = match opened {
            Ok(session) => session,
            Err(e) => {
                finish_thread(&shared, &to_encoder, encoder);
                return Err(e);
            }
        };
        let _ = clock.set(session.clock());
        if let Some(layer) = &session.layer {
            self.bubble.set(layer.retained());
        }
        let notices = self.notices(&out.events);
        let running = Running {
            _notices: notices.session(session.session(), session.device()),
            session,
            to_encoder,
            encoder,
        };
        tracing::info!(format = ?facts[i], ?size, fps, "camera: running");
        // A camera slow to send (covered, waking, on a hub) is stopped at once on the person's Stop.
        let info = match first_or_stop(&info_rx, FIRST_FRAME_WAIT, &out.stopping) {
            Ok(info) => info,
            Err(why) => {
                tracing::warn!(?why, "camera: no picture after the start");
                self.bubble.detach();
                finish(&shared, running);
                return Err(PipeError::Os(format!("no picture came ({why:?})")));
            }
        };
        // The first frame went out before the session heard of this start: the next one opens the run.
        shared.key_wanted.store(true, Ordering::Release);
        (self.shared, self.running) = (shared, Some(running));
        Ok(info)
    }

    fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
        if !paused {
            self.shared.key_wanted.store(true, Ordering::Release);
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
        let (copies, wall_us, cpu_us) = self.shared.copy_cost();
        tracing::info!(copies, wall_us, cpu_us, stats = ?self.stats(), "camera: stopped");
        self.bubble.detach();
        finish(&self.shared, running);
    }

    fn stats(&self) -> VideoStats {
        let mut stats = self.shared.stats();
        let blanks = self.suspension.blanks(host_ns());
        (stats.delivered, stats.blank) = (stats.delivered + blanks, stats.blank + blanks);
        stats
    }
}

impl Drop for MacCameraPipe {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The session stopped (its notices first), then every queued frame encoded and the encoder finished.
fn finish(shared: &Shared, running: Running) {
    shared.stopped.store(true, Ordering::Release);
    let Running {
        _notices,
        session,
        to_encoder,
        encoder,
    } = running;
    drop(_notices);
    drop(session);
    finish_thread(shared, &to_encoder, encoder);
}

fn finish_thread(shared: &Shared, to_encoder: &SyncSender<ToEncoder>, encoder: JoinHandle<()>) {
    shared.stopped.store(true, Ordering::Release);
    let _ = to_encoder.send(ToEncoder::Stop);
    if encoder.join().is_err() {
        tracing::error!("camera: the encoder's thread panicked");
    }
}
