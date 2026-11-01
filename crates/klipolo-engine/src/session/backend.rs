use std::future::Future;
use std::sync::Arc;

use klipolo_model::time::Us;
use tokio::sync::Notify;

use crate::audio::pcm::PcmStore;
use crate::audio::plan::AudioPlan;
use crate::frames::buffer::FrameBytes;
use crate::media::{Hold, MediaSink, Want};
use crate::plan::FramePlan;
use crate::present::{NativeFrame, Output};
use crate::render::readback::PendingPixels;
use crate::vendor::cap::rendering::decoder::Superseded;

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub plan: FramePlan,
    pub width: u32,
    pub height: u32,
    /// RGBA or NV12 pixels back in memory, or drawn into the native presenter's slot (nothing read back).
    pub output: Output,
}

impl RenderRequest {
    /// An RGBA frame read back to memory (export, tests).
    pub fn rgba(plan: FramePlan, width: u32, height: u32) -> Self {
        Self {
            plan,
            width,
            height,
            output: Output::Rgba,
        }
    }
}

/// A finished frame: `stride × height` bytes of RGBA in a pooled buffer with room for the trailer.
pub struct RenderedRgba {
    pub bytes: FrameBytes,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// Problems to tell the user about once; the frame is still drawn without the failed layer.
    pub warnings: Vec<String>,
    /// Sources that aren't there, by their path (`DecodedSet::missing`): said once each, as an editing copy's row
    /// says it (`api::MEDIA_MISSING`).
    pub missing: Vec<Arc<str>>,
    /// Sources whose disk didn't answer in time, by their path (`DecodedSet::not_answering`): drawn without, and said
    /// only once one stays silent (`api::WAITING_FOR_DISK`).
    pub not_answering: Vec<Arc<str>>,
}

/// What `render` returns as soon as the frame is submitted: its size and warnings now, the pixels once the GPU is
/// done (`Pixels::wait`). The actor publishes them when they arrive instead of waiting, so it keeps taking seeks
/// and edits while the GPU draws.
pub struct Rendered {
    pub pixels: Pixels,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// The pixels' layout (`frames::trailer::FORMAT_RGBA8` or `FORMAT_NV12`).
    pub format: u16,
    pub warnings: Vec<String>,
    /// Sources that aren't there, by their path (`RenderedRgba::missing`).
    pub missing: Vec<Arc<str>>,
    /// Sources whose disk didn't answer in time (`RenderedRgba::not_answering`).
    pub not_answering: Vec<Arc<str>>,
}

pub enum Pixels {
    /// Already in memory (the blank backend). Taken by the first `wait`.
    Ready(Option<FrameBytes>),
    /// On its way back from the GPU.
    Gpu(PendingPixels),
    /// Drawn into a native presenter's slot texture: there are no pixels to wait for (the presenter's copy is
    /// ordered after the drawing on the GPU queue).
    Native(Option<NativeFrame>),
}

impl Pixels {
    /// The frame's pixels. Cancel-safe; don't call it again once it has returned.
    pub async fn wait(&mut self) -> Result<FrameBytes, String> {
        match self {
            Self::Ready(bytes) => bytes
                .take()
                .ok_or_else(|| "The frame was already sent.".into()),
            Self::Gpu(pending) => pending.wait().await,
            Self::Native(_) => Err("The frame stays on the GPU.".into()),
        }
    }

    /// The frame to present (pixels in memory, or the native slot). Cancel-safe.
    pub async fn frame(&mut self) -> Result<crate::present::Frame, String> {
        match self {
            Self::Native(frame) => frame
                .take()
                .map(crate::present::Frame::Native)
                .ok_or_else(|| "The frame was already sent.".into()),
            _ => self.wait().await.map(crate::present::Frame::Bytes),
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(Some(_)) | Self::Native(Some(_)))
    }
}

impl Rendered {
    /// Waits for the pixels (export, tests).
    pub async fn finish(mut self) -> Result<RenderedRgba, String> {
        let bytes = self.pixels.wait().await?;
        Ok(RenderedRgba {
            bytes,
            width: self.width,
            height: self.height,
            stride: self.stride,
            warnings: self.warnings,
            missing: self.missing,
            not_answering: self.not_answering,
        })
    }
}

/// Where the preview's sound is. Audio is the master clock: the picture follows the samples the device has
/// played, so it never drifts from the sound.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioTime {
    /// No sound is playing (a silent project, or a backend without sound): the host clock runs the picture.
    Off,
    /// The device is opening or moving to a new position: hold the picture there.
    Waiting,
    /// The timeline time heard now. Until the new position is heard it is earlier than that position.
    At(Us),
    /// The device failed: play on without sound.
    Failed(String),
}

