//! The preview backend's side of the main app's guards: the app's preview backend, its mode and
//! its compositor's, what a lost compositor's checked effects were, and the test hooks the guards' GPU tests read.

use super::GpuBackend;
use super::effects::GuardEvent;
use super::mode::RenderMode;
use super::warm::Warmup;

impl GpuBackend {
    /// The backend the app's preview runs on (`commands/engine.rs`): the GPU warm-up's (`Interactive`, started at launch)
    /// unless its device was lost meanwhile, else a new one, `Interactive` too.
    pub async fn for_preview(warmup: Option<Warmup>) -> Result<Self, String> {
        let warmed = match warmup {
            // A device lost while Home showed would start the native presenter on it, and the first frame's device
            // swap would leave the whole session on the socket fallback: start fresh instead.
            Some(warmup) => warmup.finish().await.filter(|b| !b.is_lost()),
            None => None,
        };
        match warmed {
            Some(backend) if backend.mode == RenderMode::Interactive => Ok(backend),
            Some(_) => {
                tracing::error!("the GPU warm-up wasn't the preview's: a new backend");
                Self::new(RenderMode::Interactive).await
            }
            None => Self::new(RenderMode::Interactive).await,
        }
    }

    /// How its compositors treat agent-made shaders.
    pub fn mode(&self) -> RenderMode {
        self.mode
    }

    /// The compositor's mode (None while the warm-up is still making it).
    pub fn compositor_mode(&self) -> Option<RenderMode> {
        self.compositor.as_ref().map(|c| c.mode())
    }

    /// The device was lost: every mark its compositor held is settled as a crash at that moment would settle it: the
    /// effects under an acknowledged open mark, or drawn in the last 60 frames and 2 s, are named for
    /// the app, which strikes them; marks asked for and never acknowledged close. With the events it hadn't handed over
    /// yet. The compositor goes with the device.
    pub(super) fn lost_compositor(&mut self) {
        if let Some(mut old) = self.compositor.take() {
            let at = self.dev.lost_at().unwrap_or_else(std::time::Instant::now);
            // Which of its frames the GPU finished: every frame's own work-done callback wrote it.
            let struck = old.lost_marks(at);
            self.events.extend(old.take_guard_events());
            if !struck.is_empty() {
                self.events.push(GuardEvent::GpuLost(struck));
            }
        }
        self.coming = None;
    }

    /// After a preview frame was submitted, only while a draw mark waits for the GPU to finish its first drawing
    /// frame: when the GPU has done it, its frame number is written where the stage reads it, and the session's actor
    /// is woken (`guard_wake`). Otherwise (steady state) nothing is registered and nothing polled: a preview frame
    /// allocates nothing for the crash guard.
    pub(super) fn signal_done(&mut self) {
        let Some(c) = &self.compositor else {
            return;
        };
        if !c.waiting_for_gpu() {
            return;
        }
        self.done_callbacks += 1;
        let ((gpu, frame), wake) = (c.gpu_done(), self.guard_wake.clone());
        self.dev.queue.on_submitted_work_done(move || {
            if gpu.done(frame) {
                wake.notify_one();
            }
        });
        self.dev.poll_soon();
    }

    /// Work-done callbacks registered for the crash guard so far (tests).
    #[doc(hidden)]
    pub fn done_callbacks(&self) -> u64 {
        self.done_callbacks
    }

    /// Test hook: waits for the warm-up's compositor; its mode when it came from the warm-up.
    #[doc(hidden)]
    pub async fn warm_compositor_mode(&mut self) -> Option<RenderMode> {
        self.compositor_ready().await;
        self.warmed.then(|| self.compositor_mode()).flatten()
    }

    /// Test hook: the device reads as lost (the next frame makes a new device and compositor).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn simulate_device_loss(&self) {
        self.dev.simulate_loss();
    }

    /// The submissions the last frame's checked passes took: 1 each, or one per tile (tests).
    #[doc(hidden)]
    pub fn checked_pass_submissions(&self) -> u32 {
        self.compositor
            .as_ref()
            .map_or(0, |c| c.checked_submissions())
    }

    /// Requests the compile thread hasn't handed back yet (tests).
    #[doc(hidden)]
    pub fn pipelines_pending(&self) -> usize {
        self.compositor
            .as_ref()
            .map_or(0, |c| c.pipelines_pending())
    }

    #[doc(hidden)]
    pub fn drawing_thread_compiles(&self) -> u64 {
        self.compositor
            .as_ref()
            .map_or(0, |c| c.drawing_thread_compiles())
    }

    #[doc(hidden)]
    pub fn open_marks(&self) -> usize {
        self.compositor.as_ref().map_or(0, |c| c.open_marks())
    }

    #[doc(hidden)]
    pub fn builtins_ready(&self) -> bool {
        self.compositor.as_ref().is_some_and(|c| c.builtins_ready())
    }

    #[doc(hidden)]
    pub fn half_size_step_ready(&self) -> bool {
        self.compositor
            .as_ref()
            .is_some_and(|c| c.half_size_step_ready())
    }

    #[doc(hidden)]
    pub fn checked_pipelines_kept(&self) -> usize {
        self.compositor
            .as_ref()
            .map_or(0, |c| c.checked_pipelines_kept())
    }

    /// The sampler's test hooks: sampled every `every` frames on a clock that moves `step_s` a frame.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn sample_with_clock(&mut self, every: u64, step_s: f64) {
        if let Some(c) = &mut self.compositor {
            c.sampler_mut().set_clock(every, step_s);
        }
    }

    /// Sampling on or off (the check run by hand on a Mac after a Mac build).
    #[doc(hidden)]
    pub fn set_sampling(&mut self, on: bool) {
        if let Some(c) = &mut self.compositor {
            c.sampler_mut().set_on(on);
        }
    }

    /// The time the sampler itself took so far (the check run by hand on a Mac after a Mac build).
    #[doc(hidden)]
    pub fn sampler_cost(&self) -> std::time::Duration {
        self.compositor
            .as_ref()
            .map_or(std::time::Duration::ZERO, |c| c.sampler().cost())
    }
}
