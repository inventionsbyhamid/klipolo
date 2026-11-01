//! The GPU preview backend: decoder pool → compositor → RGBA or NV12 readback, or a native presenter's slot, plus the
//! preview's sound and the timeline's thumbnails (`media`).

mod backend_guards;
pub(crate) mod build_times;
pub mod captions;
pub mod compositor;
pub mod cursor;
pub mod device;
pub mod effects;
pub mod export;
pub mod font_fallback;
pub mod fonts;
pub mod image_load;
pub mod look;
pub mod lut;
pub mod mips;
pub mod mode;
pub mod nv12;
pub mod nv12_scale;
pub(crate) mod outline_glyphs;
pub mod pool;
pub mod probe;
pub mod readback;
pub mod scene;
#[cfg(any(test, feature = "test-support"))]
mod scene_hooks;
pub mod script_runs;
pub mod shared_fonts;
pub mod text;
pub mod text_timing;
mod tuning;
pub mod uniforms;
pub mod video;
pub mod warm;
mod worker_mode;
pub mod yuv;

pub use klipolo_platform::pressure;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Notify, oneshot};

use compositor::Compositor;
use device::{RenderDevice, WorkDone};
use effects::GuardEvent;
use klipolo_model::contrib::ContribId;
use klipolo_model::time::Us;
use mode::RenderMode;
use pool::{DecodedSet, DecoderPool};

use crate::audio::output::{AudioOutput, MasterGain};
use crate::audio::pcm::PcmStore;
use crate::audio::plan::AudioPlan;
use crate::decode_wait::DecodeBeat;
use crate::media::{Hold, MediaSink, ThumbStore, Want};
use crate::plan::FramePlan;
use crate::present::native::Slots;
use crate::registry::Registry;
use crate::session::{AudioTime, PreviewBackend, RenderRequest, Rendered};
pub use tuning::{EXPORT_TUNING, PREVIEW_TUNING, export_tuning};

pub struct GpuBackend {
    dev: Arc<RenderDevice>,
    /// None while the GPU warm-up is still making it (`coming`): the engine has started, and its first frame's
    /// decoders open meanwhile.
    compositor: Option<Compositor>,
    coming: Option<oneshot::Receiver<Compositor>>,
    pool: DecoderPool,
    /// The decoded frames of the last frames drawn, reused in turn so decoding allocates no maps. A frame's set is
    /// kept until two more frames were submitted: the GPU may still be reading its IOSurfaces (the actor keeps at
    /// most two frames in flight, and the decoders' pools may recycle a buffer once nobody holds it).
    decoded: [DecodedSet; 3],
    next_set: usize,
    /// When each set's frame finished on the GPU: at most two frames are on the GPU at once, whatever the caller
    /// does with their pixels, so a set (and the IOSurfaces it holds) is never reused while the GPU reads it.
    work: [Arc<WorkDone>; 3],
    pcm: Arc<PcmStore>,
    /// Thumbnail sprites (None until `with_media_cache`).
    thumbs: Option<Arc<ThumbStore>>,
    /// Opened on Play; a pause keeps it for a grace period; hide and close release it.
    audio: Option<AudioOutput>,
    /// The newest `start_audio`, and whether it is sounding (a silent plan opens nothing).
    request: u64,
    sounding: bool,
    /// The loudness target's gain times the listening volume, read by the mixer every block.
    gain: MasterGain,
    /// Notified when the sound is mapped onto the clock or the device fails.
    audio_wake: Arc<Notify>,
    /// Fired when an image, LUT or the font scan finished loading in the background (the actor redraws the still).
    redraw: Arc<Notify>,
    wait_for_assets: bool,
    /// Test hooks' settings (`set_downsample`, `set_upright`), kept for a compositor made later.
    downsample: bool,
    upright: bool,
    /// The native presenter's slots (frames rendered with `Output::Native` go there) and its failure flag.
    native: Option<(Arc<Slots>, Arc<AtomicBool>)>,
    /// Whether the still being drawn was replaced by a newer seek (`PreviewBackend::set_superseded`).
    superseded: Option<crate::vendor::cap::rendering::decoder::Superseded>,
    /// The cursor takes frames draw: a shape the cursor pass refuses is marked on them (`refuse_cursor_shapes`).
    cursors: Option<Arc<crate::media::cursor::CursorTracks>>,
    /// How its compositors treat agent-made shaders: kept, so a compositor it makes again (the
    /// warm-up's fallback, after a device loss) has the same mode.
    mode: RenderMode,
    /// An export's: checked passes predicted slow draw in tiles (kept for a compositor made later, as `downsample`).
    tiled: bool,
    /// The registry the preview draws with (`prepare_registry`), given again to a compositor made later.
    registry: Option<Arc<Registry>>,
    /// Guard events of a compositor that went (a device loss).
    events: Vec<GuardEvent>,
    /// Its compositor came from the GPU warm-up (tests).
    warmed: bool,
    /// Fired when the GPU finished a frame a crash-guard mark waits for.
    guard_wake: Arc<Notify>,
    /// Work-done callbacks registered for the crash guard so far (tests).
    done_callbacks: u64,
}

