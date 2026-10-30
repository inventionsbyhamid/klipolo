//! What one frame's drawing carries from layer to layer (`FrameCtx`), how a run of passes starts (`PassStart`), and the
//! render worker's timer on a frame's first and last render passes (`FrameTimer`).

use crate::render::effects::FrameInfo;

/// What every layer of one frame shares while it is prepared and drawn.
pub(super) struct FrameCtx<'a> {
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub info: FrameInfo,
    /// The output size.
    pub size: (u32, u32),
    /// The format's canvas in pixels (`FramePlan::canvas`): text is laid out on it.
    pub canvas: (u32, u32),
    pub warnings: &'a mut Vec<String>,
    /// Image files that aren't there (said as the copy row says a missing file, `api::MEDIA_MISSING`).
    pub missing: &'a mut Vec<std::sync::Arc<str>>,
    /// Wait for images and LUTs (export, tests).
    pub wait: bool,
    /// The timeline frame grain follows.
    pub grain_frame: u32,
    /// Black fades prepared so far this frame (each has its own `blacks` entry).
    pub blacks: usize,
    /// The next entry of the decoded frames: they follow `FramePlan::videos()` order (a transition's outgoing side,
    /// then its incoming side).
    pub video: usize,
    /// The size of the target the layers being prepared draw into: the output, or a transition side's.
    pub target: (u32, u32),
}

/// The timestamp the frame's last render pass writes at its end (`FrameTimer`).
pub(crate) const END_INDEX: u32 = 1;

/// How the first pass of `encode_passes` starts.
#[derive(Clone, Copy)]
pub(super) enum PassStart {
    /// Cleared to black, then the project's background (the frame itself).
    Background,
    /// Cleared to this colour, with no background (each side of a transition).
    Clear(wgpu::Color),
}

/// Times one frame's GPU work for the render worker's meter, when the device has
/// `TIMESTAMP_QUERY` and its timestamps are trusted (`render::device::timestamps_trusted`): the frame's first render
/// pass writes the first timestamp at its beginning (a pass that clears the frame's target, before anything else the
/// frame encodes) and its last render pass the second at its end (`RenderPassTimestampWrites`), both resolved in the
/// frame's own command buffer and read once it is done. Made once and reused every frame; a reading that fails or
/// isn't plausible gives way to the fenced wall clock (`compositor::timed`).
pub(crate) struct FrameTimer {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// Nanoseconds per timestamp tick (`Queue::get_timestamp_period`).
    period_ns: f64,
}

impl FrameTimer {
    /// None without trusted timestamps (the meter then times the submission on the wall clock).
    pub(crate) fn new(dev: &crate::render::device::RenderDevice) -> Option<Self> {
        if !dev.timestamps_trusted() {
            return None;
        }
        let device = &dev.device;
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Klipolo frame timer"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: buffer(
                "Klipolo frame timer resolve",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            read: buffer(
                "Klipolo frame timer read",
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            period_ns: f64::from(dev.queue.get_timestamp_period()),
        })
    }

    /// The frame's first render pass: it clears `target` (the frame's own, which its first layer pass clears again) and
    /// writes the first timestamp at its beginning, before anything else the frame encodes.
    pub(crate) fn begin(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Klipolo frame timer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                query_set: &self.set,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: None,
            }),
            occlusion_query_set: None,
        }));
    }

    /// The query set the frame's last render pass writes the second timestamp to, at its end (`END_INDEX`).
    pub(crate) fn query_set(&self) -> wgpu::QuerySet {
        self.set.clone()
    }

    /// After the last pass: both timestamps resolved into the buffer `read` maps.
    pub(crate) fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.resolve_query_set(&self.set, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.read, 0, 16);
    }

    /// The frame's GPU milliseconds, once it was submitted (the device's poll thread completes the map). `fail` answers
    /// as a readback that timed out does, its map left pending (tests).
    pub(crate) async fn read(
        &self,
        dev: &crate::render::device::RenderDevice,
        fail: bool,
    ) -> Result<f64, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.read
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        dev.poll_soon();
        if fail {
            return Err("the frame's timestamps didn't arrive".into());
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(()))) => {}
            _ => return Err("the frame's timestamps didn't arrive".into()),
        }
        let ticks: [u64; 2] = {
            let bytes = self.read.slice(..).get_mapped_range();
            [
                u64::from_le_bytes(bytes[..8].try_into().expect("16 bytes")),
                u64::from_le_bytes(bytes[8..16].try_into().expect("16 bytes")),
            ]
        };
        self.read.unmap();
        Ok(ticks[1].saturating_sub(ticks[0]) as f64 * self.period_ns / 1e6)
    }
}
