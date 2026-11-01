//! The VideoToolbox encoder with a pinned engine: FFmpeg's VideoToolbox encoders
//! can't choose between the T2's `ave` and Quick Sync's `gva`, so a take's session names its encoder by id and reads
//! back which one it got. Its settings are Apple's for live capture:
//! - `RealTime` and `ExpectedFrameRate`, so the encoder keeps up rather than runs ahead;
//! - no frame reordering (no B-frames: decode order is presentation order, as our files assume);
//! - a keyframe every second (`MaxKeyFrameIntervalDuration` 1 s and `MaxKeyFrameInterval` = fps, so VideoToolbox can't
//!   add an early one), for the editor's scrub budget;
//! - the average bitrate, with a peak of 1.5× over one second (`DataRateLimits`), never CBR;
//! - H.264 High or HEVC Main, automatic level; BT.709 colour tags in the stream (what ScreenCaptureKit is set to).
//!
//! It takes IOSurface NV12 buffers (`SurfacePool`'s), zero-copy into the hardware; packets come out through
//! `output`. A forced keyframe (`encode(…, true)` or the packet gate's ask) is a reused frame-properties dictionary.

mod output;

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;

use cidre::{arc, cf, cm, cv, os, vt};
use klipolo_platform::clock::host_ns;
use vt::compression::keys;

use self::output::{Shared, output};
use super::encoders::{guarded, pinned, video_codec};
use super::pool::nv12_attrs;
use crate::media::{Codec, EncodedPacket, StreamInfo};
use crate::pipe::{PipeError, PipeEvent};

/// What a session encodes: the encoder pinned by its VideoToolbox id, the codec, size, rate and average bitrate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderSpec {
    pub encoder_id: String,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u64,
}

/// The peak over one second, as a multiple of the average bitrate (Cap's ratio).
const PEAK: f64 = 1.5;

pub struct VtEncoder {
    session: arc::R<vt::CompressionSession>,
    shared: Arc<Shared>,
    force_key: arc::R<cf::DictionaryOf<cf::String, cf::Type>>,
    actual_id: Option<String>,
    finished: bool,
}

// SAFETY: a compression session may be used from any thread, one at a time (`&mut self`); its callback reaches only
// `Shared`, which is `Sync`.
unsafe impl Send for VtEncoder {}

impl VtEncoder {
    /// A session on `spec.encoder_id`, its packets to `out` (through a `PacketGate`), a failure to `events`. Busy
    /// (−12915) or missing (−12908) encoders, and any other refusal, are `PipeError::Encoder` with VideoToolbox's code.
    pub fn new(
        spec: &EncoderSpec,
        out: SyncSender<EncodedPacket>,
        events: SyncSender<PipeEvent>,
    ) -> Result<Self, PipeError> {
        let refused = |e: os::Error| PipeError::Encoder(e.0.get());
        let attrs = || PipeError::Os("the encoder's attributes".into());
        let pin = pinned(&spec.encoder_id).ok_or_else(attrs)?;
        let source = nv12_attrs(spec.width, spec.height).ok_or_else(attrs)?;
        let shared = Arc::new(Shared::new(spec.codec, out, events));
        let made = guarded("VTCompressionSessionCreate", || {
            vt::CompressionSession::new::<Shared>(
                spec.width,
                spec.height,
                video_codec(spec.codec),
                Some(&pin),
                Some(&source),
                None,
                Some(output),
                Arc::as_ptr(&shared).cast_mut(),
            )
        });
        let mut session = made.map_err(|e| {
            tracing::warn!(
                id = spec.encoder_id,
                status = e.0.get(),
                "encode: no session"
            );
            refused(e)
        })?;
        let ready =
            guarded("the live settings", || live_settings(&mut session, spec)).and_then(|()| {
                guarded("VTCompressionSessionPrepareToEncodeFrames", || {
                    session.prepare()
                })
            });
        if let Err(e) = ready {
            tracing::warn!(
                id = spec.encoder_id,
                status = e.0.get(),
                "encode: the session isn't ready"
            );
            session.invalidate();
            return Err(refused(e));
        }
        let actual_id = (session.prop(keys::encoder_id()).ok().flatten())
            .and_then(|id| id.try_as_string().map(|s| s.to_string()));
        if actual_id.as_deref() != Some(spec.encoder_id.as_str()) {
            tracing::warn!(asked = spec.encoder_id, got = ?actual_id, "encode: VideoToolbox gave another encoder");
        }
        let force_key = cf::DictionaryOf::with_keys_values(
            &[vt::compression::properties::frame_keys::force_key_frame()],
            &[cf::Boolean::value_true().as_type_ref()],
        );
        Ok(Self {
            session,
            shared,
            force_key,
            actual_id,
            finished: false,
        })
    }

    /// The stream's description, once the first frame came out: the `avcC`/`hvcC` from its format description.
    pub fn info(&self) -> Option<StreamInfo> {
        self.shared.info()
    }

    /// Encodes `frame` (an IOSurface NV12 buffer) captured at `host_ns`; `keyframe` forces one, as does the packet
    /// gate after a drop. Never blocks on the packet channel; VideoToolbox may hold the call while its queue is full.
    pub fn encode(
        &mut self,
        frame: &cv::PixelBuf,
        host_ns_at: u64,
        keyframe: bool,
    ) -> Result<(), PipeError> {
        if self.finished || self.shared.closed.load(Ordering::Acquire) {
            return Err(PipeError::Encoder(vt::errors::INVALID_SESSION.0.get()));
        }
        let key = keyframe | self.shared.force_key.swap(false, Ordering::AcqRel);
        let pts = cm::Time::new(host_ns_at as i64, 1_000_000_000);
        let submitted = host_ns() as *mut c_void;
        let props = key.then_some(&*self.force_key);
        self.session
            .encode_frame(
                frame,
                pts,
                cm::Time::invalid(),
                props,
                submitted,
                std::ptr::null_mut(),
            )
            .map_err(|e| {
                self.shared.dropped_encoder.fetch_add(1, Ordering::Relaxed);
                self.shared.fail(e.0.get());
                PipeError::Encoder(e.0.get())
            })
    }

