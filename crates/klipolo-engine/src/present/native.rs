//! The native presenter: frames go straight from the compositor into a CAMetalLayer under the transparent
//! webview, with no CPU copy.
//! - The compositor renders each frame into a slot texture (`slots.rs`) instead of reading it back.
//! - A presenter thread (`thread.rs`) copies each frame into the layer's next drawable and presents it at its due
//!   time (stills at once). `nextDrawable` is only ever called on that thread, and outside wgpu's locks, so a slow
//!   drawable never holds up the engine.
//!
//! The slot ring, the pacing and the log are the portable half; only macOS has a presenter thread to drain them until
//! Windows and Linux add theirs (a DXGI swap chain, a Wayland subsurface), so elsewhere they are unused.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

mod slots;
#[cfg(target_os = "macos")]
mod thread;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use serde::Serialize;

use super::{Frame, FramePresenter, Output, PresenterKind};
use crate::frames::trailer::FrameMeta;
use crate::platform::NativeSurface;
#[cfg(target_os = "macos")]
use slots::BlitInputs;
pub(crate) use slots::Slots;
pub use slots::{NativeFrame, dropped_total as frames_dropped};

/// Most presents the trial log keeps (about five minutes at 30 fps).
const LOG_CAP: usize = 10_000;

pub(crate) enum Msg {
    /// A frame was queued.
    Queued,
    Visible(bool),
    /// No project is open: let the drawables and what is on screen go.
    Release,
    Stop,
}

/// Told about frames put on screen: every still, the first frames of each run of playback, then every
/// `thread::SHOWN_EVERY`th (the UI's clock, its "live" state and the latency readout), with when it is on screen
/// (epoch ms) and how many frames were presented since the last call.
pub type ShownSink = Arc<dyn Fn(&FrameMeta, f64, u32) + Send + Sync>;

/// One present, for the on-device trial (dev builds turn the log on; it records nothing otherwise).
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentEntry {
    pub at_us: i64,
    pub seq: u32,
    pub playback: bool,
    /// When it was due: its sound is heard then (host ns; 0: a still).
    pub due_ns: u64,
    /// The time it was presented at (`presentDrawable:atTime:`, host ns; 0: at once).
    pub scheduled_ns: u64,
    /// When the copy into the drawable was committed (host ns).
    pub committed_ns: u64,
    /// When Core Animation put it on the glass (host ns; 0: not yet, or never shown).
    pub presented_ns: u64,
    pub sent_ms: f64,
    pub shown_ms: f64,
}

#[derive(Default)]
pub struct PresentLog {
    enabled: AtomicBool,
    entries: Mutex<Vec<PresentEntry>>,
    /// Drawables that didn't come within `nextDrawable`'s one-second timeout.
    pub timeouts: AtomicU64,
}

impl PresentLog {
    /// Starts a fresh log (or stops logging).
    pub fn set_enabled(&self, on: bool) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.enabled.store(on, Ordering::SeqCst);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn entries(&self) -> Vec<PresentEntry> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Records a present; returns its index for `presented`.
    pub(crate) fn push(&self, entry: PresentEntry) -> Option<usize> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        (entries.len() < LOG_CAP).then(|| {
            entries.push(entry);
            entries.len() - 1
        })
    }

    pub(crate) fn presented(&self, index: usize, host_ns: u64) {
        if let Some(entry) = self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(index)
        {
            entry.presented_ns = host_ns;
        }
    }
}

/// The actor's handle on the presenter thread.
pub struct NativeLayerPresenter {
    tx: mpsc::Sender<Msg>,
    slots: Arc<Slots>,
    failed: Arc<AtomicBool>,
    log: Arc<PresentLog>,
}

impl NativeLayerPresenter {
    /// Frames rendered for this presenter go into these slots (the backend leases them).
    pub(crate) fn slots(&self) -> Arc<Slots> {
        self.slots.clone()
    }

    pub fn log(&self) -> Arc<PresentLog> {
        self.log.clone()
    }

