// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/layers/image.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: ImageSegment/ProjectUniforms inputs replaced: Klipolo calls `texture` (Cap's `ensure_texture`, reworked: it reads the file version itself, drops the `reusable` argument, returns the failure message, and in preview never waits: a missing texture is decoded on a blocking thread while the frame draws without it (a replaced file keeps drawing its previous texture), the redraw signal fires when it is ready, and only export waits) and `push_draw` (which also takes the queue) with an `ImageQuad` in output pixels (Cap's `visible_images` geometry minus timing, tracks and flips), then `render_draw` per draw inside its own render pass; `CacheState` became `CacheEntry { ready, job, failed, stale }` and failures keep the user-facing message; `forget`, `begin_frame` and `warm` added; `decode_image` decodes through Klipolo's `render::image_load` (EXIF orientation, HEIC via ImageIO) instead of `image::ImageReader`, premultiplied by ImageIO itself, at the largest side the layer is drawn at (rounded up to 256 px, never above 4096 or the GPU limit; a larger box decodes again) instead of the full image, keeping the version checks; mipmaps are made on the GPU (`render::mips`) instead of by CPU resizes, so `texture_dimensions`, `premultiplied_mips`, `upload_texture` and their tests are gone; each draw slot keeps one uniform buffer (rewritten with `queue.write_buffer` when its quad moves) and the bind groups of the textures it drew in the last two frames (overlay video alternates between two), and drops bind groups of evicted textures, instead of a new buffer and bind group per change; file versions are re-read at most once a second (no per-frame stat or path allocation); MAX_TEXTURE_BYTES 256 → 128 MiB; `ImageUniforms.flips.z` carries the stored frame's clockwise turn (overlay video from phones); `visible_images`, `ActiveTexture`, `prepare`, `has_content`, `has_track`, `render`, `render_track`, the draw tracks, the anyhow errors and the now-unused imports and constants removed; a decode stores its result before firing the redraw signal (no lost wake-up); export waits for any load in flight (a missing, stale or too-small texture), never returning an undersized one; textures drawn in this or the last frame are never evicted, and when they alone exceed the budget new decodes are made at half size (`squeeze`) instead of thrashing; `trim` lets off-screen images go under memory pressure; the overlay warnings name the file, not its full path (the app log never holds folders). Load failures are typed (`image_load::ImageError`: a missing file, or the message to show), so the compositor says a missing image as it says a missing video. `ImageQuad.mirror`, passed as `flips.x` (the shader flips the shown picture before `stored_uv`). `ImageQuad.blend`; one pipeline per fixed-function blend mode (normal, multiply, screen, add; `render::compositor::blend`), made on first use (or by `warm_pipelines`) on one explicit bind group layout, so every pipeline takes every draw's bind group; each draw remembers its pipeline. `push_touch` queues a texture's zero-opacity draw over the first pixel (the compositor scissors it there; one texel a pixel, so its full-size level is sampled, `touch_uniforms`), sharing `push_draw`'s slots (`push_uniforms`); the uniforms carry the output's size and a draw is two triangles over the quad's own bounds (`image.wgsl`), where Cap's one triangle covered the whole output, so a quad shades its own pixels only (the same pixels drawn). Each draw is also scissored to its quad's turned box (`scissor`; a touch to its one pixel), the pass's scissor put back after.

use std::{
    collections::HashMap,
    fs::Metadata,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Instant, SystemTime},
};

use bytemuck::{Pod, Zeroable};
use image::RgbaImage;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use wgpu::util::DeviceExt;

use klipolo_model::motion::Blend;

use crate::render::compositor::blend;
use crate::render::image_load::{self, ImageError};
use crate::render::mips::{MipGenerator, level_count};
use crate::vendor::cap::compat::XY;