impl GpuBackend {
    /// A backend on a new device, its compositors in `mode` (no default).
    pub async fn new(mode: RenderMode) -> Result<Self, String> {
        Ok(Self::with_device(RenderDevice::new().await?, mode))
    }

    /// A backend on `dev`, its compositors in `mode`: the compositor and the pipelines a first frame needs are made
    /// here; the rest the first time a frame needs them.
    pub(crate) fn with_device(dev: Arc<RenderDevice>, mode: RenderMode) -> Self {
        let redraw = Arc::new(Notify::new());
        let compositor = Compositor::new(dev.clone(), redraw.clone(), mode);
        Self::assemble(dev, redraw, Some(compositor), None, mode)
    }

    /// A backend on `dev` whose compositor the GPU warm-up is still making in `mode` (`coming`, drawing with `redraw`):
    /// the engine starts on it at once, and its first frame opens its decoders while the pipelines compile; `render`
    /// takes the compositor before it draws.
    pub(crate) fn coming(
        dev: Arc<RenderDevice>,
        redraw: Arc<Notify>,
        coming: oneshot::Receiver<Compositor>,
        mode: RenderMode,
    ) -> Self {
        Self::assemble(dev, redraw, None, Some(coming), mode)
    }

    fn assemble(
        dev: Arc<RenderDevice>,
        redraw: Arc<Notify>,
        compositor: Option<Compositor>,
        coming: Option<oneshot::Receiver<Compositor>>,
        mode: RenderMode,
    ) -> Self {
        Self {
            compositor,
            coming,
            dev,
            pool: {
                // A file whose disk was slow to answer redraws the still once it does.
                let mut pool = DecoderPool::new(PREVIEW_TUNING, false);
                pool.set_redraw(redraw.clone());
                pool
            },
            decoded: Default::default(),
            next_set: 0,
            work: Default::default(),
            pcm: PcmStore::new(std::env::temp_dir().join("klipolo-pcm")),
            thumbs: None,
            audio: None,
            request: 0,
            sounding: false,
            gain: MasterGain::default(),
            audio_wake: Arc::new(Notify::new()),
            redraw,
            wait_for_assets: false,
            downsample: true,
            upright: true,
            native: None,
            superseded: None,
            cursors: None,
            mode,
            tiled: false,
            registry: None,
            events: Vec::new(),
            warmed: false,
            guard_wake: Arc::new(Notify::new()),
            done_callbacks: 0,
        }
    }

    /// The backend export renders with: full-size decode (`EXPORT_TUNING`), and every frame waits for its images,
    /// LUTs and fonts, and a disk that is slow to answer is waited for, up to a minute (`DecoderPool::patient`). Take
    /// each frame's pixels with `Rendered::finish`.
    pub async fn for_export() -> Result<Self, String> {
        Ok(Self::for_export_on(RenderDevice::new().await?))
    }

    /// `for_export` on a device already made (the export tests' `test_support::render_text_via_export`). The export
    /// renderer is `Contained` (agent-made effects draw from frame 0) and draws a slow checked pass in tiles.
    #[doc(hidden)]
    pub fn for_export_on(dev: Arc<RenderDevice>) -> Self {
        let mut backend = Self::with_device(dev, RenderMode::Contained).wait_for_assets();
        backend.pool = DecoderPool::new(export_tuning(), false).patient();
        backend.tiled = true;
        if let Some(compositor) = &mut backend.compositor {
            compositor.set_tiled(true);
        }
        backend
    }

