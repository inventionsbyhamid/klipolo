//! Frames without a window, drawn the way export draws them (images and LUTs are waited for). Golden images, the
//! plugin test harness and store CI build on this, and so does the render worker (`with_device`): on
//! its own device, its video frames from the app's stills (`set_stills`), optionally transparent, its frames timed
//! (`render_timed`), and its registry swapped per request (`set_registry`) while its pipelines stay warm.

use std::sync::Arc;

use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::Us;

use crate::api::EngineEvent;
use crate::export::prepare::{cursor_shapes, load_cursors, prepare_cursors};
use crate::media::cursor::CursorTracks;
use crate::media::frozen::FrozenStills;
use crate::plan::{FramePlan, PlanContext, frame_plan};
use crate::present::Output;
use crate::registry::Registry;
use crate::render::GpuBackend;
use crate::render::compositor::FrameTime;
use crate::render::device::RenderDevice;
use crate::render::mode::RenderMode;
use crate::render::shared_fonts::SharedFonts;
use crate::session::{PreviewBackend, RenderRequest, RenderedRgba};

/// Renders many frames on one GPU backend (decoders and pipelines stay warm between them).
pub struct Headless {
    backend: GpuBackend,
    registry: Arc<Registry>,
    /// The cursor takes frames draw and Auto zooms follow (none until `load_cursors` or `set_cursors`).
    cursors: Arc<CursorTracks>,
    /// The render worker's: its videos come from these stills only, never from a decoder.
    worker: bool,
    stills: Option<Arc<FrozenStills>>,
    /// The next timed frame's timestamps fail to arrive, their map left pending (tests).
    fail_timer_read: bool,
    /// The guard's events its frames raised, as the engine sends them to the app.
    events: Vec<EngineEvent>,
}

/// How the render worker's `Headless` draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeadlessOptions {
    /// No background layer: frames start cleared to transparent (a draft layer drawn alone).
    pub transparent: bool,
}

impl Headless {
    /// Frames drawn as export draws them: `Contained`, so agent-made effects draw on frame 0.
    pub async fn new(registry: Arc<Registry>) -> Result<Self, String> {
        Self::with_mode(registry, RenderMode::Contained).await
    }

    /// Frames drawn in `mode`: `Interactive` draws as the app's preview does (the guards' tests).
    pub async fn with_mode(registry: Arc<Registry>, mode: RenderMode) -> Result<Self, String> {
        Ok(Self {
            backend: GpuBackend::new(mode).await?.wait_for_assets(),
            registry,
            cursors: Arc::default(),
            worker: false,
            stills: None,
            fail_timer_read: false,
            events: Vec::new(),
        })
    }

    /// Test hook: frames drawn the preview's way (`Interactive`), never waiting for images, LUTs or scene text being
    /// built (a scene's text shows once its build lands).
    #[doc(hidden)]
    pub async fn preview(registry: Arc<Registry>) -> Result<Self, String> {
        Ok(Self {
            backend: GpuBackend::new(RenderMode::Interactive).await?,
            registry,
            cursors: Arc::default(),
            worker: false,
            stills: None,
            fail_timer_read: false,
            events: Vec::new(),
        })
    }

    /// Reads and smooths `project`'s cursor takes now and puts every shape they draw in the GPU atlas, as export does
    /// before its first frame (`export::prepare`: the same code), so goldens and the plugin harness draw their cursors
    /// (their real shapes from the first frame) and follow them with Auto zooms. Blocks: IO,
    /// smoothing and decoding. It never asks: a take whose cursor can't be read draws none, and a shape that can't be
    /// used draws the Arrow look (both logged).
    pub fn load_cursors(&mut self, project: &Project) {
        let cursors = load_cursors(project).cursors;
        let shapes = cursor_shapes(project, &cursors);
        if !shapes.is_empty() {
            self.backend
                .with_cursor_pass(|pass, dev| prepare_cursors(&cursors, &shapes, pass, dev));
        }
        self.set_cursors(cursors);
    }

    /// Frames draw the cursors of `cursors`' takes (a cache loaded elsewhere); a shape found unusable as a frame draws
    /// it is refused on its take there.
    pub fn set_cursors(&mut self, cursors: Arc<CursorTracks>) {
        self.backend.set_cursors(cursors.clone());
        self.cursors = cursors;
    }

    /// Test hook (see `GpuBackend::cursor_pass_made`).
    #[doc(hidden)]
    pub fn cursor_pass_made(&self) -> bool {
        self.backend.cursor_pass_made()
    }

    /// The render worker's, on its own device `dev`: no decoder at all (its videos come from `set_stills`).
    pub fn with_device(
        dev: Arc<RenderDevice>,
        registry: Arc<Registry>,
        options: HeadlessOptions,
    ) -> Self {
        Self {
            backend: GpuBackend::for_worker(dev, options.transparent),
            registry,
            cursors: Arc::default(),
            worker: true,
            stills: None,
            fail_timer_read: false,
            events: Vec::new(),
        }
    }

