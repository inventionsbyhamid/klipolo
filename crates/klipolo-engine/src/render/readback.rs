//! The end of every frame: copy the render target into a reused staging buffer, submit, and hand back a
//! `PendingPixels` that resolves when the GPU is done. The device's poll thread completes the `map_async`; nobody
//! spins or sleeps, and the actor keeps working while the GPU draws (it publishes when the pixels arrive).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::frames::buffer::{FrameBytes, FramePool};
use crate::render::device::{GPU_FAILED, RenderDevice};
use crate::vendor::cap::rendering::frame_pipeline::padded_bytes_per_row;

/// Staging buffers kept: the frames in flight (the actor allows two) and one spare.
const RING: usize = 3;
/// A frame the GPU hasn't finished in this long is given up (Cap's limit; software adapters get longer).
const TIMEOUT: Duration = Duration::from_secs(10);
const SOFTWARE_TIMEOUT: Duration = Duration::from_secs(60);

/// Reused MAP_READ staging buffers of the current frame size (a new one only when all are busy or the size grows).
#[derive(Clone, Default)]
pub(crate) struct ReadbackRing {
    free: Arc<Mutex<Vec<Arc<wgpu::Buffer>>>>,
}

impl ReadbackRing {
    fn lease(&self, device: &wgpu::Device, size: u64) -> Lease {
        let buffer = {
            let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
            // Buffers of another size were for another preview size; let them go.
            free.retain(|b| b.size() == size);
            free.pop()
        };
        let buffer = buffer.unwrap_or_else(|| {
            Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Klipolo readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }))
        });
        Lease {
            buffer,
            ring: self.clone(),
            mapping: false,
        }
    }
}

/// A staging buffer out of the ring; it goes back (unmapped) when dropped.
struct Lease {
    buffer: Arc<wgpu::Buffer>,
    ring: ReadbackRing,
    /// A map was requested and not yet undone.
    mapping: bool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.mapping {
            // Also cancels a map still in flight (a frame dropped on reset).
            self.buffer.unmap();
        }
        let mut free = self
            .ring
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if free.len() < RING {
            free.push(self.buffer.clone());
        }
    }
}

/// Frame pixels the GPU is still producing.
pub struct PendingPixels {
    rx: oneshot::Receiver<Result<(), wgpu::BufferAsyncError>>,
    lease: Lease,
    bytes: u64,
    pool: FramePool,
    deadline: tokio::time::Instant,
}

impl PendingPixels {
    /// The pixels, copied once from the mapped staging buffer into a pooled frame buffer. Cancel-safe: dropping the
    /// future before the GPU is done leaves the frame pending. Don't call it again once it has returned.
    pub async fn wait(&mut self) -> Result<FrameBytes, String> {
        match tokio::time::timeout_at(self.deadline, &mut self.rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                tracing::error!(%error, "frame readback failed");
                return Err(GPU_FAILED.into());
            }
            Ok(Err(_)) | Err(_) => {
                tracing::error!("frame readback timed out");
                return Err(GPU_FAILED.into());
            }
        }
        let mut out = self.pool.take(self.bytes as usize);
        {
            let view = self.lease.buffer.slice(..self.bytes).get_mapped_range();
            out.vec_mut().extend_from_slice(&view);
        }
        self.lease.buffer.unmap();
        self.lease.mapping = false;
        Ok(out)
    }
}

/// Copies `texture` (the frame, `size` pixels) into a staging buffer at the end of `encoder`, submits it, and starts
/// mapping the buffer. Returns the bytes per row and the pending pixels.
pub(crate) fn submit_readback(
    dev: &RenderDevice,
    ring: &ReadbackRing,
    pool: &FramePool,
    texture: &wgpu::Texture,
    size: (u32, u32),
    mut encoder: wgpu::CommandEncoder,
) -> (u32, PendingPixels) {
    let stride = padded_bytes_per_row(size);
    let bytes = u64::from(stride) * u64::from(size.1);
    let lease = ring.lease(&dev.device, bytes);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &lease.buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    (stride, finish(dev, pool, lease, bytes, encoder))
}

/// Copies the first `bytes` of `source` (NV12 planes the converter wrote) into a staging buffer at the end of
/// `encoder`, submits it, and starts mapping the buffer.
pub(crate) fn submit_buffer_readback(
    dev: &RenderDevice,
    ring: &ReadbackRing,
    pool: &FramePool,
    source: &wgpu::Buffer,
    bytes: u64,
    mut encoder: wgpu::CommandEncoder,
) -> PendingPixels {
    let lease = ring.lease(&dev.device, bytes);
    encoder.copy_buffer_to_buffer(source, 0, &lease.buffer, 0, bytes);
    finish(dev, pool, lease, bytes, encoder)
}

/// Submits `encoder` and starts mapping the lease's first `bytes`; the device's poll thread completes the map.
fn finish(
    dev: &RenderDevice,
    pool: &FramePool,
    mut lease: Lease,
    bytes: u64,
    encoder: wgpu::CommandEncoder,
) -> PendingPixels {
    dev.queue.submit([encoder.finish()]);
    let (tx, rx) = oneshot::channel();
    lease
        .buffer
        .slice(..bytes)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    lease.mapping = true;
    dev.poll_soon();
    let timeout = if dev.software {
        SOFTWARE_TIMEOUT
    } else {
        TIMEOUT
    };
    PendingPixels {
        rx,
        lease,
        bytes,
        pool: pool.clone(),
        deadline: tokio::time::Instant::now() + timeout,
    }
}