    /// Set when the presenter can't go on (the backend sets it when the GPU device was lost).
    pub(crate) fn fail_flag(&self) -> Arc<AtomicBool> {
        self.failed.clone()
    }

    /// Frames that never reached the screen so far.
    pub fn dropped(&self) -> u64 {
        self.slots.dropped()
    }
}

impl NativeLayerPresenter {
    /// Starts presenting into `surface` on `dev`'s GPU.
    pub fn start(
        dev: Arc<crate::render::device::RenderDevice>,
        surface: NativeSurface,
        shown: ShownSink,
    ) -> Result<Self, String> {
        match surface {
            #[cfg(target_os = "macos")]
            NativeSurface::Metal(layer) => Self::start_metal(dev, layer, shown),
            #[cfg(not(target_os = "macos"))]
            NativeSurface::Unsupported(never) => {
                let _ = (dev, shown);
                match never {}
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl NativeLayerPresenter {
    /// Starts presenting into `layer` (made by `macos::install` on the main thread) on `dev`'s GPU.
    fn start_metal(
        dev: Arc<crate::render::device::RenderDevice>,
        layer: super::macos::NativeLayer,
        shown: ShownSink,
    ) -> Result<Self, String> {
        let device = &dev.device;
        let inputs = BlitInputs {
            layout: blit_layout(device),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Klipolo native blit"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        };
        // SAFETY: reads wgpu's MTLDevice handle (retained); the layer's drawables must come from the device wgpu
        // renders on.
        let raw = unsafe {
            device
                .as_hal::<wgpu_hal::api::Metal, _, _>(|d| d.map(|d| d.raw_device().lock().clone()))
        }
        .ok_or("The graphics card isn't a Metal device.")?;
        let pipeline = blit_pipeline(device, &inputs.layout);
        let slots = Slots::new(inputs);
        let failed = Arc::new(AtomicBool::new(false));
        let log = Arc::new(PresentLog::default());
        let tx = thread::spawn(thread::Parts {
            dev,
            device: raw,
            layer,
            pipeline,
            slots: slots.clone(),
            failed: failed.clone(),
            log: log.clone(),
            shown,
        })?;
        Ok(Self {
            tx,
            slots,
            failed,
            log,
        })
    }
}

/// Compiles the pipeline the presenter copies frames with and drops it, so the presenter that starts later on this
/// process's GPU finds it in Metal's shader cache (the GPU warm-up, `render::warm`).
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub fn warm(device: &wgpu::Device) {
    drop(blit_pipeline(device, &blit_layout(device)));
}

#[cfg(not(target_os = "macos"))]
#[doc(hidden)]
pub fn warm(_device: &wgpu::Device) {}

#[cfg(target_os = "macos")]
fn blit_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Klipolo native blit"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

#[cfg(target_os = "macos")]
fn blit_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Klipolo native blit"),
        source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Klipolo native blit"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Klipolo native blit"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: thread::DRAWABLE_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

impl FramePresenter for NativeLayerPresenter {
    fn kind(&self) -> PresenterKind {
        PresenterKind::Native
    }

    fn output(&self, _playback: bool) -> Output {
        Output::Native
    }

    fn present(&mut self, frame: Frame, meta: FrameMeta) {
        if let Frame::Native(frame) = frame
            && self.slots.queue(frame, meta)
        {
            let _ = self.tx.send(Msg::Queued);
        }
    }

    fn set_visible(&mut self, visible: bool) {
        if !visible {
            self.slots.release();
        }
        let _ = self.tx.send(Msg::Visible(visible));
    }

    fn release(&mut self) {
        self.slots.release();
        let _ = self.tx.send(Msg::Release);
    }

    fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

impl Drop for NativeLayerPresenter {
    fn drop(&mut self) {
        // The thread ends on its own; nobody waits for it (it may be inside `nextDrawable` for up to a second).
        let _ = self.tx.send(Msg::Stop);
    }
}
