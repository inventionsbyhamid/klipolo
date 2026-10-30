//! The GPU cursor pass on a 256 × 144 target read back: where it draws and where it doesn't,
//! the looks, smear and ripples (`cursor_gpu/looks.rs`), the atlas, preload and refused images (`cursor_gpu/atlas.rs`),
//! allocation and a separate take through the compositor (`cursor_gpu/frames.rs`), the pass's GPU cost
//! (`cursor_gpu/timing.rs`, a measurement). The GPU tests need a GPU (lavapipe in CI):
//! `WGPU_BACKEND=vulkan cargo test -p klipolo-engine --test cursor_gpu -- --ignored`.

mod support;

#[path = "cursor_gpu/atlas.rs"]
mod atlas;
#[path = "cursor_gpu/frames.rs"]
mod frames;
#[path = "cursor_gpu/looks.rs"]
mod looks;
#[path = "cursor_gpu/timing.rs"]
mod timing;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use klipolo_engine::plan::{CursorDraw, CursorLayer, Ripple};
use klipolo_engine::render::cursor::CursorPass;
use klipolo_engine::render::device::RenderDevice;
use klipolo_model::layout::BoxN;

/// Counts this thread's allocations (`drawing_a_cursor_frame_allocates_nothing_after_warm_up`).
struct Counting;

thread_local! {
    pub static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator; the counter is a const-initialised thread-local (no allocation).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(p, layout, size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

pub const W: u32 = 256;
pub const H: u32 = 144;

pub fn device() -> Arc<RenderDevice> {
    futures::executor::block_on(RenderDevice::new()).expect("a GPU (lavapipe in CI)")
}

/// A pass whose redraw signal nobody waits for.
pub fn pass(dev: &RenderDevice) -> CursorPass {
    CursorPass::new(&dev.device, Arc::default())
}

/// A cursor layer of `look` with its hotspot at `hotspot` and `height` (canvas-normalized), over the whole canvas.
pub fn layer(look: CursorDraw, hotspot: (f64, f64), height: f64) -> CursorLayer {
    CursorLayer {
        id: "cursor:clip".into(),
        look,
        hotspot,
        height,
        opacity: 1.0,
        press: 1.0,
        blur: (0.0, 0.0),
        clip: BoxN {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
        radius: 0.0,
        mask: None,
        ripples: [Ripple::default(); 4],
        ripple_count: 0,
    }
}

/// A frame read back: premultiplied RGBA, `W` × `H`.
pub struct Frame(pub Vec<u8>);

impl Frame {
    pub fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * W + x) * 4) as usize;
        self.0[i..i + 4].try_into().unwrap()
    }

    pub fn alpha(&self, x: u32, y: u32) -> u8 {
        self.at(x, y)[3]
    }

    /// The pixels of row `y` (or column `x` when `down`) whose alpha passes `over`.
    pub fn extent(&self, line: u32, down: bool, over: u8) -> u32 {
        let n = if down { H } else { W };
        let at = |k| {
            if down {
                self.alpha(line, k)
            } else {
                self.alpha(k, line)
            }
        };
        (0..n).filter(|&k| at(k) > over).count() as u32
    }
}

/// One frame of `layers` (each into its own slot) on a transparent target, as the compositor draws it: shapes decoded
/// since the last frame go in first, a strict frame loads what it misses, then the slots draw in order.
pub fn render(
    dev: &RenderDevice,
    pass: &mut CursorPass,
    layers: &[CursorLayer],
    strict: bool,
) -> Frame {
    let (device, queue) = (&dev.device, &dev.queue);
    let mut encoder = device.create_command_encoder(&Default::default());
    pass.begin_frame();
    pass.upload_ready(device, queue);
    let mut ranges = Vec::new();
    for l in layers {
        if strict {
            pass.ensure(device, queue, l);
        }
        ranges.extend(pass.prepare(l, (W, H), strict));
    }
    let texture = target(device, W, H);
    draw_into(&mut encoder, &texture, |rp| {
        for r in &ranges {
            pass.draw(rp, *r);
        }
    });
    pass.finish_frame(queue);
    Frame(read_back(dev, encoder, &texture))
}

pub fn target(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cursor test target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// One render pass on `texture`, cleared transparent, recorded by `f`.
pub fn draw_into(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    f: impl FnOnce(&mut wgpu::RenderPass<'_>),
) {
    let view = texture.create_view(&Default::default());
    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("cursor test"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    f(&mut rp);
}

/// Submits `encoder` with a copy of `texture` (`W` wide) and waits for its pixels.
pub fn read_back(
    dev: &RenderDevice,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let (w, h) = (texture.width(), texture.height());
    let buffer = dev.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cursor test read back"),
        size: u64::from(w * h * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    dev.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    dev.device.poll(wgpu::PollType::Wait).unwrap();
    rx.recv().unwrap().unwrap();
    slice.get_mapped_range().to_vec()
}

/// A take's folder in `dir` (`take<k>/`) with solid shapes `(width, height, RGBA)` as `cursors/<i>.png`: the take's
/// key (its cursor file's path) and each shape's look, its hotspot at the image's top left.
pub fn take_with(
    dir: &Path,
    k: usize,
    shapes: &[(u32, u32, [u8; 4])],
) -> (Arc<str>, Vec<CursorDraw>) {
    let folder = dir.join(format!("take{k}"));
    std::fs::create_dir_all(folder.join("cursors")).unwrap();
    let take: Arc<str> = folder.join("cursor.jsonl").to_str().unwrap().into();
    let looks = shapes
        .iter()
        .enumerate()
        .map(|(i, &(w, h, rgba))| {
            let png = folder.join("cursors").join(format!("{i}.png"));
            image::RgbaImage::from_pixel(w, h, image::Rgba(rgba))
                .save(&png)
                .unwrap();
            CursorDraw::Shape {
                take: take.clone(),
                shape: i as u16,
                file: png.to_str().unwrap().into(),
                hotspot: (0.0, 0.0),
                aspect: w as f32 / h as f32,
            }
        })
        .collect();
    (take, looks)
}

/// What `preload` takes for `looks`.
pub fn preload_list(looks: &[CursorDraw]) -> Vec<(Arc<str>, u16, Arc<str>)> {
    looks
        .iter()
        .filter_map(|l| match l {
            CursorDraw::Shape {
                take, shape, file, ..
            } => Some((take.clone(), *shape, file.clone())),
            _ => None,
        })
        .collect()
}

/// The heaviest slot: a smear of 4 heights (16 taps) over four rings.
pub fn busy(look: CursorDraw, at: (f64, f64), height: f64, aspect: f64) -> CursorLayer {
    let mut c = layer(look, at, height);
    (c.blur, c.press) = ((4.0 * height / aspect, 0.0), 0.9);
    for (k, r) in c.ripples.iter_mut().enumerate() {
        *r = Ripple {
            center: at,
            radius: height * (0.5 + 0.5 * k as f64),
            alpha: 0.5,
            color: [255, 255, 255, 255],
        };
    }
    c.ripple_count = 4;
    c
}
