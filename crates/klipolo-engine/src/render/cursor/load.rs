//! Getting shapes into the cursor pass's atlas: the preview's, decoded off the render thread and uploaded between
//! frames (`upload_ready`, asked for by the lookahead's `request`); export's and headless renders', decoded inline
//! before frame 1 (`preload`) or before a strict frame draws one it still misses (`ensure`). The bind group follows
//! the atlas when it is made or let go (`forget`).

use std::sync::Arc;

use super::{CursorPass, CursorShapeError, pipeline};
use crate::plan::{CursorDraw, CursorLayer};

impl CursorPass {
    /// Shapes decoded off-thread enter the atlas (between frames, never mid-pass): level 0 written now, their mips
    /// made and submitted before the frame is.
    pub fn upload_ready(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.atlas.upload_ready(device, queue) {
            self.atlas.finish_uploads(device, queue);
            self.rebind(device);
        }
    }

    /// Export and headless: decodes and uploads the listed shapes now (blocking), before frame 1, in order, until the
    /// atlas is full: none goes to make room, and the rest load as strict frames draw them (`ensure`).
    /// The first that can't be used is returned (it draws the Arrow look, in preview too); the shapes after the cut
    /// aren't decoded, so `failed` is what says which were refused.
    pub fn preload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[(Arc<str>, u16, Arc<str>)],
    ) -> Result<(), CursorShapeError> {
        let mut first = Ok(());
        for (take, shape, png) in shapes {
            if self.atlas.resident(take, *shape) {
                continue;
            }
            match self.atlas.load_now(device, queue, take, *shape, png, false) {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    tracing::warn!(file = %crate::log_name(&**png), %error, "cursor shape: drawn as the arrow");
                    first = first.and(Err(error));
                }
            }
        }
        self.atlas.finish_uploads(device, queue);
        self.rebind(device);
        first
    }

    /// A strict frame (export, headless): `layer`'s shape is loaded now if it isn't resident (more shapes in range
    /// than the atlas holds at once), so it never draws the stand-in. Nothing to do once it is.
    pub fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, layer: &CursorLayer) {
        let CursorDraw::Shape {
            take, shape, file, ..
        } = &layer.look
        else {
            return;
        };
        if layer.opacity <= 0.0
            || self.atlas.resident(take, *shape)
            || self.atlas.failed(take, *shape)
        {
            return;
        }
        match self.atlas.load_now(device, queue, take, *shape, file, true) {
            // At most one other shape is drawn in a frame, so room is always made.
            Ok(placed) => debug_assert!(placed, "no room for a strict frame's shape"),
            Err(error) => {
                tracing::warn!(file = %crate::log_name(&**file), %error, "cursor shape: drawn as the arrow");
            }
        }
        self.atlas.finish_uploads(device, queue);
        self.rebind(device);
    }

    /// `f` for each shape refused since the last call (a decode that failed: the preview's decoder, a preload or an
    /// `ensure`), so its take plans the Arrow look from then on (`CursorTracks::refuse_shape`). No allocation.
    pub fn take_refused(&mut self, f: impl FnMut(&Arc<str>, u16)) {
        self.atlas.take_refused(f);
    }

    /// Starts loading `layer`'s shape if it isn't there (the playback lookahead).
    pub fn request(&mut self, layer: &CursorLayer) {
        if let CursorDraw::Shape {
            take, shape, file, ..
        } = &layer.look
        {
            self.atlas.request(take.clone(), *shape, file.clone());
        }
    }

    /// Lets go of the atlas and its shapes (the project closed, the window hid); the pipeline and the ring stay.
    pub fn forget(&mut self, device: &wgpu::Device) {
        self.atlas.forget();
        self.rebind(device);
    }

    /// The bind group follows the atlas (made or let go).
    pub(super) fn rebind(&mut self, device: &wgpu::Device) {
        if self.bound != self.atlas.texture.epoch {
            let view = self.atlas.texture.view();
            let view = view.as_ref().unwrap_or(&self.placeholder);
            self.bind = pipeline::bind(device, &self.made, view, &self.ring);
            self.bound = self.atlas.texture.epoch;
        }
    }
}
