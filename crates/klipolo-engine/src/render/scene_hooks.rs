//! Test hooks into the backend's scene drawer: its text build thread lost, and its builds made slow.

use super::GpuBackend;

impl GpuBackend {
    /// The scene drawer's text build thread is gone, as one that failed to start.
    pub fn lose_scene_text_thread(&mut self) {
        if let Some(compositor) = &mut self.compositor {
            compositor.scene_drawer().lose_text_thread();
        }
    }

    /// Each scene text build waits `delay` first.
    pub fn delay_scene_text_builds(&mut self, delay: std::time::Duration) {
        if let Some(compositor) = &mut self.compositor {
            compositor.scene_drawer().delay_text_builds(delay);
        }
    }
}
