//! The offscreen effect stage. It runs a layer's shader passes, ping-ponging between pooled targets, and hands
//! back a texture the compositor places with the image quad. Layers without effects never come here: the
//! single-pass path stays. A pipeline is made once per shader key, after naga has validated the source, so a
//! broken shader names its line in the log. Its effect is then skipped (the layer draws without it) and reported once.
//!
//! Once warm a frame allocates nothing here: targets, uniform slots and the bind groups of the textures passes
//! read (targets, images, text, overlay video, the frame under an adjustment, a transition's two sides) are kept and
//! reused; targets go when idle or over budget, bind groups shortly after their texture was last used.
//!
//! Agent-made (`Checked`) shaders are drawn as the stage's `RenderMode` says: in the app's preview their
//! pipelines compile off the drawing thread (`compile_thread`) and draw only after the crash guard's handshake
//! (`handshake`), and their passes are sampled and slowed when over budget (`sampling`); in an export a slow one is drawn
//! in tiles (`tiles`). Every error scope goes through the device's lock (`scopes`).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::params::{ParamKind, ParamSpec, PointN};

use crate::plan::MissingContrib;
use crate::registry::{ShaderEffect, ShaderKind};
use crate::render::device::RenderDevice;
use crate::render::mode::RenderMode;

pub mod compile_thread;
mod draw;
mod frame_info;
mod guarded;
pub mod handshake;
mod inputs;
mod padding;
mod passes;
mod pipeline_cache;
mod pipelines;
pub mod sampling;
pub mod scopes;
mod statics;
mod targets;
pub mod tiles;
mod watch;

pub use frame_info::FrameInfo;
pub(crate) use frame_info::frame_bytes;
pub use handshake::GuardEvent;
use inputs::{External, PairInput};
pub use padding::{Inner, PadLook, Padding};
pub use passes::{Withheld, protects};
use pipelines::{UniformSlot, input_group};
use targets::Target;
pub(crate) use targets::clamped as target_size;
pub use targets::{
    MAX_FRAME_TARGET_BYTES, MAX_PROTECTED_TARGET_BYTES, PixelRect, effect_target_peak,
    reset_effect_target_peak,
};
pub(crate) use watch::count_pass;
pub use watch::pass_pixels;

/// Effect targets: premultiplied, gamma-encoded BT.709, like the frame.
pub const EFFECT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The largest target side; bigger layers are processed at this size and scaled by the quad.
pub const MAX_EFFECT_SIDE: u32 = 4096;
/// Bind groups of outside textures are kept this many frames after their last use (the native presenter cycles
/// three slots, the readback session two).
const KEEP_EXTERNAL_FRAMES: u64 = 4;
/// Copies the frame drawn so far from `origin` (pixels) for an adjustment layer: exact texels, no filtering.
const GRAB: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    return textureLoad(kl_a, vec2<i32>(floor(uv * kl.size + params.origin)), 0);
}";
/// A layer's texture at the size its effects run at, as their first pass (`EffectStage::resample`): an image decoded
/// bigger than it's drawn is sampled from the mip level that fits (trilinear), so effects that read texels at the
/// drawn size don't alias; a texture stored turned `turn`° clockwise (phone video) is turned upright, and a mirrored
/// layer flipped (`mirror`), so effects see the picture as it's shown. `stored`
/// follows the image quad's flip and then its `stored_uv`. `turn` is 0 for the
/// half-size step (`halve`), which then only samples down.
const RESAMPLE: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let shown = select(uv, vec2<f32>(1.0 - uv.x, uv.y), params.mirror == 1i);
    var stored = shown;
    var source = vec2<f32>(textureDimensions(kl_a));
    if params.turn == 90i {
        stored = vec2<f32>(shown.y, 1.0 - shown.x);
        source = source.yx;
    } else if params.turn == 180i {
        stored = vec2<f32>(1.0) - shown;
    } else if params.turn == 270i {
        stored = vec2<f32>(1.0 - shown.y, shown.x);
        source = source.yx;
    }
    let lod = max(log2(max(source.x / kl.size.x, source.y / kl.size.y)), 0.0);
    return textureSampleLevel(kl_a, kl_samp, stored, lod);
}";

/// Targets, uniform slots, input bind groups and pipelines made so far, by every stage in the process.
static MADE: AtomicU64 = AtomicU64::new(0);