const MAX_TEXTURES: usize = 16;
const MAX_TEXTURE_BYTES: u64 = 128 * 1024 * 1024;
/// Klipolo: the longest side decoded (64 MiB of RGBA at most, plus mips).
const MAX_DECODE_SIDE: u32 = 4096;
/// Klipolo: decode sizes are rounded up to this step, so dragging a resize handle doesn't decode on every pixel.
const SIDE_STEP: u32 = 256;
/// Klipolo: how often a cached image's file is looked at again (it may have been replaced).
const RECHECK: std::time::Duration = std::time::Duration::from_secs(1);
pub const MAX_IMAGE_SOURCE_DIMENSION: u32 = 32_768;
pub const MAX_IMAGE_SOURCE_PIXELS: u64 = 64 * 1024 * 1024;
pub const MAX_IMAGE_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_IMAGE_DECODED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
enum FileVersion {
    File {
        length: u64,
        modified: Option<SystemTime>,
        created: Option<SystemTime>,
        #[cfg(unix)]
        identity: (u64, u64, i64, i64),
    },
    Unavailable(ErrorKind),
    NotFile,
}

impl FileVersion {
    fn from_metadata(metadata: &Metadata) -> Self {
        if !metadata.is_file() {
            return Self::NotFile;
        }
        Self::File {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        }
    }

    fn read(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Ok(metadata) => Self::from_metadata(&metadata),
            Err(error) => Self::Unavailable(error.kind()),
        }
    }
}

struct CachedTexture {
    texture: wgpu::Texture,
    byte_len: u64,
    last_used: u64,
    /// Klipolo: the frame that last drew it; a texture the current frame draws is never evicted.
    drawn: u64,
    /// The longer side it was decoded at, and whether that is the whole image (a bigger box gains nothing).
    side: u32,
    full: bool,
}

/// A decode + upload running on a blocking thread.
struct Job {
    handle: JoinHandle<()>,
    side: u32,
    /// The decode's result, stored before the redraw signal fires (so the redraw always finds it).
    done: Arc<Mutex<Option<Result<CachedTexture, ImageError>>>>,
}

