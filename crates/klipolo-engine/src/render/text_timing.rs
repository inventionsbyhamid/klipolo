//! The text drawers' own time on the render thread, for the perf harness: the CPU a frame spends in
//! the title and caption drawers (preparing their layers and the drawer's frame bookkeeping), and the GPU time of
//! their work in the frame, from timestamp queries: a title built on the render thread (its glyph pass and mipmaps)
//! and every title and caption drawn in the frame's layer passes. Off unless the harness turns it on
//! (`GpuBackend::time_text`); a frame then reads nothing but `Instant::now` and writes no query.
//!
//! GPU timestamps need a device made after `ask_for_timestamps` on an adapter with timestamps inside passes (Metal on
//! Intel and AMD Macs, Vulkan); elsewhere the GPU time is None ("n/a"), never a guess. The wall-time figures the
//! harness printed before (frame submitted until its pixels are back) include the base video and everything else
//! on the queue; these don't.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Timestamps a frame may write (pairs: a start and an end each); past it, the frame's GPU time is None.
const CAPACITY: u32 = 64;
/// Bytes of one resolved timestamp.
const STAMP_BYTES: u64 = 8;

static ASKED: AtomicBool = AtomicBool::new(false);

/// Perf harness: devices made from now on ask for GPU timestamps where the adapter has them (`timestamp_features`).
#[doc(hidden)]
pub fn ask_for_timestamps() {
    ASKED.store(true, Ordering::Relaxed);
}

/// The timestamp features a device asks for (`create_shared_device`): every one the text timing needs, when the
/// harness asked and the adapter has them all; otherwise none (the app's devices never ask).
pub(crate) fn timestamp_features(adapter: wgpu::Features) -> wgpu::Features {
    let all = wgpu::Features::TIMESTAMP_QUERY
        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
    if ASKED.load(Ordering::Relaxed) && adapter.contains(all) {
        all
    } else {
        wgpu::Features::empty()
    }
}

/// What one frame's text drawers cost on the render thread.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextTime {
    /// The drawers' CPU, µs.
    pub cpu_us: f64,
    /// Their GPU time, ms: None where the device has no timestamps (or the frame wrote more than it can hold).
    pub gpu_ms: Option<f64>,
    /// Whether the frame drew a title or a caption.
    pub drew: bool,
}

/// One frame's GPU timestamps: pairs written around the drawers' work, resolved into the frame's own encoder and
/// read back once the frame is done (`read`).
pub(crate) struct Stamps {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// The next free timestamp this frame, and how many the last resolved frame wrote.
    next: AtomicU32,
    resolved: AtomicU32,
    /// Nanoseconds per timestamp tick.
    period: f64,
}

impl Stamps {
    /// None when `device` wasn't made with the timestamp features (`timestamp_features`).
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Arc<Self>> {
        if timestamp_features(device.features()).is_empty() {
            return None;
        }
        let bytes = u64::from(CAPACITY) * STAMP_BYTES;
        Some(Arc::new(Self {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Klipolo text timing"),
                ty: wgpu::QueryType::Timestamp,
                count: CAPACITY,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Klipolo text timing (resolved)"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Klipolo text timing (read back)"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            next: AtomicU32::new(0),
            resolved: AtomicU32::new(0),
            period: f64::from(queue.get_timestamp_period()),
        }))
    }

    pub(crate) fn set(&self) -> &wgpu::QuerySet {
        &self.set
    }

    /// A start and an end timestamp for one piece of work this frame; None once the frame holds `CAPACITY`. Every pair
    /// given must be written (a resolve reads them all).
    pub(crate) fn pair(&self) -> Option<(u32, u32)> {
        // Past `CAPACITY` (the count stays past it): this frame's GPU time is None.
        let at = self.next.fetch_add(2, Ordering::Relaxed);
        (at + 2 <= CAPACITY).then_some((at, at + 1))
    }

    fn begin_frame(&self) {
        self.next.store(0, Ordering::Relaxed);
    }