    /// Set while a frame waits on a decoder that is still making progress: export's worker sends its heartbeat from it.
    pub fn decode_beat(&self) -> Arc<DecodeBeat> {
        self.pool.decode_beat()
    }

    /// Every frame waits for its images, LUTs and fonts (export, tests); the preview draws without them meanwhile.
    /// Decoding stays at preview size: export uses `for_export`.
    pub fn wait_for_assets(mut self) -> Self {
        self.wait_for_assets = true;
        if let Some(compositor) = &mut self.compositor {
            compositor.set_wait_for_assets(true);
        }
        self
    }

    /// Test hook: turn built-in half-size passes off to compare against full size (a GPU test). Kept for a
    /// compositor the warm-up hands over later and for one a device reset makes.
    #[doc(hidden)]
    pub fn set_downsample(&mut self, on: bool) {
        self.downsample = on;
        if let Some(compositor) = &mut self.compositor {
            compositor.set_downsample(on);
        }
    }

    /// Test hook: run effects on turned video as stored, without the upright pass, to measure that pass (a GPU
    /// test). Kept like `set_downsample`.
    #[doc(hidden)]
    pub fn set_upright(&mut self, on: bool) {
        self.upright = on;
        if let Some(compositor) = &mut self.compositor {
            compositor.set_upright(on);
        }
    }

    /// The backend's settings on a compositor it didn't make with them (the warm-up's, or a device reset's): its
    /// waits, test hooks, tiles and the preview's registry (its checked shaders compiled again).
    fn configure(&self, compositor: &mut Compositor) {
        compositor.set_wait_for_assets(self.wait_for_assets);
        compositor.set_downsample(self.downsample);
        compositor.set_upright(self.upright);
        compositor.set_tiled(self.tiled);
        if let Some(registry) = &self.registry {
            compositor.prepare_registry(registry);
        }
    }

    /// Compiles every pipeline the preview can need (`Compositor::warm_all`, plus the native presenter's copy), so
    /// no frame waits for the shader compiler. The warm-up runs it on a device of its own (`warm::Warmup`).
    pub fn warm_all(&mut self) {
        if let Some(compositor) = &mut self.compositor {
            compositor.warm_all();
        }
        crate::present::native::warm(&self.dev.device);
    }

    /// Takes the compositor the GPU warm-up is making, waiting for it (or makes one if the warm-up failed).
    async fn compositor_ready(&mut self) {
        if self.compositor.is_some() {
            return;
        }
        let made = match self.coming.take() {
            Some(coming) => coming.await.ok(),
            None => None,
        };
        self.warmed = made.is_some();
        let mut compositor = made
            .unwrap_or_else(|| Compositor::new(self.dev.clone(), self.redraw.clone(), self.mode));
        self.configure(&mut compositor);
        self.compositor = Some(compositor);
    }

    /// Where decoded audio is cached: a store of its own in `dir`.
    pub fn with_audio_cache(self, dir: PathBuf) -> Self {
        self.with_audio_store(PcmStore::new(dir))
    }

    /// The app's sound cache, which transcription shares: one store per folder, so a file is decoded once
    /// and never by two writers, and it outlives an engine that is replaced.
    pub fn with_audio_store(mut self, store: Arc<PcmStore>) -> Self {
        self.pcm = store;
        if let Some(thumbs) = &self.thumbs {
            self.pcm.set_thumbs(thumbs.clone());
        }
        self
    }

    /// Where the timeline's thumbnails go (`thumbs::root_in(dir)`, the app's media cache): scans cut them after the
    /// sound cache's builds (which they wait for), and preview decoders read the keyframe index stored there.
    pub fn with_media_cache(mut self, dir: PathBuf) -> Self {
        let root = crate::media::thumbs::root_in(&dir);
        let thumbs = ThumbStore::new(root.clone(), Arc::new(klipolo_platform::durable::Os));
        self.pcm.set_thumbs(thumbs.clone());
        crate::media::activity::ActivityStore::attach(&thumbs, dir.join("activity"));
        crate::media::keyframes::set_root(Some(root));
        self.thumbs = Some(thumbs);
        self
    }