impl Job {
    fn take(&self) -> Option<Result<CachedTexture, ImageError>> {
        self.done
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

struct CacheEntry {
    version: FileVersion,
    /// When `version` was last read from disk.
    checked: Instant,
    ready: Option<CachedTexture>,
    job: Option<Job>,
    failed: Option<ImageError>,
    /// `ready` is from an older version of the file (drawn until the new one is decoded).
    stale: bool,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct ImageUniforms {
    center_size: [f32; 4],
    rotation_opacity_radius: [f32; 4],
    flips: [f32; 4],
    /// Klipolo: the output's size in pixels (xy), so the quad covers only its own bounds.
    output: [f32; 4],
}

/// Where one texture lands, in output pixels: its centre and size before rotation, clockwise degrees about the
/// centre, opacity, corner rounding (px), the clockwise quarter turns that make the texture upright (0 for images
/// and text; phone video frames are stored turned), whether it is shown as in a mirror (a mirrored video layer), and
/// how it mixes with what is under it (Klipolo: overlay never reaches `push_draw`, the compositor's read-below pass
/// draws it).
#[derive(Clone, Copy, Debug)]
pub struct ImageQuad {
    pub center: XY<f64>,
    pub size: XY<f64>,
    pub rotation_deg: f64,
    pub opacity: f64,
    pub rounding: f64,
    pub source_rotation: u16,
    pub mirror: bool,
    pub blend: Blend,
}

impl ImageQuad {
    /// Cap's `visible_images` geometry checks (finite, positive, on screen) and uniforms, for one quad.
    fn uniforms(&self, output_size: (u32, u32)) -> Option<ImageUniforms> {
        if !self.opacity.is_finite()
            || self.opacity <= 0.0
            || !self.rotation_deg.is_finite()
            || !self.rounding.is_finite()
        {
            return None;
        }
        let center_size = [
            self.center.x as f32,
            self.center.y as f32,
            self.size.x as f32,
            self.size.y as f32,
        ];
        if center_size
            .iter()
            .any(|value| !value.is_finite() || value.abs() > 1.0e7)
            || center_size[2] <= 0.0
            || center_size[3] <= 0.0
        {
            return None;
        }
        let (sin, cos) = (self.rotation_deg % 360.0).to_radians().sin_cos();
        let (sin, cos) = (sin as f32, cos as f32);
        let half_width = (cos.abs() * center_size[2] + sin.abs() * center_size[3]) * 0.5;
        let half_height = (sin.abs() * center_size[2] + cos.abs() * center_size[3]) * 0.5;
        if center_size[0] + half_width < 0.0
            || center_size[1] + half_height < 0.0
            || center_size[0] - half_width > output_size.0 as f32
            || center_size[1] - half_height > output_size.1 as f32
        {
            return None;
        }
        Some(ImageUniforms {
            center_size,
            rotation_opacity_radius: [
                cos,
                sin,
                self.opacity.clamp(0.0, 1.0) as f32,
                (self.rounding as f32)
                    .max(0.0)
                    .min(center_size[2].min(center_size[3]) * 0.5),
            ],
            flips: [
                f32::from(u8::from(self.mirror)),
                0.0,
                f32::from(self.source_rotation),
                0.0,
            ],
            output: [output_size.0 as f32, output_size.1 as f32, 0.0, 0.0],
        })
    }
}

struct CachedDraw {
    buffer: wgpu::Buffer,
    uniforms: ImageUniforms,
    /// Bind groups of the textures drawn in this slot recently (at most two), with the frame each was last used.
    groups: Vec<(wgpu::Texture, wgpu::BindGroup, u64)>,
}

/// Decode size for a layer drawn `side` pixels long: rounded up to `SIDE_STEP`, at most `max_dimension`.
fn decode_side(side: u32, max_dimension: u32) -> u32 {
    side.max(1)
        .div_ceil(SIDE_STEP)
        .saturating_mul(SIDE_STEP)
        .min(MAX_DECODE_SIDE)
        .min(max_dimension)
}

fn decode_image(path: &Path, expected: &FileVersion, side: u32) -> Result<RgbaImage, ImageError> {
    let unreadable = || ImageError::Message(image_load::unreadable(path));
    let metadata = image_load::check(path)?;
    if FileVersion::from_metadata(&metadata) != *expected {
        return Err(unreadable());
    }
    if metadata.len() > MAX_IMAGE_SOURCE_BYTES {
        return Err(unreadable());
    }
    let decoded = image_load::decode_premultiplied(path, side)?;
    if FileVersion::read(path) != *expected {
        return Err(unreadable());
    }
    Ok(decoded)
}

/// Uploads level 0 and makes the mips on the GPU.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mips: &MipGenerator,
    rgba: &RgbaImage,
    side: u32,
) -> CachedTexture {
    let (width, height) = rgba.dimensions();
    let levels = level_count(width, height);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Image overlay texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    mips.generate(device, queue, &texture);
    let byte_len = (0..levels)
        .map(|level| u64::from((width >> level).max(1)) * u64::from((height >> level).max(1)) * 4)
        .sum();
    CachedTexture {
        texture,
        byte_len,
        last_used: 0,
        drawn: 0,
        side,
        full: width.max(height) < side,
    }
}

pub struct ImageLayer {
    shader: wgpu::ShaderModule,
    /// Klipolo: explicit, so the blend pipelines share it (and every draw's bind group).
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    /// Klipolo: one per `blend::FIXED` mode; normal is made in `new`, the others on first use.
    pipelines: [Option<wgpu::RenderPipeline>; 4],
    sampler: wgpu::Sampler,
    /// This frame's draws: a bind group, the pipeline it draws with, and the box it may touch (x, y, w, h) in its
    /// output of the size after it.
    draws: Vec<(wgpu::BindGroup, usize, [u32; 4], (u32, u32))>,
    cached_draws: Vec<CachedDraw>,
    cache: HashMap<PathBuf, CacheEntry>,
    counter: u64,
    /// Klipolo: frames begun, for draw-slot bind group ageing.
    frame: u64,
    mips: Arc<MipGenerator>,
    /// Klipolo: told when a texture finished loading, so the preview draws again.
    redraw: Arc<Notify>,
    /// Klipolo: the textures the current frame draws don't fit the budget: new decodes are made at half size until
    /// they do, instead of evicting what's on screen (and decoding it again next frame).
    squeeze: bool,
}

impl ImageLayer {
    pub fn new(device: &wgpu::Device, redraw: Arc<Notify>) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/image.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Image overlay bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Image overlay pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let mut layer = Self {
            shader,
            layout,
            pipeline_layout,
            pipelines: [None, None, None, None],
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Image overlay sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            draws: Vec::new(),
            cached_draws: Vec::new(),
            cache: HashMap::new(),
            counter: 0,
            frame: 0,
            mips: MipGenerator::new(device),
            redraw,
            squeeze: false,
        };
        layer.ensure_pipeline(device, blend::pipeline_index(Blend::Normal));
        layer
    }

    /// Klipolo: the pipeline of `blend::FIXED[index]`, made now if it isn't yet.
    fn ensure_pipeline(&mut self, device: &wgpu::Device, index: usize) {
        if self.pipelines[index].is_some() {
            return;
        }
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Image overlay pipeline"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(blend::state(blend::FIXED[index])),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        self.pipelines[index] = Some(pipeline);
    }

    /// Klipolo: makes every blend mode's pipeline now (the GPU warm-up), so no frame waits for one.
    pub fn warm_pipelines(&mut self, device: &wgpu::Device) {
        for index in 0..blend::FIXED.len() {
            self.ensure_pipeline(device, index);
        }
    }

    /// Evicts least recently used textures until `byte_len` more fits, never `keep`'s and never one on screen (drawn
    /// in this frame or the last). False: what's left doesn't fit (then new decodes are made smaller, `squeeze`).
    fn make_room(&mut self, byte_len: u64, keep: &Path) -> bool {
        loop {
            let mut count = 0;
            let mut used = 0;
            let mut oldest: Option<(&Path, u64)> = None;
            for (path, entry) in &self.cache {
                if let Some(texture) = &entry.ready {
                    count += 1;
                    used += texture.byte_len;
                    // Drawn this frame or the last: the frame being drawn may still need it.
                    let pinned = path == keep || texture.drawn + 1 >= self.frame;
                    if !pinned && oldest.is_none_or(|(_, last)| texture.last_used < last) {
                        oldest = Some((path, texture.last_used));
                    }
                }
            }
            if count < MAX_TEXTURES && used + byte_len <= MAX_TEXTURE_BYTES {
                return true;
            }
            let Some((path, _)) = oldest else {
                return false;
            };
            let path = path.to_path_buf();
            self.evict(&path);
        }
    }

    fn evict(&mut self, path: &Path) {
        if let Some(evicted) = self.cache.get_mut(path).and_then(|e| e.ready.take()) {
            // Draw slots must not keep the evicted texture alive.
            for draw in &mut self.cached_draws {
                draw.groups.retain(|(t, _, _)| *t != evicted.texture);
            }
        }
    }

    /// Klipolo: under memory pressure, lets go of every image not on screen (drawn in this frame or the last) and stops
    /// their loads.
    pub fn trim(&mut self) {
        let frame = self.frame;
        let idle: Vec<PathBuf> = self
            .cache
            .iter()
            .filter(|(_, e)| e.ready.as_ref().is_none_or(|t| t.drawn + 1 < frame))
            .map(|(p, _)| p.clone())
            .collect();
        for path in idle {
            self.evict(&path);
            if let Some(job) = self.cache.get_mut(&path).and_then(|e| e.job.take()) {
                job.handle.abort();
            }
        }
    }

    /// Starts decoding the image at `path` for a layer drawn `side` pixels long, unless it is cached big enough or
    /// already loading (the playback lookahead calls this about a second before the layer shows).
    pub fn warm(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &Path, side: u32) {
        let _ = self.poll(device, queue, path, side);
    }

    /// The uploaded texture of the image at `path` to draw `side` pixels long. Preview (`wait` false) never waits:
    /// `Ok(None)` while the first decode runs (the redraw signal fires when it is done); a replaced or too-small
    /// texture keeps drawing until its successor is ready. Export (`wait` true) waits for the decode. Errors are the
    /// message to show; a failed file isn't decoded again until it changes.
    pub async fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
        side: u32,
        wait: bool,
    ) -> Result<Option<wgpu::Texture>, ImageError> {
        loop {
            let texture = self.poll(device, queue, path, side)?;
            let frame = self.frame;
            let entry = self.cache.get_mut(path).expect("polled above");
            if let Some(t) = entry.ready.as_mut() {
                t.drawn = frame;
            }
            // A load in flight means the texture is missing, stale or smaller than needed: export waits for it.
            let Some(job) = entry.job.as_mut().filter(|_| wait) else {
                return Ok(texture);
            };
            let _ = (&mut job.handle).await;
            let done = job
                .take()
                .unwrap_or_else(|| Err(ImageError::Message(image_load::unreadable(path))));
            self.install(path, done);
        }
    }

