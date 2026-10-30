//! What the cursor pass costs on the GPU (a measurement, never a gate): two slots at their heaviest, at 1080p and 4K,
//! with the smear across and on the diagonal, against the same pass with nothing in it, through timestamp queries
//! where the adapter has them. lavapipe gives relative cost only; the Mac's own figure comes from a Mac run.

use std::sync::Arc;
use std::time::Instant;

use klipolo_engine::plan::{CursorDraw, CursorLayer};
use klipolo_engine::render::cursor::CursorPass;

use crate::{busy, target};

/// A GPU with timestamp queries where it has them, and what reads them back.
struct Timer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    set: Option<wgpu::QuerySet>,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
}

impl Timer {
    fn new(adapter: &wgpu::Adapter) -> Self {
        use futures::executor::block_on;
        let stamps = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let features = if stamps {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let descriptor = wgpu::DeviceDescriptor {
            required_features: features,
            ..Default::default()
        };
        let (device, queue) = block_on(adapter.request_device(&descriptor)).unwrap();
        let set = stamps.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: None,
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let buffer = |usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        let resolve = buffer(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC);
        let read = buffer(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        Self {
            device,
            queue,
            set,
            resolve,
            read,
        }
    }

    /// The median over 60 frames of the pass's GPU time (if timestamps) and of submit-to-done, in ms.
    fn run(
        &self,
        p: &mut CursorPass,
        size: (u32, u32),
        layers: &[CursorLayer],
    ) -> [Option<f64>; 2] {
        let (device, queue) = (&self.device, &self.queue);
        let view = target(device, size.0, size.1).create_view(&Default::default());
        let (mut gpu, mut wall) = (Vec::new(), Vec::new());
        for _ in 0..60 {
            let mut encoder = device.create_command_encoder(&Default::default());
            p.begin_frame();
            let ranges: Vec<_> = layers
                .iter()
                .filter_map(|l| p.prepare(l, size, true))
                .collect();
            {
                let writes = self.set.as_ref().map(|q| wgpu::RenderPassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                });
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: writes,
                    occlusion_query_set: None,
                });
                for r in &ranges {
                    p.draw(&mut rp, *r);
                }
            }
            p.finish_frame(queue);
            if let Some(q) = &self.set {
                encoder.resolve_query_set(q, 0..2, &self.resolve, 0);
                encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.read, 0, 16);
            }
            let started = Instant::now();
            queue.submit([encoder.finish()]);
            device.poll(wgpu::PollType::Wait).unwrap();
            wall.push(started.elapsed().as_secs_f64() * 1e3);
            if self.set.is_some() {
                self.read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device.poll(wgpu::PollType::Wait).unwrap();
                let ticks: Vec<u64> =
                    bytemuck::cast_slice(&self.read.slice(..).get_mapped_range()).to_vec();
                self.read.unmap();
                let ns = ticks[1].saturating_sub(ticks[0]) as f64
                    * f64::from(queue.get_timestamp_period());
                gpu.push(ns / 1e6);
            }
        }
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied()
        };
        [median(gpu), median(wall)]
    }
}

/// `busy`, its smear turned `degrees` from the horizontal (the same length).
fn busy_at(
    look: CursorDraw,
    at: (f64, f64),
    height: f64,
    aspect: f64,
    degrees: f64,
) -> CursorLayer {
    let mut c = busy(look, at, height, aspect);
    let (sin, cos) = degrees.to_radians().sin_cos();
    c.blur = (4.0 * height * cos / aspect, 4.0 * height * sin);
    c
}

#[test]
#[ignore = "a measurement, relative on lavapipe (the Mac's figure comes from a Mac run): \
            cargo test -p klipolo-engine --release --test cursor_gpu -- --ignored --nocapture cursor_pass_gpu_time"]
fn cursor_pass_gpu_time() {
    let instance = wgpu::Instance::new(&klipolo_engine::platform::gpu::instance_descriptor());
    let adapter = futures::executor::block_on(instance.request_adapter(&Default::default()))
        .expect("an adapter");
    let timer = Timer::new(&adapter);
    let mut p = CursorPass::new(&timer.device, Arc::default());
    println!(
        "cursor pass on {}: two slots, a 4-height smear (16 taps) over 4 rings each",
        adapter.get_info().name
    );
    for (label, size) in [("1080p", (1920, 1080)), ("4K", (3840, 2160))] {
        let aspect = f64::from(size.0) / f64::from(size.1);
        let plain = timer.run(&mut p, size, &[]);
        for degrees in [0.0, 45.0] {
            let layers = [
                busy_at(CursorDraw::Arrow, (0.3, 0.5), 0.03, aspect, degrees),
                busy_at(CursorDraw::Dot, (0.7, 0.5), 0.03, aspect, degrees),
            ];
            let [gpu, wall] = timer.run(&mut p, size, &layers);
            println!(
                "  {label}, smear at {degrees}°: pass {gpu:?} ms GPU (clear only {:?}); submit-to-done {wall:?} ms \
                 (clear only {:?})",
                plain[0], plain[1]
            );
        }
    }
}
