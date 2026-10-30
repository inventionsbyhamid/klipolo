//! The scene pass's stencil attachments: a translucent stroke blends
//! once a pixel through one. One `Stencil8` texture per target size a scene draws at (an attachment must match its
//! target, so its size is clamped as the target's is), made the first time a size draws: a resize, never a steady
//! frame. Every stencil drawn with this frame or the last stays, so any number of scene layers of any sizes on screen
//! together keep theirs (each matches a target the effect stage budgeted); past [`MAX_STENCILS`] or
//! [`MAX_STENCIL_BYTES`] those off screen go, the least recently used first, and any unused for
//! [`EXPIRE_AFTER_FRAMES`] go at a frame's start; all go when the drawer forgets. Their bytes are counted one a pixel
//! where Stencil8 is native (Metal, so the Mac), four elsewhere (DX12, and Vulkan without S8, emulate it in a
//! depth-stencil format): against their own bound, never the scene instances', and reported in
//! the drawer's stats and the scene bytes peak. A test may count them at the other platforms' bytes a pixel, so the
//! bound is pressed at the Mac's count on any GPU.

use std::sync::atomic::{AtomicU64, Ordering};

use super::cache::MAX_SCENES;
use super::path_pipeline::STENCIL_FORMAT;
use crate::render::effects::{made, target_size};

/// The most stencils kept when some are off screen.
pub const MAX_STENCILS: usize = MAX_SCENES;
/// The most bytes they may hold, counted, when some are off screen: two 4K stencils or eight at 1080p, emulated.
pub const MAX_STENCIL_BYTES: u64 = 64 << 20;
/// A stencil unused for this many frames goes.
const EXPIRE_AFTER_FRAMES: u64 = 8;
/// The bytes a stencil pixel takes on this platform, counted.
const BYTES_PER_PIXEL: u64 = if cfg!(target_os = "macos") { 1 } else { 4 };
/// The bytes a stencil pixel is counted at: [`BYTES_PER_PIXEL`] unless a test asks for another platform's.
static COUNTED_PER_PIXEL: AtomicU64 = AtomicU64::new(BYTES_PER_PIXEL);

/// Test hook: stencils made from now on are counted at `bytes` a pixel (1 as on the Mac, 4 where Stencil8 is
/// emulated), whatever this platform's; `None` goes back to this platform's. Only in the engine's own tests and test
/// builds (`test-support`): a shipped build has no way to change the count.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn count_stencil_pixels_at(bytes: Option<u64>) {
    COUNTED_PER_PIXEL.store(bytes.unwrap_or(BYTES_PER_PIXEL), Ordering::Relaxed);
}

struct Stencil {
    size: (u32, u32),
    /// Counted when it was made.
    bytes: u64,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    used: u64,
}

/// The bytes the stencil of a target of `size` is counted at (a scene's `SceneBytes::buffers`).
pub(super) fn bytes_for(size: (u32, u32)) -> u64 {
    bytes_of(size)
}

fn bytes_of(size: (u32, u32)) -> u64 {
    u64::from(size.0) * u64::from(size.1) * COUNTED_PER_PIXEL.load(Ordering::Relaxed)
}

/// Which of the `kept` stencils (bytes, frame last used) goes to make room for one more of `adding` bytes at frame
/// `now`: past [`MAX_STENCILS`] or [`MAX_STENCIL_BYTES`], the least recently used of those off screen (not drawn this
/// frame or the last); none while every one is on screen.
fn victim(kept: impl Iterator<Item = (u64, u64)> + Clone, adding: u64, now: u64) -> Option<usize> {
    let bytes: u64 = kept.clone().map(|(b, _)| b).sum();
    if kept.clone().count() < MAX_STENCILS && bytes + adding <= MAX_STENCIL_BYTES {
        return None;
    }
    kept.enumerate()
        .filter(|(_, (_, used))| used + 1 < now)
        .min_by_key(|(_, (_, used))| *used)
        .map(|(i, _)| i)
}

#[derive(Default)]
pub(super) struct Stencils {
    list: Vec<Stencil>,
}

impl Stencils {
    /// The stencil for a target of `size` drawn at frame `now`, made when none is kept.
    pub(super) fn view(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        now: u64,
    ) -> &wgpu::TextureView {
        let size = target_size(size);
        let i = match self.list.iter().position(|s| s.size == size) {
            Some(i) => i,
            None => {
                let adding = bytes_of(size);
                while let Some(i) = victim(self.list.iter().map(|s| (s.bytes, s.used)), adding, now)
                {
                    self.list.swap_remove(i);
                }
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Klipolo scene stencil"),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: STENCIL_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                made();
                made();
                self.list.push(Stencil {
                    size,
                    bytes: adding,
                    _texture: texture,
                    view,
                    used: now,
                });
                self.list.len() - 1
            }
        };
        self.list[i].used = now;
        &self.list[i].view
    }

    /// A frame's start (`now`): stencils unused for a while go.
    pub(super) fn expire(&mut self, now: u64) {
        self.list
            .retain(|s| now.saturating_sub(s.used) <= EXPIRE_AFTER_FRAMES);
    }

    /// The bytes they hold, counted.
    pub(super) fn bytes(&self) -> u64 {
        self.list.iter().map(|s| s.bytes).sum()
    }

    pub(super) fn clear(&mut self) {
        self.list.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off-screen stencils go past their own bounds, the count or [`MAX_STENCIL_BYTES`], the
    /// least recently used first; one on screen (drawn this frame or the last) never goes.
    #[test]
    fn off_screen_stencils_go_past_their_own_bounds() {
        let mb = 1 << 20;
        let kept = [(40 * mb, 10), (40 * mb, 99)];
        assert_eq!(victim(kept.iter().copied(), 10 * mb, 100), Some(0));
        let on_screen = [(40 * mb, 99), (40 * mb, 100)];
        assert_eq!(victim(on_screen.iter().copied(), 10 * mb, 100), None);
        assert_eq!(victim([(mb, 1)].into_iter(), mb, 100), None);
        let many: Vec<(u64, u64)> = (0..MAX_STENCILS as u64).map(|i| (mb, 50 + i)).collect();
        assert_eq!(victim(many.iter().copied(), mb, 100), Some(0));
    }
}