    /// One non-blocking step of `texture`: reread the file version (once a second), take a finished decode, start
    /// one if needed, and return what can be drawn now.
    fn poll(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
        side: u32,
    ) -> Result<Option<wgpu::Texture>, ImageError> {
        let now = Instant::now();
        if !self.cache.contains_key(path) {
            self.cache.insert(
                path.to_path_buf(),
                CacheEntry {
                    version: FileVersion::read(path),
                    checked: now,
                    ready: None,
                    job: None,
                    failed: None,
                    stale: false,
                },
            );
        }
        let entry = self.cache.get_mut(path).expect("inserted above");
        if now.duration_since(entry.checked) >= RECHECK {
            entry.checked = now;
            let version = FileVersion::read(path);
            if entry.version != version {
                entry.version = version;
                entry.failed = None;
                entry.stale = entry.ready.is_some();
                if let Some(job) = entry.job.take() {
                    job.handle.abort();
                }
            }
        }
        if let Some(message) = &entry.failed {
            return Err(message.clone());
        }
        if !matches!(entry.version, FileVersion::File { .. }) {
            tracing::warn!(file = %crate::log_name(path), version = ?entry.version, "Image overlay is unavailable");
            // Not found or not a file: missing (said as a missing video is); another error: unreadable.
            let error = match entry.version {
                FileVersion::Unavailable(ErrorKind::NotFound) | FileVersion::NotFile => {
                    ImageError::Missing(path.to_path_buf())
                }
                _ => ImageError::Message(image_load::unreadable(path)),
            };
            entry.failed = Some(error.clone());
            entry.ready = None;
            return Err(error);
        }
        if let Some(done) = entry.job.as_ref().and_then(Job::take) {
            self.install(path, done);
        }
        let max_dimension = device.limits().max_texture_dimension_2d;
        let side = decode_side(if self.squeeze { side / 2 } else { side }, max_dimension);
        let entry = self.cache.get_mut(path).expect("inserted above");
        if let Some(message) = &entry.failed {
            return Err(message.clone());
        }
        let small = entry
            .ready
            .as_ref()
            .is_some_and(|t| !t.full && t.side < side);
        let loading_enough = entry.job.as_ref().is_some_and(|job| job.side >= side);
        if (entry.ready.is_none() || entry.stale || small) && !loading_enough {
            if let Some(job) = entry.job.take() {
                job.handle.abort();
            }
            let side = entry.ready.as_ref().map_or(side, |t| t.side.max(side));
            let (device, queue) = (device.clone(), queue.clone());
            let (mips, redraw) = (self.mips.clone(), self.redraw.clone());
            let (owned, version) = (path.to_path_buf(), entry.version.clone());
            let done = Arc::new(Mutex::new(None));
            let result = done.clone();
            let handle = tokio::task::spawn_blocking(move || {
                let loaded = decode_image(&owned, &version, side)
                    .map(|rgba| upload(&device, &queue, &mips, &rgba, side));
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(loaded);
                // After the result is stored: the redraw it triggers always finds it.
                redraw.notify_one();
            });
            entry.job = Some(Job { handle, side, done });
        }
        self.counter = self.counter.saturating_add(1);
        let counter = self.counter;
        Ok(entry.ready.as_mut().map(|t| {
            t.last_used = counter;
            t.texture.clone()
        }))
    }