    /// The caches keep themselves under their budgets while the app runs; unfinished export jobs live in `jobs_root`
    /// (`<exports>/.jobs`, when the exports folder is known) and their files are kept (call after `with_audio_cache`
    /// and `with_media_cache`).
    pub fn with_cache_tidy(self, jobs_root: Option<PathBuf>) -> Self {
        self.pcm.evict(jobs_root.clone());
        if let (Some(thumbs), Some(jobs_root)) = (&self.thumbs, jobs_root) {
            thumbs.set_jobs_root(jobs_root);
        }
        self
    }

    /// The thumbnail store, so the app can hold scans for exports and editing copies.
    pub fn thumbs(&self) -> Option<Arc<ThumbStore>> {
        self.thumbs.clone()
    }

    /// The sound cache playback, waveforms and transcription share, so the app keeps the running engine's (Sync).
    pub fn pcm_store(&self) -> Arc<PcmStore> {
        self.pcm.clone()
    }

    /// Drops what a hidden window or a closed project lets go.
    fn forget(&mut self) {
        // The output thread ends on its own (nobody waits for the device).
        self.audio = None;
        self.sounding = false;
        self.decoded.iter_mut().for_each(|set| set.frames.clear());
        self.pool.clear();
        if let Some(compositor) = &mut self.compositor {
            compositor.forget();
        }
    }

    pub fn max_preview_pixels(&self) -> u32 {
        self.dev.max_preview_pixels()
    }

    pub fn socket_playback_pixels(&self) -> u32 {
        self.dev.socket_playback_pixels()
    }

    pub fn adapter_name(&self) -> String {
        self.dev.adapter_name()
    }

    /// The render passes of the last frame drawn (`Compositor::passes_last_frame`); 0 before the first. For the
    /// motion cost test and the perf harness.
    #[doc(hidden)]
    pub fn passes_last_frame(&self) -> u32 {
        self.compositor
            .as_ref()
            .map_or(0, |c| c.passes_last_frame())
    }

    /// What the scene drawer did (`Compositor::scene_stats`); the default before the first frame.
    #[doc(hidden)]
    pub fn scene_stats(&self) -> scene::SceneStats {
        self.compositor
            .as_ref()
            .map_or_else(Default::default, |c| c.scene_stats())
    }

    /// What the titles and captions cost and hold (`Compositor::text_stats`): for the title and caption tests and the
    /// perf harness's `text` cases.
    #[doc(hidden)]
    pub fn text_stats(&self) -> (text::TitleStats, captions::CaptionStats) {
        self.compositor
            .as_ref()
            .map(Compositor::text_stats)
            .unwrap_or_default()
    }

    /// Times the text drawers' own work (their CPU on the render thread, and their GPU time where the device was made
    /// after `text_timing::ask_for_timestamps` on an adapter with timestamps) from the next frame on, or stops. For the
    /// perf harness.
    #[doc(hidden)]
    pub fn time_text(&mut self, on: bool) {
        if let Some(compositor) = &mut self.compositor {
            compositor.time_text(on);
        }
    }

    /// What the last frame's text drawers cost (`time_text`): call once its pixels are back (it waits for the GPU).
    #[doc(hidden)]
    pub fn text_time(&self) -> text_timing::TextTime {
        self.compositor
            .as_ref()
            .map(Compositor::text_time)
            .unwrap_or_default()
    }

    /// Waits until the GPU has done everything submitted so far (the perf harness times a frame's GPU work with it).
    #[doc(hidden)]
    pub fn wait_for_gpu(&self) {
        let _ = self.dev.device.poll(wgpu::PollType::Wait);
    }

    /// What kind of GPU this is (`RenderDevice::gpu_class`): the engine reports it (`EngineInfo::gpu_class`), and
    /// editing copies follow it.
    pub fn gpu_class(&self) -> crate::platform::gpu::GpuClass {
        self.dev.gpu_class()
    }

    /// The device was lost (driver reset, eGPU unplugged) since it was made.
    pub fn is_lost(&self) -> bool {
        self.dev.is_lost()
    }