    /// What later frames draw with; pipelines and textures stay. A worker's lets the pipelines and kept pictures of
    /// shaders the new registry doesn't hold go, so drafts never pile up.
    pub fn set_registry(&mut self, registry: Arc<Registry>) {
        if self.worker && !Arc::ptr_eq(&self.registry, &registry) {
            self.backend.keep_shaders_of(&registry);
        }
        self.registry = registry;
    }

    /// The effect pipelines a worker's `Headless` keeps (tests: bounded across drafts).
    #[doc(hidden)]
    pub fn effect_pipelines(&self) -> usize {
        self.backend.pipelines_kept()
    }

    /// Where a worker's frames take their video frames (`with_device` only).
    pub fn set_stills(&mut self, stills: Option<Arc<FrozenStills>>) {
        self.stills = stills;
    }

    /// A worker's frames without a background (a draft alone), or with the project's.
    pub fn set_transparent(&mut self, on: bool) {
        self.backend.set_transparent(on);
    }

    /// The font system a worker's texts are shaped with (`make::fonts`).
    pub fn set_fonts(&mut self, fonts: SharedFonts) {
        self.backend.set_fonts(fonts);
    }

    pub fn device(&self) -> &Arc<RenderDevice> {
        self.backend.device()
    }

    /// One frame of a worker's `project`, timed (`Compositor::render_timed`): nothing is read back.
    pub async fn render_timed(
        &mut self,
        project: Arc<Project>,
        at_us: Us,
        format: FormatKind,
        size: (u32, u32),
    ) -> Result<FrameTime, String> {
        let plan = self.plan(project, at_us, format);
        let req = request(plan, size);
        let fail = std::mem::take(&mut self.fail_timer_read);
        self.backend
            .render_timed(req, self.stills.as_deref(), fail)
            .await
    }

    /// `frames` copies of one frame of a worker's `project` in one submission, with one wait, timed by the wall clock
    /// around them (`Compositor::render_batch`): nothing is read back.
    pub async fn render_batch(
        &mut self,
        project: Arc<Project>,
        at_us: Us,
        format: FormatKind,
        size: (u32, u32),
        frames: u32,
    ) -> Result<FrameTime, String> {
        let plan = self.plan(project, at_us, format);
        let req = request(plan, size);
        self.backend
            .render_batch(req, self.stills.as_deref(), frames)
            .await
    }