    /// Takes a finished decode into the cache (evicting others to fit), or records its failure.
    fn install(&mut self, path: &Path, done: Result<CachedTexture, ImageError>) {
        match done {
            Ok(texture) => {
                // Over budget with only on-screen textures left: keep this one and make new decodes smaller.
                self.squeeze = !self.make_room(texture.byte_len, path);
                if let Some(entry) = self.cache.get_mut(path) {
                    entry.job = None;
                    entry.stale = false;
                    entry.ready = Some(texture);
                }
            }
            Err(error) => {
                tracing::warn!(file = %crate::log_name(path), %error, "Failed to decode image overlay");
                if let Some(entry) = self.cache.get_mut(path) {
                    entry.job = None;
                    entry.ready = None;
                    entry.failed = Some(error);
                }
            }
        }
    }

    /// Drops every cached image (the project closed or changed).
    pub fn forget(&mut self) {
        for (_, entry) in self.cache.drain() {
            if let Some(job) = entry.job {
                job.handle.abort();
            }
        }
        self.cached_draws.clear();
        self.draws.clear();
    }

    /// Starts a frame: the draws pushed from now on are numbered from 0. Slots let go of textures they haven't
    /// drawn in the last two frames, so a slot never keeps an evicted image or an old text texture alive.
    pub fn begin_frame(&mut self) {
        self.draws.clear();
        self.frame += 1;
        let frame = self.frame;
        for draw in &mut self.cached_draws {
            draw.groups.retain(|(_, _, used)| used + 2 >= frame);
        }
    }