/// What the actor needs from a renderer. The GPU backend and `BlankBackend` implement it.
pub trait PreviewBackend: Send + 'static {
    /// Draws `req` and submits it; the pixels follow (`Rendered::pixels`). The GPU backend keeps at most two frames
    /// on the GPU however long their pixels are held. Export renders with `GpuBackend::for_export()` (full-size
    /// decode, waits for images, LUTs and fonts) and takes each frame's pixels with `Rendered::finish`.
    fn render(
        &mut self,
        req: RenderRequest,
    ) -> impl Future<Output = Result<Rendered, String>> + Send;
    /// Forget decoders, caches and per-project GPU textures: the project was closed or switched. The engine stays
    /// alive for the app's lifetime, so with no project open this is all it holds on to.
    fn reset(&mut self);
    /// The window was hidden: let decoders and caches go (the next render reopens them).
    fn release(&mut self) {
        self.reset();
    }
    /// Play `plan` from `from_us` (if sound is already playing, move it there without reopening the device). Output
    /// only, never an input device, and only once the user presses Play; tests never open a device. Never waits on
    /// the device: true when sound is on its way (follow `audio_time`), false when the picture runs on its own.
    fn start_audio(&mut self, _plan: Arc<AudioPlan>, _from_us: Us) -> bool {
        false
    }
    /// An edit during playback: keep playing from where the sound is, with the new plan.
    fn update_audio(&mut self, _plan: Arc<AudioPlan>, _at_us: Us) {}
    /// Pause the sound (the device may stay open a little while, so playing again starts at once).
    fn stop_audio(&mut self) {}
    fn audio_time(&self) -> AudioTime {
        AudioTime::Off
    }
    /// Notified when `audio_time` leaves `Waiting` (the sound is mapped onto the clock) or the device fails.
    fn audio_wake(&self) -> Option<Arc<Notify>> {
        None
    }
    /// Decode the plan's audio into the PCM cache in the background (sources under `at_us` first), so Play can
    /// start at once.
    fn prepare_audio(&mut self, _plan: Arc<AudioPlan>, _at_us: Us) {}
    /// The loudness target's gain on the whole mix (1.0: none).
    fn set_master_gain(&mut self, _gain: f32) {}
    /// The person's listening volume on top of it, 0 (silent) to 1: the preview's alone, never an export's.
    fn set_volume(&mut self, _volume: f32) {}
    /// The PCM cache, for measuring loudness (None: the backend has no sound).
    fn pcm(&self) -> Option<Arc<PcmStore>> {
        None
    }
    /// The project closed: forget its decoded audio (cached files stay for the next opening; the cache's tidy evicts
    /// by least recent use).
    fn close_audio(&mut self) {}
    /// Cut thumbnails for the project's video files in the background (`media::wants`: the one under the playhead
    /// first).
    fn prepare_media(&mut self, _wants: Vec<Want>) {}
    /// Where "a file's peaks or thumbnails are ready" goes.
    fn set_media_sink(&mut self, _sink: MediaSink) {}
    /// Hold (`on`) or release thumbnail scans for `why`: they run only while nothing holds them.
    fn hold_media(&mut self, _why: Hold, _on: bool) {}
    /// The screen activity pass (after the thumbnails); None without a media cache.
    fn activity(&self) -> Option<Arc<crate::media::activity::ActivityStore>> {
        None
    }
    /// Get ready for a frame shown soon (open decoders, load images and LUTs) without drawing it.
    fn warm(&mut self, _plan: &FramePlan) {}
    /// Says, while a still is drawn, whether a newer seek replaced it (the actor's seek): its video reads then stop
    /// at the nearest frame read so far where the GOP lasts at most a second.
    fn set_superseded(&mut self, _superseded: Superseded) {}
    /// The session's cursor takes: a shape whose image the backend can't decode is marked on its take
    /// (`CursorTracks::refuse_shape`), so the preview plans the Arrow look there, as export does.
    fn set_cursors(&mut self, _cursors: Arc<crate::media::cursor::CursorTracks>) {}
    /// Forget what `warm` started: playback stopped or jumped, so those starts won't come.
    fn cancel_warm(&mut self) {}
    /// Notified when something the last frames drew without (an image, a LUT, the system fonts) finished loading in
    /// the background, so the still is drawn again.
    fn redraw(&self) -> Option<Arc<Notify>> {
        None
    }
    /// Starts presenting into the app's native view (`platform::NativeSurface`); frames rendered with
    /// `Output::Native` go to its slots. A backend without a GPU can't, and the engine keeps the socket.
    fn attach_native(
        &mut self,
        _surface: crate::platform::NativeSurface,
        _shown: crate::present::native::ShownSink,
    ) -> Result<crate::present::NativeLayerPresenter, String> {
        Err("This preview can't draw into a native view.".into())
    }
    /// The native presenter was dropped: let its slots go.
    fn detach_native(&mut self) {}
    /// What the preview draws with from now on (the project feed's registry): its agent-made shaders are
    /// compiled now, as they are placed, and only its pipelines are kept.
    fn prepare_registry(&mut self, _registry: &Arc<crate::registry::Registry>) {}
    /// The app wrote `contrib`'s crash-guard mark `seq` to disk: what it covers may go ahead (a compile, or drawing).
    fn mark_written(&mut self, _contrib: &klipolo_model::contrib::ContribId, _seq: u64) {}
    /// What the app hears about agent-made effects since the last call (`EngineEvent::{FirstDraw, DrewSafely,
    /// EffectDegraded, GpuLost}`).
    fn take_guard_events(&mut self) -> Vec<crate::render::effects::GuardEvent> {
        Vec::new()
    }
    /// When an open crash-guard mark next runs out of time (`guard_tick` then closes it); None: none does.
    fn guard_deadline(&self) -> Option<std::time::Instant> {
        None
    }
    /// Closes the crash-guard marks whose time ran out.
    fn guard_tick(&mut self) {}
    /// Notified when the GPU finished a frame a crash-guard mark waits for (`guard_tick` then starts its time).
    fn guard_wake(&self) -> Option<Arc<Notify>> {
        None
    }
}