    /// A lost device (driver reset, eGPU unplugged) is replaced before the next frame; textures start over.
    async fn ensure_device(&mut self) -> Result<(), String> {
        if !self.dev.is_lost() {
            return Ok(());
        }
        self.lost_compositor();
        let dev = RenderDevice::new().await?;
        let mut compositor = Compositor::new(dev.clone(), self.redraw.clone(), self.mode);
        self.configure(&mut compositor);
        self.compositor = Some(compositor);
        // One the warm-up was making belongs to the lost device.
        self.coming = None;
        self.dev = dev;
        // The native presenter's slots and pipeline belonged to the lost device: the engine falls back to the socket.
        if let Some((_, failed)) = self.native.take() {
            failed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

impl Drop for GpuBackend {
    /// The engine ended (shut down, or its actor stopped mid-playback): a scan parked on a hold lets its file and its
    /// decoder go instead of waiting for a resume that won't come.
    fn drop(&mut self) {
        if let Some(thumbs) = &self.thumbs {
            thumbs.cancel_all();
        }
    }
}

impl PreviewBackend for GpuBackend {
    async fn render(&mut self, req: RenderRequest) -> Result<Rendered, String> {
        self.ensure_device().await?;
        let index = self.next_set;
        self.next_set = (index + 1) % self.decoded.len();
        // The frame two before this one must be off the GPU (then the one three before, whose set this frame reuses,
        // is too: the GPU finishes in order). This doesn't wait for anyone to take pixels, only for the GPU.
        self.work[(index + 1) % self.work.len()].wait().await;
        self.pool
            .decode_unless(
                &req.plan,
                &mut self.decoded[index],
                self.superseded.as_ref(),
            )
            .await;
        // The first frame's pipelines may still be on their way from the GPU warm-up: its decoders opened meanwhile.
        self.compositor_ready().await;
        let set = &self.decoded[index];
        let native = self.native.as_ref().map(|(slots, _)| slots);
        let rendered = self
            .compositor
            .as_mut()
            .expect("made by compositor_ready")
            .render(&req.plan, set, (req.width, req.height), req.output, native)
            .await;
        if rendered.is_ok() {
            // The preview's frames carry their stage number to the crash guard.
            let frame = match (&self.compositor, self.mode) {
                (Some(c), RenderMode::Interactive) => Some(c.gpu_done()),
                _ => None,
            };
            self.work[index].arm_frame(&self.dev, frame);
            if self.mode == RenderMode::Interactive {
                self.signal_done();
            }
        }
        self.refuse_cursor_shapes();
        rendered
    }

    fn set_superseded(&mut self, superseded: crate::vendor::cap::rendering::decoder::Superseded) {
        self.superseded = Some(superseded);
    }

    fn set_cursors(&mut self, cursors: Arc<crate::media::cursor::CursorTracks>) {
        self.cursors = Some(cursors);
    }

    fn reset(&mut self) {
        self.forget();
        if let Some(compositor) = &mut self.compositor {
            compositor.project_closed();
        }
        // The project closed or switched: its scans stop (finished chunks stay), and the next project tries the disk
        // again. A reset engine isn't playing.
        if let Some(thumbs) = &self.thumbs {
            thumbs.cancel_all();
            thumbs.reset_disk_full();
            thumbs.resume(Hold::Playback);
        }
    }

    fn release(&mut self) {
        // Hidden: decoders and the sound device go, but thumbnail scans go on (they wait while nothing plays anyway).
        self.forget();
    }

    fn start_audio(&mut self, plan: Arc<AudioPlan>, from_us: Us) -> bool {
        self.pcm.prepare(&plan, from_us);
        if plan.sources.is_empty() {
            self.stop_audio();
            return false;
        }
        let open =
            |b: &Self| AudioOutput::open(b.pcm.clone(), b.gain.shared(), b.audio_wake.clone());
        if !self.audio.as_ref().is_some_and(AudioOutput::is_alive) {
            self.audio = Some(open(self));
        }
        self.request += 1;
        let played = self
            .audio
            .as_ref()
            .is_some_and(|out| out.play(plan.clone(), from_us, self.request));
        if !played {
            // It closed as its grace period ran out: a fresh output takes the play.
            let out = open(self);
            out.play(plan, from_us, self.request);
            self.audio = Some(out);
        }
        self.sounding = true;
        true
    }

    fn update_audio(&mut self, plan: Arc<AudioPlan>, at_us: Us) {
        self.pcm.prepare(&plan, at_us);
        if let Some(out) = self.audio.as_ref().filter(|_| self.sounding) {
            out.set_plan(plan);
        }
    }

    fn stop_audio(&mut self) {
        self.sounding = false;
        match &self.audio {
            Some(out) if out.is_alive() => out.pause(),
            _ => self.audio = None,
        }
    }

    fn audio_time(&self) -> AudioTime {
        let Some(out) = self.audio.as_ref().filter(|_| self.sounding) else {
            return AudioTime::Off;
        };
        if let Some(message) = out.failure() {
            return AudioTime::Failed(message);
        }
        out.time_us(self.request)
            .map_or(AudioTime::Waiting, AudioTime::At)
    }

    fn audio_wake(&self) -> Option<Arc<Notify>> {
        Some(self.audio_wake.clone())
    }

    fn prepare_audio(&mut self, plan: Arc<AudioPlan>, at_us: Us) {
        self.pcm.prepare(&plan, at_us);
    }

    fn set_master_gain(&mut self, gain: f32) {
        self.gain.set_loudness(gain);
    }

    fn set_volume(&mut self, volume: f32) {
        self.gain.set_volume(volume);
    }

    fn pcm(&self) -> Option<Arc<PcmStore>> {
        Some(self.pcm.clone())
    }

    fn close_audio(&mut self) {
        self.audio = None;
        self.sounding = false;
        self.pcm.close_project();
    }

    fn prepare_media(&mut self, wants: Vec<Want>) {
        if let Some(thumbs) = &self.thumbs {
            thumbs.want(wants);
        }
    }

    fn set_media_sink(&mut self, sink: MediaSink) {
        self.pcm.set_sink(sink.clone());
        // An editing copy that can't be opened is said through it (the pool falls back to the original).
        self.pool.set_media_sink(sink.clone());
        if let Some(thumbs) = &self.thumbs {
            thumbs.set_sink(sink);
        }
    }

    fn activity(&self) -> Option<Arc<crate::media::activity::ActivityStore>> {
        self.thumbs.as_ref()?.activity()
    }

    fn hold_media(&mut self, why: Hold, on: bool) {
        if let Some(thumbs) = &self.thumbs {
            if on {
                thumbs.pause(why);
            } else {
                thumbs.resume(why);
            }
        }
    }

    fn warm(&mut self, plan: &FramePlan) {
        self.pool.warm(plan);
        if let Some(compositor) = &mut self.compositor {
            compositor.warm(plan);
        }
    }

    fn cancel_warm(&mut self) {
        self.pool.cancel_warming();
    }

    fn redraw(&self) -> Option<Arc<Notify>> {
        Some(self.redraw.clone())
    }

    fn attach_native(
        &mut self,
        surface: crate::platform::NativeSurface,
        shown: crate::present::native::ShownSink,
    ) -> Result<crate::present::NativeLayerPresenter, String> {
        let presenter =
            crate::present::NativeLayerPresenter::start(self.dev.clone(), surface, shown)?;
        self.native = Some((presenter.slots(), presenter.fail_flag()));
        Ok(presenter)
    }

    fn detach_native(&mut self) {
        self.native = None;
    }

    fn prepare_registry(&mut self, registry: &Arc<Registry>) {
        self.registry = Some(registry.clone());
        if let Some(compositor) = &mut self.compositor {
            compositor.prepare_registry(registry);
        }
    }

    fn mark_written(&mut self, contrib: &ContribId, seq: u64) {
        if let Some(compositor) = &mut self.compositor {
            compositor.mark_written(contrib, seq);
        }
    }

    fn guard_deadline(&self) -> Option<std::time::Instant> {
        self.compositor.as_ref().and_then(|c| c.guard_deadline())
    }

    fn guard_tick(&mut self) {
        if let Some(compositor) = &mut self.compositor {
            compositor.guard_tick();
        }
    }

    fn guard_wake(&self) -> Option<Arc<Notify>> {
        Some(self.guard_wake.clone())
    }

    fn take_guard_events(&mut self) -> Vec<GuardEvent> {
        let mut events = std::mem::take(&mut self.events);
        if let Some(compositor) = &mut self.compositor {
            events.extend(compositor.take_guard_events());
        }
        events
    }
}