    /// Queues `texture` drawn as `quad` in an output of `output_size` and returns its draw index for `render_draw`,
    /// or None when the quad is invisible or off screen. Each slot keeps its uniform buffer and its last two bind groups.
    pub fn push_draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        quad: ImageQuad,
        output_size: (u32, u32),
    ) -> Option<usize> {
        let uniforms = quad.uniforms(output_size)?;
        let rect = scissor(&quad, output_size);
        self.push_uniforms(
            device,
            queue,
            texture,
            uniforms,
            (blend::pipeline_index(quad.blend), rect, output_size),
        )
    }

    /// Klipolo: queues `texture` drawn at zero opacity from the output's first pixel, one texel a pixel
    /// (the pass scissors it to that pixel, `touch_uniforms`): its first sampling, at its full-size level, a frame or more
    /// before it shows, with nothing changed on screen.
    pub fn push_touch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        output_size: (u32, u32),
    ) -> Option<usize> {
        let uniforms = touch_uniforms((texture.width(), texture.height()), output_size);
        // Its one pixel (each draw's own scissor, as the pass's own).
        let rect = [0, 0, output_size.0.min(1), output_size.1.min(1)];
        self.push_uniforms(
            device,
            queue,
            texture,
            uniforms,
            (blend::pipeline_index(Blend::Normal), rect, output_size),
        )
    }

    /// `push_draw`'s slot: its uniform buffer and its last two bind groups, kept; drawn with `pipeline`, scissored to
    /// `rect` in an output of `output_size`.
    fn push_uniforms(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        uniforms: ImageUniforms,
        (pipeline, rect, output_size): (usize, [u32; 4], (u32, u32)),
    ) -> Option<usize> {
        self.ensure_pipeline(device, pipeline);
        let index = self.draws.len();
        if self.cached_draws.len() == index {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Image overlay uniforms"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            self.cached_draws.push(CachedDraw {
                buffer,
                uniforms,
                groups: Vec::with_capacity(2),
            });
        }
        let draw = &mut self.cached_draws[index];
        if draw.uniforms != uniforms {
            queue.write_buffer(&draw.buffer, 0, bytemuck::bytes_of(&uniforms));
            draw.uniforms = uniforms;
        }
        let frame = self.frame;
        let group = match draw.groups.iter_mut().find(|(t, _, _)| t == texture) {
            Some((_, group, used)) => {
                *used = frame;
                group.clone()
            }
            None => {
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Image overlay bind group"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: draw.buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(
                                &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                if draw.groups.len() == 2 {
                    draw.groups.remove(0);
                }
                draw.groups.push((texture.clone(), group.clone(), frame));
                group
            }
        };
        self.draws.push((group, pipeline, rect, output_size));
        Some(index)
    }

    /// Draws queued draw `index` (from `push_draw` this frame), premultiplied, blended over what is already drawn: only
    /// over its quad's box (Klipolo), the pass's scissor put back after.
    pub fn render_draw(&self, pass: &mut wgpu::RenderPass<'_>, index: usize) {
        let Some((group, pipeline, [x, y, w, h], out)) = self.draws.get(index) else {
            return;
        };
        let Some(pipeline) = &self.pipelines[*pipeline] else {
            return;
        };
        if *w == 0 || *h == 0 {
            return;
        }
        pass.set_scissor_rect(*x, *y, *w, *h);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        // Klipolo: two triangles over the quad's own bounds (`image.wgsl`), not one over the output.
        pass.draw(0..6, 0..1);
        pass.set_scissor_rect(0, 0, out.0, out.1);
    }
}