/// How many GPU objects effect stages have made (a diagnostic: once warm, drawing a frame makes none; the perf
/// harness checks it).
pub fn gpu_objects_made() -> u64 {
    MADE.load(Ordering::Relaxed)
}

/// Counts one GPU object made (the scene drawer's too, `render::scene`).
pub(crate) fn made() {
    MADE.fetch_add(1, Ordering::Relaxed);
}

/// What the first pass reads.
#[derive(Clone, Copy)]
pub enum Input<'a> {
    /// One of this stage's targets.
    Target(usize),
    /// Any other texture (an image, text or overlay video texture).
    Texture(&'a wgpu::Texture),
    /// A view of another texture (the frame under an adjustment).
    View(&'a wgpu::TextureView),
    /// Two of this stage's targets, as `kl_a` and `kl_b` (a transition's outgoing and incoming sides).
    Pair(usize, usize),
    /// The frame drawn so far (`kl_a`) and a layer's texture (`kl_b`): an overlay-blended layer's read-below pass.
    Under(&'a wgpu::TextureView, &'a wgpu::Texture),
    /// Nothing: generators.
    Empty,
}

pub struct EffectStage {
    frame_layout: wgpu::BindGroupLayout,
    input_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    empty_view: wgpu::TextureView,
    empty_input: wgpu::BindGroup,
    /// By shader key and text (`pipeline_cache`).
    pipelines: pipeline_cache::PipelineCache,
    slots: Vec<UniformSlot>,
    used_slots: usize,
    targets: Vec<Target>,
    externals: Vec<External>,
    /// Bind groups of two targets read together (transitions), by the targets' ids.
    pairs: Vec<PairInput>,
    grab: Arc<ShaderEffect>,
    resample: Arc<ShaderEffect>,
    /// The overlay blend's read-below pass (`compositor::blend`).
    overlay: Arc<ShaderEffect>,
    /// Passes drawn since this frame began (`passes`).
    passes: u32,
    /// Uniform slots written since this frame began (`uniform_writes`): a pass whose uniforms didn't change since the
    /// slot's last frame writes nothing.
    uniform_writes: u32,
    /// Contributions already reported missing (until the project closes), so a frame formats no message for them.
    missing_warned: Vec<Arc<str>>,
    /// A frame left effects out for the targets' budget, and said so (until the project closes).
    over_budget_said: bool,
    /// A frame hid a layer whose protective effect couldn't run, and said so (until the project closes).
    withheld_said: bool,
    /// One opaque black texel: drawn over an adjustment's box when its protective effect can't run. Made on first use.
    black: Option<wgpu::Texture>,
    frame: u64,
    /// Targets made so far: each target's `id`.
    targets_made: u64,
    /// Built-ins whose settings allow it run at half size (`ShaderEffect::downsample`); only a test turns it off.
    downsample: bool,
    /// Static and stepped generators' pictures, kept in pooled targets (`statics`).
    statics: statics::GeneratorCache,
    /// Its device: the error-scope lock every compile takes, and the compile thread's device.
    dev: Arc<RenderDevice>,
    mode: RenderMode,
    /// `Interactive`: checked shaders' pipelines, compiled off the drawing thread; made with the first one asked for.
    compiler: Option<compile_thread::PipelineCompiler>,
    /// The checked shaders asked of `compiler`, by key, with their compile mark's request, until it hands them back
    /// (never dropped before).
    asked: HashMap<u64, (Arc<ShaderEffect>, u64)>,
    /// Checked shaders waiting for their compile mark (its request number).
    awaiting: Vec<(u64, Arc<ShaderEffect>)>,
    /// The built-ins queued on the compile thread ahead of the first checked compile, until it
    /// hands them back; and whether they were. The shader presets are queued behind that compile (`presets_asked`).
    builtins_queued: HashMap<u64, Arc<ShaderEffect>>,
    builtins_asked: bool,
    presets_asked: bool,
    /// The newest frame the GPU finished.
    gpu_done: Arc<handshake::GpuDone>,
    /// Fired by the compile thread when a pipeline is ready (the compositor's redraw signal).
    wake: Option<Arc<tokio::sync::Notify>>,
    /// The crash guard's handshake and the events for the app.
    handshake: handshake::Handshake,
    /// Runtime sampling of checked passes (`Interactive` only).
    pass_sampler: sampling::PassSampler,
    /// Effects the sampler slowed down, said once each until the project closes (`frame.agentEffectSlow`).
    slow_said: Vec<klipolo_model::contrib::ContribId>,
    /// Notes for the next frame: checked shaders the compile thread couldn't compile, and effects just slowed down.
    notes: Vec<String>,
    /// The shaders of the registry the preview draws with (`prepare`): their pipelines are never evicted. None until
    /// a registry was given (the export, the worker and tests keep the cap of 32).
    pins: Option<std::collections::HashSet<u64>>,
    /// An export's stage: checked passes predicted over 200 ms draw in tiles (`tiles`).
    tiled: bool,
    /// The submissions checked passes took in the last frame (1 each, or one per tile; tests).
    checked_submissions: u32,
    /// Pipelines compiled on the drawing thread so far (tests).
    sync_compiles: u64,
    /// Targets handed out are marked (`watch.rs`).
    watching: bool,
    /// The render worker times this frame: no generator reuses a kept picture, and no
    /// still scene shows or keeps its held one, so every timed frame, and every copy of a batch,
    /// draws each pass it costs.
    fresh: bool,
}

impl EffectStage {
    /// A stage on `dev` that compiles each shader's pipeline the first time a pass needs it (`ensure_pipeline`; in
    /// `Interactive` mode a checked one on the compile thread); `warm` compiles every built-in one now. The GPU warm-up
    /// compiles them on a device of its own, so here they come from Metal's shader cache.
    pub fn new(dev: &Arc<RenderDevice>, mode: RenderMode) -> Self {
        let device = &dev.device;
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo effect frame"),
            entries: &[
                uniform(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform(2),
            ],
        });
        let input_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo effect input"),
            entries: &[texture(0), texture(1)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Klipolo effect"),
            bind_group_layouts: &[&frame_layout, &input_layout],
            push_constant_ranges: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Klipolo effect"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Only the resample pass reads mip levels; effects read level 0.
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // wgpu zero-initializes textures: this reads as transparent black.
        let empty = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Klipolo effect empty"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: EFFECT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let empty_view = empty.create_view(&wgpu::TextureViewDescriptor::default());
        let empty_input = input_group(device, &input_layout, &empty_view, &empty_view);
        let grab = ContribDef {
            params: vec![ParamSpec {
                key: "origin".into(),
                label: "Origin".into(),
                visible_when: None,
                kind: ParamKind::Point {
                    default: PointN { x: 0.0, y: 0.0 },
                },
            }],
            ..ContribDef::new(
                "klipolo.builtin/internal.grab",
                ContribKind::Effect,
                "Adjustment",
            )
        };
        let grab = ShaderEffect::assemble(&grab, ShaderKind::Filter, GRAB)
            .expect("the grab shader assembles");
        let resample = ContribDef {
            params: vec![
                ParamSpec {
                    key: "turn".into(),
                    label: "Turn".into(),
                    visible_when: None,
                    kind: ParamKind::Int {
                        min: 0,
                        max: 270,
                        default: 0,
                    },
                },
                ParamSpec {
                    key: "mirror".into(),
                    label: "Mirror".into(),
                    visible_when: None,
                    kind: ParamKind::Int {
                        min: 0,
                        max: 1,
                        default: 0,
                    },
                },
            ],
            ..ContribDef::new(
                "klipolo.builtin/internal.resample",
                ContribKind::Effect,
                "Image",
            )
        };
        let resample = ShaderEffect::assemble(&resample, ShaderKind::Filter, RESAMPLE)
            .expect("the resample shader assembles");
        Self {
            frame_layout,
            input_layout,
            pipeline_layout,
            sampler,
            empty_view,
            empty_input,
            pipelines: Default::default(),
            slots: Vec::new(),
            used_slots: 0,
            targets: Vec::new(),
            externals: Vec::new(),
            pairs: Vec::new(),
            grab: Arc::new(grab),
            resample: Arc::new(resample),
            overlay: Arc::new(crate::render::compositor::blend::overlay_shader()),
            passes: 0,
            uniform_writes: 0,
            missing_warned: Vec::new(),
            over_budget_said: false,
            withheld_said: false,
            black: None,
            frame: 0,
            targets_made: 0,
            downsample: true,
            fresh: false,
            statics: statics::GeneratorCache::new(),
            pass_sampler: sampling::PassSampler::new(dev, mode == RenderMode::Interactive),
            dev: dev.clone(),
            mode,
            compiler: None,
            asked: HashMap::new(),
            awaiting: Vec::new(),
            builtins_queued: HashMap::new(),
            builtins_asked: false,
            presets_asked: false,
            gpu_done: Arc::default(),
            wake: None,
            handshake: Default::default(),
            slow_said: Vec::new(),
            notes: Vec::new(),
            pins: None,
            tiled: false,
            checked_submissions: 0,
            sync_compiles: 0,
            watching: false,
        }
    }

    /// Reports each contribution in `missing` (this frame's `FramePlan::missing`) once until the project closes. A
    /// caption style isn't left out but drawn in Clean, which its own coded note says, naming it
    /// (`api::caption_style_missing`); one being checked says so (`api::agent_effect_checking`).
    pub fn note_missing(&mut self, missing: &[MissingContrib], warnings: &mut Vec<String>) {
        for MissingContrib { kind, id, checking } in missing {
            if !self.missing_warned.contains(id) {
                self.missing_warned.push(id.clone());
                warnings.push(if *checking {
                    crate::api::agent_effect_checking(id)
                } else if *kind == ContribKind::CaptionStyle {
                    crate::api::caption_style_missing(id)
                } else {
                    format!(
                        "Something in this project needs a plugin that isn't installed ({id}), so it's left out."
                    )
                });
            }
        }
    }

    /// Starts a frame: slots and targets are handed out again; targets idle for a while or over the budget, and
    /// bind groups of textures no longer drawn, are dropped. Only here: target indices handed out during a frame
    /// stay valid until the next one starts.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.used_slots = 0;
        self.passes = 0;
        self.uniform_writes = 0;
        self.checked_submissions = 0;
        let now = self.frame;
        self.poll_compiles();
        if self.mode == RenderMode::Interactive {
            self.guard_tick();
        }
        for step in self.pass_sampler.begin_frame(&self.dev, now) {
            self.slowed(step);
        }
        self.expire_statics();
        self.evict_targets();
        self.externals
            .retain(|e| now - e.used <= KEEP_EXTERNAL_FRAMES);
        self.evict_pairs(now.saturating_sub(KEEP_EXTERNAL_FRAMES));
    }

    /// Whether passes that allow it run at half size (on by default; a GPU test compares against full size).
    pub(crate) fn set_downsample(&mut self, on: bool) {
        self.downsample = on;
    }

    /// While on, no generator reuses a picture it kept (`generate`), and no still scene shows or keeps a held one
    /// (`held`, `hold`): the render worker's timed frames.
    pub(crate) fn set_fresh(&mut self, on: bool) {
        self.fresh = on;
    }

    /// Drops the pooled targets and bind groups (project closed; window hidden).
    pub fn forget(&mut self) {
        self.targets.clear();
        self.statics.clear();
        self.externals.clear();
        self.pairs.clear();
        self.missing_warned.clear();
        self.over_budget_said = false;
        self.withheld_said = false;
    }

    /// Under memory pressure: only what this frame used is kept. Call it only when no target index of this frame is
    /// still to be used (the compositor does, once every layer is prepared).
    pub fn trim(&mut self) {
        let now = self.frame;
        self.retain_targets(|t| t.used == now);
        self.externals.retain(|e| e.used == now);
        self.evict_pairs(now);
    }

    /// Passes drawn since this frame began (`Compositor::passes_last_frame`).
    pub fn passes(&self) -> u32 {
        self.passes
    }

    /// Uniform slots written since this frame began (`FrameTime::uniform_writes`).
    pub(crate) fn uniform_writes(&self) -> u32 {
        self.uniform_writes
    }

    /// An opaque black texture (one texel), made once: what an adjustment whose protection couldn't run draws over its
    /// box.
    pub fn black(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> &wgpu::Texture {
        self.black.get_or_insert_with(|| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Klipolo effect black"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: EFFECT_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &[0, 0, 0, 255],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            made();
            texture
        })
    }

    /// The overlay blend's read-below pass.
    pub fn overlay_shader(&self) -> &Arc<ShaderEffect> {
        &self.overlay
    }

    pub fn texture(&self, target: usize) -> &wgpu::Texture {
        &self.targets[target].texture
    }

    pub fn view(&self, target: usize) -> &wgpu::TextureView {
        &self.targets[target].view
    }

    /// The target's id: unique among the targets this stage made, while its index changes as others are dropped.
    pub fn id(&self, target: usize) -> u64 {
        self.targets[target].id
    }
}
