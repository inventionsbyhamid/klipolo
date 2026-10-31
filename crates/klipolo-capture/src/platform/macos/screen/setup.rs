//! A screen pipe's encoder for the take: the pinned one the take names, else this Mac's safe pick, at the
//! preset's size fitted to the captured area; the stream's frame interval and settings; a stream's start error as the
//! pipe's; the cursor's live switch into the frames; and a started stream's release.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::Duration;

use cidre::{cg, cm, cv, sc};

use super::super::encode::{EncoderSpec, VtEncoder};
use super::super::encoders::hardware_encoders;
use super::super::handoff::{Shared, ToEncoder};
use crate::backend::{EncoderPick, PresetId, VideoSpec};
use crate::media::Codec;
use crate::pipe::{PipeError, VideoOut};
use crate::platform::common::sck::{USER_DECLINED, fit_size, frame_interval_parts, queue_depth};
use crate::platform::common::vt_ids::dedicated_on_mac;
use crate::preflight::cursor::CursorPlan;
use crate::preset::native_preset;
use crate::vendor::cap::sck::{Capturer, StreamCfgBuilder};

/// How long the cursor's live switch waits for the stream's acknowledgement.
const SHOW_CURSOR_WAIT: Duration = Duration::from_secs(1);

/// A video pipe's encoder for the take (the screen's, and the camera's), for `pick` (None: this Mac's first hardware
/// H.264, a dedicated engine first), at the preset's size fitted to the source's `pixels`.
pub(in crate::platform::macos) fn encoder(
    preset: PresetId,
    pick: Option<&EncoderPick>,
    pixels: (u32, u32),
    out: &VideoOut,
) -> Result<(VtEncoder, (u32, u32), u32), PipeError> {
    let pick = match pick {
        Some(p) => p.clone(),
        None => own_pick()?,
    };
    let p = native_preset(preset, pick.codec, pixels);
    let size = fit_size((p.width, p.height), pixels);
    let spec = EncoderSpec {
        encoder_id: pick.id,
        codec: pick.codec,
        width: size.0,
        height: size.1,
        fps: p.fps,
        bitrate_bps: p.bitrate_bps,
    };
    let vt = VtEncoder::new(&spec, out.packets.clone(), out.events.clone())?;
    Ok((vt, size, p.fps))
}

/// This Mac's first hardware H.264 encoder, a dedicated engine first: the safe pick when the take names none.
fn own_pick() -> Result<EncoderPick, PipeError> {
    let arch = klipolo_platform::caps::current().os.cpu_arch;
    let mut h264: Vec<_> = (hardware_encoders().into_iter())
        .filter(|e| e.codec == Codec::H264)
        .collect();
    h264.sort_by_key(|e| !dedicated_on_mac(&e.id, &arch));
    let e = h264.into_iter().next().ok_or(PipeError::Unsupported)?;
    Ok(EncoderPick {
        codec: e.codec,
        id: e.id,
    })
}

/// A stream's start error as the pipe's: a refused permission, else the OS's code.
pub(in crate::platform::macos) fn start_error(code: isize) -> PipeError {
    if code == USER_DECLINED {
        PipeError::PermissionDenied
    } else {
        PipeError::Os(format!("ScreenCaptureKit {code}"))
    }
}

/// The stream's minimum frame interval for a take at `fps`: 1 / (fps × 1.1) (`frame_interval_parts`).
pub fn frame_interval(fps: u32) -> cm::Time {
    let (value, scale) = frame_interval_parts(fps);
    cm::Time::new(value, scale)
}

/// A started stream, its encoder thread and the hand-off to it.
pub(super) struct Running {
    pub(super) capturer: Capturer,
    pub(super) to_encoder: SyncSender<ToEncoder>,
    pub(super) encoder: JoinHandle<()>,
    /// What its settings were made from: a single window's picture, its size, its frame rate.
    pub(super) settings: (bool, (u32, u32), u32),
}

/// The stream's settings for `spec` (a single `window`'s picture or not) at `size` and `fps`.
pub(super) fn stream_config(
    spec: &VideoSpec,
    window: bool,
    size: (u32, u32),
    fps: u32,
) -> cidre::arc::R<sc::StreamCfg> {
    let mut c = StreamCfgBuilder::default();
    c.set_width(size.0 as usize);
    c.set_height(size.1 as usize);
    c.set_frame_interval(frame_interval(fps));
    c.set_queue_depth(queue_depth(fps));
    c.set_pixel_format(cv::PixelFormat::_420V);
    c.set_color_matrix(cg::DisplayStreamYCbCrMatrix::itu_r_709_2());
    c.set_color_space_name(cg::color_space::names::srgb());
    c.set_shows_cursor(spec.cursor == CursorPlan::Burned);
    c.set_captures_audio(spec.sound);
    if spec.sound {
        c.set_excludes_current_process_audio(true);
        c.set_sample_rate(48_000);
        c.set_channel_count(2);
    }
    c.set_presenter_overlay_alert(sc::PresenterOverlayAlertSetting::Never);
    if window {
        c.set_scales_to_fit(true);
        c.set_preserves_aspect_ratio(true);
    }
    c.build()
}

/// The running stream's settings again for `spec` (its cursor burned in), applied live: the nudge takes them first, so
/// no nudge after this can apply the old ones, then `updateConfiguration:`, waiting up to 1 s for its answer.
pub(super) fn show_cursor(
    spec: &VideoSpec,
    running: &Running,
    shared: &Arc<Shared>,
) -> Result<(), PipeError> {
    let (window, size, fps) = running.settings;
    let cfg = stream_config(spec, window, size, fps);
    let answers = shared.clone();
    let nudge = (running.capturer).nudger(cfg.retained(), move |e| answers.answered(e));
    shared.set_nudge(Some(Box::new(nudge)));
    (running.capturer.update_within(&cfg, SHOW_CURSOR_WAIT)).map_err(|code| match code {
        Some(code) => PipeError::Os(format!(
            "ScreenCaptureKit {code} on the cursor's live switch"
        )),
        None => PipeError::Os("the cursor's live switch wasn't acknowledged within 1 s".into()),
    })
}

/// The stream released, then every queued frame encoded and the session finished.
pub(super) fn finish(shared: &Shared, running: Running) {
    shared.stopped.store(true, Ordering::Release);
    // The nudge holds the stream: it goes with it.
    shared.set_nudge(None);
    let Running {
        capturer,
        to_encoder,
        encoder,
        ..
    } = running;
    drop(capturer);
    let _ = to_encoder.send(ToEncoder::Stop);
    if encoder.join().is_err() {
        tracing::error!("screen: the encoder's thread panicked");
    }
}
