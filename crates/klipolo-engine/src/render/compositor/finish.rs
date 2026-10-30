//! The end of a frame: submit it, finished as the presenter asked. Read back as RGBA or as NV12 (BT.709, converted on
//! the GPU first), with the pixels following when the GPU is done; or left in a native presenter's slot, whose copy to
//! the screen comes after this on the same queue.

use super::Compositor;
use crate::frames::trailer::{FORMAT_NV12, FORMAT_RGBA8};
use crate::present::{NativeFrame, Output};
use crate::render::device::RenderDevice;
use crate::render::readback::{submit_buffer_readback, submit_readback};
use crate::session::Pixels;
use crate::vendor::cap::rendering::frame_pipeline::RgbaToNv12Converter;

impl Compositor {
    /// Submits `encoder` (the frame drawn at `size`); returns the bytes per row, the pixel format and the pixels.
    pub(super) fn finish(
        &mut self,
        dev: &RenderDevice,
        slot: Option<NativeFrame>,
        output: Output,
        size: (u32, u32),
        mut encoder: wgpu::CommandEncoder,
    ) -> (u32, u16, Pixels) {
        let (device, queue) = (&dev.device, &dev.queue);
        if let Some(slot) = slot {
            queue.submit([encoder.finish()]);
            return (size.0 * 4, FORMAT_RGBA8, Pixels::Native(Some(slot)));
        }
        let texture = self
            .session
            .as_ref()
            .expect("made before drawing")
            .current_texture();
        let nv12 = (output == Output::Nv12)
            .then(|| {
                self.nv12
                    .get_or_insert_with(|| RgbaToNv12Converter::new(device))
            })
            .and_then(|c| {
                c.encode_into_storage(device, queue, &mut encoder, texture, size.0, size.1)
            });
        match nv12 {
            Some((buffer, bytes, stride)) => {
                let pending =
                    submit_buffer_readback(dev, &self.ring, &self.frames, buffer, bytes, encoder);
                (stride, FORMAT_NV12, Pixels::Gpu(pending))
            }
            None => {
                let (stride, pending) =
                    submit_readback(dev, &self.ring, &self.frames, texture, size, encoder);
                (stride, FORMAT_RGBA8, Pixels::Gpu(pending))
            }
        }
    }
}