/// Klipolo: a touch's quad (`ImageLayer::push_touch`): the texture at one texel a pixel
/// from the output's top left corner, upright, at zero opacity. The pass scissors it to the first pixel, which so
/// samples the texture's full-size level (a mipmapped title's level 0, where the whole texture squeezed into the pixel
/// sampled its smallest).
fn touch_uniforms(texture: (u32, u32), output_size: (u32, u32)) -> ImageUniforms {
    let (w, h) = (texture.0.max(1) as f32, texture.1.max(1) as f32);
    ImageUniforms {
        center_size: [w / 2.0, h / 2.0, w, h],
        rotation_opacity_radius: [1.0, 0.0, 0.0, 0.0],
        flips: [0.0; 4],
        output: [output_size.0 as f32, output_size.1 as f32, 0.0, 0.0],
    }
}

/// The pixels `quad` may touch in an output of `out` (x, y, w, h): its turned box, a pixel wider each way, within the
/// output (Klipolo). The quad's shader draws nothing outside its box, so outside it the draw is skipped,
/// not changed: a layer covering a corner of the frame no longer runs its shader over every pixel of it.
fn scissor(quad: &ImageQuad, out: (u32, u32)) -> [u32; 4] {
    let (sin, cos) = (quad.rotation_deg % 360.0).to_radians().sin_cos();
    let (w, h) = (quad.size.x.abs(), quad.size.y.abs());
    let half = [
        (cos.abs() * w + sin.abs() * h) / 2.0 + 1.0,
        (sin.abs() * w + cos.abs() * h) / 2.0 + 1.0,
    ];
    let span = |c: f64, half: f64, of: u32| {
        let lo = (c - half).floor().clamp(0.0, f64::from(of));
        let hi = (c + half).ceil().clamp(0.0, f64::from(of));
        if lo.is_finite() && hi > lo {
            (lo as u32, (hi - lo) as u32)
        } else {
            (0, 0)
        }
    };
    let (x, w) = span(quad.center.x, half[0], out.0);
    let (y, h) = span(quad.center.y, half[1], out.1);
    [x, y, w, h]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A touch samples a texture's full-size level. Its quad is the texture at one
    /// texel a pixel from the output's corner (the pass scissors it to the first pixel), so that pixel samples level 0
    /// of a mipmapped title, not its smallest level as when the whole texture was squeezed into it. Nothing shows:
    /// its opacity is 0.
    #[test]
    fn a_touch_samples_the_full_size_level() {
        let u = touch_uniforms((3000, 400), (3840, 2160));
        assert_eq!(
            u.center_size,
            [1500.0, 200.0, 3000.0, 400.0],
            "one texel a pixel"
        );
        assert_eq!(
            u.rotation_opacity_radius,
            [1.0, 0.0, 0.0, 0.0],
            "upright, opacity 0"
        );
        assert_eq!(u.output, [3840.0, 2160.0, 0.0, 0.0]);
    }
}