    /// Resolves this frame's timestamps into the read-back buffer, in the frame's encoder (submitted after every
    /// piece of work they time).
    fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        let used = self.next.load(Ordering::Relaxed);
        self.resolved.store(used, Ordering::Relaxed);
        if used == 0 || used > CAPACITY {
            return;
        }
        encoder.resolve_query_set(&self.set, 0..used, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &self.readback,
            0,
            u64::from(used) * STAMP_BYTES,
        );
    }

    /// The last resolved frame's GPU time, ms (waits for the GPU): the sum of its pairs; None when it held too many.
    fn read(&self, device: &wgpu::Device) -> Option<f64> {
        let used = self.resolved.load(Ordering::Relaxed);
        if used > CAPACITY {
            return None;
        }
        if used == 0 {
            return Some(0.0);
        }
        let slice = self.readback.slice(..u64::from(used) * STAMP_BYTES);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = device.poll(wgpu::PollType::Wait);
        rx.recv().ok()?.ok()?;
        let ticks: u64 = {
            let bytes = slice.get_mapped_range();
            bytes
                .chunks_exact(2 * STAMP_BYTES as usize)
                .map(|pair| {
                    let at =
                        |i: usize| u64::from_le_bytes(pair[i..i + 8].try_into().expect("8 bytes"));
                    at(8).saturating_sub(at(0))
                })
                .sum()
        };
        self.readback.unmap();
        Some(ticks as f64 * self.period / 1e6)
    }
}

/// The compositor's timing of its text drawers (`Compositor::time_text`).
pub(crate) struct TextTiming {
    stamps: Option<Arc<Stamps>>,
    cpu: Duration,
    drew: bool,
    /// The start of the pair a pass mark opened (`mark`).
    open: Option<u32>,
    /// The last frame's CPU and whether it drew text (its GPU time is read on demand).
    last: (Duration, bool),
}

impl TextTiming {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            stamps: Stamps::new(device, queue),
            cpu: Duration::ZERO,
            drew: false,
            open: None,
            last: (Duration::ZERO, false),
        }
    }

    /// The timestamps the title drawer's own builds write into (None without timestamps).
    pub(crate) fn stamps(&self) -> Option<Arc<Stamps>> {
        self.stamps.clone()
    }

    pub(crate) fn begin_frame(&mut self) {
        self.cpu = Duration::ZERO;
        self.drew = false;
        self.open = None;
        if let Some(stamps) = &self.stamps {
            stamps.begin_frame();
        }
    }

    /// Adds the drawers' CPU since `started`.
    pub(crate) fn add_cpu(&mut self, started: Instant) {
        self.cpu += started.elapsed();
    }

    /// The frame drew a title or a caption.
    pub(crate) fn drew(&mut self) {
        self.drew = true;
    }

    /// A text's draws in a layer pass start (`start`) or end: a timestamp each.
    pub(crate) fn mark(&mut self, pass: &mut wgpu::RenderPass<'_>, start: bool) {
        let Some(stamps) = &self.stamps else {
            return;
        };
        if start {
            self.open = stamps.pair().map(|(at, end)| {
                pass.write_timestamp(stamps.set(), at);
                end
            });
        } else if let Some(end) = self.open.take() {
            pass.write_timestamp(stamps.set(), end);
        }
    }

    /// Ends the frame: its timestamps resolved in its encoder.
    pub(crate) fn end_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(stamps) = &self.stamps {
            stamps.resolve(encoder);
        }
        self.last = (self.cpu, self.drew);
    }

    /// Adds the drawers' CPU since `started` to the frame that just ended (their work after its submit).
    pub(crate) fn add_after_frame(&mut self, started: Instant) {
        self.last.0 += started.elapsed();
    }

    /// The last frame's times (waits for the GPU to read its timestamps).
    pub(crate) fn last(&self, device: &wgpu::Device) -> TextTime {
        TextTime {
            cpu_us: self.last.0.as_secs_f64() * 1e6,
            gpu_ms: self.stamps.as_ref().and_then(|s| s.read(device)),
            drew: self.last.1,
        }
    }
}