    /// The next timed frame's timestamp readback fails as one that timed out does, its map left pending (tests).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn fail_next_timer_read(&mut self) {
        self.fail_timer_read = true;
    }

    fn plan(&self, project: Arc<Project>, at_us: Us, format: FormatKind) -> FramePlan {
        let ctx = PlanContext::with_cursors(project, self.registry.clone(), self.cursors.clone());
        frame_plan(&ctx, at_us, format)
    }

    /// Test hook (see `GpuBackend::set_downsample`).
    #[doc(hidden)]
    pub fn set_downsample(&mut self, on: bool) {
        self.backend.set_downsample(on);
    }

    /// Test hook (see `GpuBackend::set_upright`).
    #[doc(hidden)]
    pub fn set_upright(&mut self, on: bool) {
        self.backend.set_upright(on);
    }

    /// Test hook (see `GpuBackend::passes_last_frame`).
    #[doc(hidden)]
    pub fn passes_last_frame(&self) -> u32 {
        self.backend.passes_last_frame()
    }

    /// Test hook (see `GpuBackend::scene_stats`).
    #[doc(hidden)]
    pub fn scene_stats(&self) -> crate::render::scene::SceneStats {
        self.backend.scene_stats()
    }

    /// Test hook (see `GpuBackend::text_stats`).
    #[doc(hidden)]
    pub fn text_stats(
        &self,
    ) -> (
        crate::render::text::TitleStats,
        crate::render::captions::CaptionStats,
    ) {
        self.backend.text_stats()
    }

    /// Test hook: the scene text build thread is gone.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn lose_scene_text_thread(&mut self) {
        self.backend.lose_scene_text_thread();
    }

    /// Test hook: each scene text build waits `delay` first.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn delay_scene_text_builds(&mut self, delay: std::time::Duration) {
        self.backend.delay_scene_text_builds(delay);
    }

    /// Test hook: warms what the frame at `at_us` draws, as the playback lookahead does for a start coming up.
    #[doc(hidden)]
    pub fn warm(&mut self, project: Arc<Project>, at_us: Us, format: FormatKind) {
        let ctx = PlanContext::with_registry(project, self.registry.clone());
        self.backend.warm(&frame_plan(&ctx, at_us, format));
    }

    pub async fn render(
        &mut self,
        project: Arc<Project>,
        at_us: Us,
        format: FormatKind,
        size: (u32, u32),
    ) -> Result<RenderedRgba, String> {
        let req = request(self.plan(project, at_us, format), size);
        let rendered = if self.worker {
            self.backend
                .render_frozen(req, self.stills.as_deref())
                .await
        } else {
            self.backend.render(req).await
        };
        self.take_events();
        rendered?.finish().await
    }

    fn take_events(&mut self) {
        let events = self.backend.take_guard_events();
        self.events
            .extend(events.into_iter().map(|e| e.into_event()));
    }

    /// The guard's events the frames so far raised (`FirstDraw`, `DrewSafely`, `EffectDegraded`, `GpuLost`).
    pub fn events(&self) -> &[EngineEvent] {
        &self.events
    }

    /// The app's answer to a `FirstDraw`: `contrib`'s crash-guard mark `seq` is on disk.
    pub fn mark_written(&mut self, contrib: &klipolo_model::contrib::ContribId, seq: u64) {
        self.backend.mark_written(contrib, seq);
    }

    /// What the session's actor does when the feed brings a registry: its agent-made shaders compiled now.
    pub fn prepare_registry(&mut self) {
        let registry = self.registry.clone();
        self.backend.prepare_registry(&registry);
        self.take_events();
    }

    /// Waits until the compile thread has handed back every pipeline asked for (at most 60 s), then takes what it made
    /// (`FirstDraw` said for each).
    #[doc(hidden)]
    pub async fn wait_for_pipelines(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while self.backend.pipelines_pending() > 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.take_events();
    }

    /// Test hook (see `GpuBackend::simulate_device_loss`).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn simulate_device_loss(&self) {
        self.backend.simulate_device_loss();
    }

    /// Pipelines the preview compiled on its drawing thread so far (tests).
    #[doc(hidden)]
    pub fn drawing_thread_compiles(&self) -> u64 {
        self.backend.drawing_thread_compiles()
    }

    /// Work-done callbacks the preview registered for the crash guard so far (tests).
    #[doc(hidden)]
    pub fn done_callbacks(&self) -> u64 {
        self.backend.done_callbacks()
    }

    /// Crash-guard marks the preview holds open (tests).
    #[doc(hidden)]
    pub fn open_marks(&self) -> usize {
        self.backend.open_marks()
    }

    /// Whether every built-in pipeline is made (tests).
    #[doc(hidden)]
    pub fn builtins_ready(&self) -> bool {
        self.backend.builtins_ready()
    }

    /// Checked compiles asked of the compile thread and not handed back (tests).
    #[doc(hidden)]
    pub fn pipelines_pending(&self) -> usize {
        self.backend.pipelines_pending()
    }

    /// What the session's actor does when the backend's guard deadline passes: marks past their time close.
    pub fn guard_tick(&mut self) {
        self.backend.guard_tick();
        self.take_events();
    }

    #[doc(hidden)]
    pub fn half_size_step_ready(&self) -> bool {
        self.backend.half_size_step_ready()
    }

    #[doc(hidden)]
    pub fn checked_pipelines_kept(&self) -> usize {
        self.backend.checked_pipelines_kept()
    }

    /// Test hook (see `GpuBackend::sample_with_clock`).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn sample_with_clock(&mut self, every: u64, step_s: f64) {
        self.backend.sample_with_clock(every, step_s);
    }

    #[doc(hidden)]
    pub fn set_sampling(&mut self, on: bool) {
        self.backend.set_sampling(on);
    }

    #[doc(hidden)]
    pub fn sampler_cost(&self) -> std::time::Duration {
        self.backend.sampler_cost()
    }
}

fn request(plan: FramePlan, size: (u32, u32)) -> RenderRequest {
    RenderRequest {
        plan,
        width: size.0,
        height: size.1,
        output: Output::Rgba,
    }
}

/// One frame of `project` at `at_us` in `format`, `size` pixels.
pub async fn render_frame(
    project: Arc<Project>,
    registry: Arc<Registry>,
    at_us: Us,
    format: FormatKind,
    size: (u32, u32),
) -> Result<RenderedRgba, String> {
    Headless::new(registry)
        .await?
        .render(project, at_us, format, size)
        .await
}

#[cfg(test)]
mod tests {
    /// The hooks that fail or hold a readback exist only with `test-support`, so a shipped build never
    /// contains them.
    #[test]
    fn the_readback_hooks_are_test_support_only() {
        let render = include_str!("make/audit/render/cache.rs");
        for (file, hook) in [
            (include_str!("headless.rs"), "pub fn fail_next_timer_read"),
            (render, "pub fn fail_next_read"),
            (render, "pub fn hold_next_read"),
        ] {
            let at = file.find(hook).expect(hook);
            let above: Vec<&str> = file[..at].lines().rev().take(3).collect();
            assert!(
                above
                    .iter()
                    .any(|l| l.trim() == "#[cfg(feature = \"test-support\")]"),
                "{hook}: {above:?}"
            );
        }
    }
}
