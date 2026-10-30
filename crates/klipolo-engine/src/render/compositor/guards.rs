//! The compositor's side of the main app's guards: its mode, the export's tiles, the crash guard's marks
//! and events, the preview's registry, and the test hooks that read them. The effect stage does the work
//! (`render::effects::{guarded, handshake, sampling, tiles}`).

use klipolo_model::contrib::ContribId;

use super::Compositor;
use crate::registry::Registry;
use crate::render::effects::GuardEvent;
use crate::render::mode::RenderMode;

impl Compositor {
    /// How it treats agent-made shaders.
    pub fn mode(&self) -> RenderMode {
        self.mode
    }

    /// An export's compositor: checked passes predicted over 200 ms draw in tiles.
    pub(crate) fn set_tiled(&mut self, on: bool) {
        self.effects.set_tiled(on);
    }

    /// The app wrote the crash-guard mark `seq` of `id` to disk.
    pub fn mark_written(&mut self, id: &ContribId, seq: u64) {
        self.effects.mark_written(id, seq);
    }

    /// Closes the draw marks whose time ran out.
    pub fn guard_tick(&mut self) {
        self.effects.guard_tick();
    }

    /// When the next open draw mark runs out of time.
    pub fn guard_deadline(&self) -> Option<std::time::Instant> {
        self.effects.guard_deadline()
    }

    /// The registry the preview draws with from now on (`EffectStage::prepare`).
    pub fn prepare_registry(&mut self, registry: &Registry) {
        self.effects.prepare(registry);
    }

    /// The guard's events since the last call.
    pub fn take_guard_events(&mut self) -> Vec<GuardEvent> {
        self.effects.poll_compiles();
        self.effects.take_guard_events()
    }

    /// The device was lost at `lost_at`: the checked effects to strike; unacknowledged marks close.
    pub fn lost_marks(&mut self, lost_at: std::time::Instant) -> Vec<ContribId> {
        self.effects.lost_marks(lost_at)
    }

    /// Whether a draw mark waits for the GPU to finish its first drawing frame.
    pub fn waiting_for_gpu(&self) -> bool {
        self.effects.waiting_for_gpu()
    }

    /// Where the newest frame the GPU finished is written, and this compositor's frame number.
    pub fn gpu_done(
        &self,
    ) -> (
        std::sync::Arc<crate::render::effects::handshake::GpuDone>,
        u64,
    ) {
        (self.effects.gpu_done(), self.effects.frame_number())
    }

    /// The project closed: what the sampler slowed down draws in full again.
    pub fn project_closed(&mut self) {
        self.effects.project_closed();
    }

    #[doc(hidden)]
    pub fn pipelines_pending(&self) -> usize {
        self.effects.pipelines_pending()
    }

    #[doc(hidden)]
    pub fn drawing_thread_compiles(&self) -> u64 {
        self.effects.drawing_thread_compiles()
    }

    #[doc(hidden)]
    pub fn open_marks(&self) -> usize {
        self.effects.open_marks()
    }

    #[doc(hidden)]
    pub fn builtins_ready(&self) -> bool {
        self.effects.builtins_ready()
    }

    #[doc(hidden)]
    pub fn half_size_step_ready(&self) -> bool {
        self.effects.half_size_step_ready()
    }

    #[doc(hidden)]
    pub fn checked_pipelines_kept(&self) -> usize {
        self.effects.checked_pipelines_kept()
    }

    #[doc(hidden)]
    pub fn checked_submissions(&self) -> u32 {
        self.effects.checked_submissions()
    }

    #[doc(hidden)]
    pub fn sampler_mut(&mut self) -> &mut crate::render::effects::sampling::PassSampler {
        self.effects.sampler_mut()
    }

    #[doc(hidden)]
    pub fn sampler(&self) -> &crate::render::effects::sampling::PassSampler {
        self.effects.sampler()
    }
}
