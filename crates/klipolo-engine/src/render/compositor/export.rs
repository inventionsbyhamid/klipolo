//! Export frames: drawn exactly as the preview draws them (`draw`), then converted on the GPU into the encoder's
//! YUV layout and either copied into an IOSurface VideoToolbox reads directly, or read back (SoftwareSafe mode).

use super::Compositor;
use crate::plan::FramePlan;
use crate::render::device::GPU_FAILED;
use crate::render::pool::DecodedSet;
use crate::render::yuv::{YuvOut, YuvPixels, YuvSink};

impl Compositor {
    /// Draws `plan` at `size`, converts it into `out`'s layout and submits it. Returns the pixels (an IOSurface the
    /// GPU is still writing, or a pending readback) and the frame's warnings.
    pub(crate) async fn render_yuv(
        &mut self,
        plan: &FramePlan,
        frames: &DecodedSet,
        size: (u32, u32),
        out: &mut YuvOut,
    ) -> Result<(YuvPixels, Vec<String>), String> {
        let dev = self.dev.clone();
        let target = self.session_view(size);
        let (mut encoder, mut warnings, missing) = self.draw(plan, frames, size, &target).await;
        // Export fails on an image that isn't there, in its own words (a video's is checked before drawing).
        warnings.extend(
            missing
                .iter()
                .map(|path| crate::render::pool::missing(path)),
        );
        let texture = self
            .session
            .as_ref()
            .expect("made by session_view")
            .current_texture();
        let YuvOut { converter, sink } = out;
        converter.encode(&dev.device, &mut encoder, texture);
        let pixels: Result<YuvPixels, String> = match sink {
            #[cfg(target_os = "macos")]
            YuvSink::Surface(pool) => match pool.next(&dev.device).await {
                Ok((buffer, y, uv)) => {
                    converter.copy_to_planes(&mut encoder, y, uv);
                    dev.queue.submit([encoder.finish()]);
                    Ok(YuvPixels::Surface(buffer))
                }
                // Given up before its submit.
                Err(e) => Err(e),
            },
            YuvSink::Readback => Ok(YuvPixels::Readback(converter.submit_readback(
                &dev,
                &self.ring,
                &self.frames,
                encoder,
            ))),
        };
        // As the preview's frames do: the captions' staging buffers are mapped again.
        let faulted = dev.take_fault().is_some();
        self.frame_submitted(faulted || pixels.is_err());
        if faulted {
            return Err(GPU_FAILED.into());
        }
        Ok((pixels?, warnings))
    }
}