    /// The presentation time (host ns) of the last keyframe that came out, if one has.
    pub fn last_keyframe_ns(&self) -> Option<u64> {
        Some(self.shared.last_key_ns.load(Ordering::Acquire)).filter(|&ns| ns > 0)
    }

    /// The session's packet receiver is gone (the take ended): nothing more is encoded.
    pub fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// The encoder VideoToolbox gave, read back (`kVTCompressionPropertyKey_EncoderID`).
    pub fn actual_encoder_id(&self) -> Option<String> {
        self.actual_id.clone()
    }

    /// Every frame given out, then the session ended; later `encode`s fail. Idempotent.
    pub fn finish(&mut self) {
        if std::mem::replace(&mut self.finished, true) {
            return;
        }
        if let Err(e) = self.session.complete_all() {
            tracing::warn!(
                status = e.0.get(),
                "encode: the last frames didn't all come out"
            );
        }
        self.session.invalidate();
    }

    /// The 99th percentile of the last 1,000 frames' latency from `encode` to the packet, in ms.
    pub fn latency_p99_ms(&self) -> f32 {
        self.shared.latency_ms(0.99)
    }

    pub(super) fn latency_ms(&self, q: f32) -> f32 {
        self.shared.latency_ms(q)
    }

    /// Frames VideoToolbox dropped or failed, and packets the full channel dropped (`VideoStats`).
    pub fn dropped(&self) -> (u64, u64) {
        let encoder = self.shared.dropped_encoder.load(Ordering::Relaxed);
        (encoder, self.shared.dropped_channel())
    }

    /// The probe's flat-out run: `RealTime` off, so the encoder runs as fast as it can rather than at the frame rate
    /// (measured so).
    pub(super) fn flat_out(&mut self) {
        let no = cf::Boolean::value_false().as_type_ref();
        if let Err(e) = self.session.set_prop(keys::real_time(), Some(no)) {
            tracing::warn!(
                status = e.0.get(),
                "encode: RealTime stayed on for the flat-out run"
            );
        }
    }

    /// (packets out, first's host ns, last's host ns): the probe's achieved rate.
    pub(super) fn meter(&self) -> (u64, Option<u64>, u64) {
        self.shared.meter()
    }
}

impl Drop for VtEncoder {
    /// The session ends before `Shared`, so no callback outlives it.
    fn drop(&mut self) {
        self.finish();
    }
}

/// Apple's live-capture settings (the module doc). The rate, keyframe and reordering rules must hold; the profile and
/// colour tags are asked for and only logged if an encoder refuses them.
fn live_settings(session: &mut vt::CompressionSession, spec: &EncoderSpec) -> os::Result {
    let fps = cf::Number::from_i64(spec.fps.into());
    let bitrate = cf::Number::from_i64(spec.bitrate_bps as i64);
    let one_second = cf::Number::from_f64(1.0);
    let peak_bytes = cf::Number::from_f64(spec.bitrate_bps as f64 * PEAK / 8.0);
    let limits = cf::ArrayOf::<cf::Number>::from_slice(&[&peak_bytes, &one_second]);
    let yes = cf::Boolean::value_true().as_type_ref();
    let no = cf::Boolean::value_false().as_type_ref();
    let must = [
        (keys::real_time(), yes),
        (keys::expected_frame_rate(), fps.as_type_ref()),
        (keys::allow_frame_reordering(), no),
        (keys::max_key_frame_interval(), fps.as_type_ref()),
        (
            keys::max_key_frame_interval_duration(),
            one_second.as_type_ref(),
        ),
        (keys::avarage_bit_rate(), bitrate.as_type_ref()),
    ];
    for (key, value) in must {
        if let Err(e) = session.set_prop(key, Some(value)) {
            tracing::warn!(key = ?key, status = e.0.get(), "encode: a setting live capture needs was refused");
            return Err(e);
        }
    }
    let profile = match spec.codec {
        Codec::H264 => vt::compression::profile_level::h264::high_auto_lvl(),
        Codec::Hevc => vt::compression::profile_level::hevc::main_auto_lvl(),
    };
    let bt709 = [
        (
            keys::color_primaries(),
            cv::image_buf_attach::color_primaries::itu_r_709_2(),
        ),
        (
            keys::transfer_fn(),
            cv::image_buf_attach::transfer_fn::itu_r_709_2(),
        ),
        (
            keys::ycbcr_matrix(),
            cv::image_buf_attach::ycbcr_matrix::itu_r_709_2(),
        ),
    ];
    let wanted = [
        (keys::data_rate_limits(), limits.as_type_ref()),
        (keys::profile_lvl(), profile.as_type_ref()),
    ];
    for (key, value) in wanted
        .into_iter()
        .chain(bt709.map(|(k, v)| (k, v.as_type_ref())))
    {
        if let Err(e) = session.set_prop(key, Some(value)) {
            tracing::warn!(key = ?key, status = e.0.get(), "encode: a setting was refused");
        }
    }
    Ok(())
}
