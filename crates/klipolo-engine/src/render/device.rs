use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;

use crate::platform::gpu::GpuClass;
use crate::vendor::cap::rendering::create_shared_device_with;

/// What the preview says when the graphics card can't be used; the details go to the log (never raw wgpu text).
pub const GPU_UNAVAILABLE: &str = "The preview can't use the graphics card.";
/// Whether Metal's timestamps are trusted for the render worker's meter. Off until the [Mac] check's
/// `make_mac` on an Apple-silicon Mac shows its timed frames agree with the wall clock; flipping it is a reviewed
/// change. Until then the meter uses the fenced wall clock on Metal.
pub const METAL_TIMESTAMPS_VERIFIED: bool = false;
/// What the preview says when a frame hit a graphics error.
pub const GPU_FAILED: &str = "The preview couldn't draw a frame.";

/// What the app hears about its GPU device: it was lost, or it raised an uncaptured (validation) error.
/// Only the kind: no message text and no stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuReport {
    pub lost: bool,
}

/// The app's hook (the app process only; an export worker sets none, so its GPU loss surfaces as its export's error).
static GPU_REPORT: OnceLock<fn(GpuReport)> = OnceLock::new();

/// Sets the hook both device callbacks report to; the first call wins. Each device reports each kind at most once
/// (an uncaptured error can fire every frame), so a loss after an error is still heard.
pub fn on_gpu_report(f: fn(GpuReport)) {
    let _ = GPU_REPORT.set(f);
}

/// One device's latches, one per kind: each kind's first report goes to the hook, every later one is dropped.
#[derive(Default)]
struct DeviceReports {
    error: AtomicBool,
    lost: AtomicBool,
}

impl DeviceReports {
    fn report(&self, lost: bool) {
        self.report_to(lost, |r| {
            if let Some(hook) = GPU_REPORT.get() {
                hook(r);
            }
        });
    }

    fn report_to(&self, lost: bool, hook: impl FnOnce(GpuReport)) {
        let latch = if lost { &self.lost } else { &self.error };
        if !latch.swap(true, Ordering::AcqRel) {
            hook(GpuReport { lost });
        }
    }
}

/// One wgpu device per process, shared by the compositor (preview) or by every format's compositor (export).
pub struct RenderDevice {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
    pub software: bool,
    /// wgpu's default handler panics on a validation error, which would end the engine's actor; ours records it.
    fault: Arc<Mutex<Option<String>>>,
    lost: Arc<AtomicBool>,
    /// When the device was lost, stamped in its own callback.
    lost_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// The device's error-scope lock: every push … pop pair on this device takes it.
    pub scopes: crate::render::effects::scopes::ScopeLock,
    /// Wakes the poll thread: GPU work was submitted that a caller waits for.
    poll: mpsc::Sender<()>,
}

/// What a device is made with beyond the preview's defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceOptions {
    /// `TIMESTAMP_QUERY` when the adapter has it (the render worker's meter).
    pub timestamps: bool,
    /// `max_texture_dimension_2d`.
    pub max_texture_side: u32,
    /// `max_buffer_size`.
    pub max_buffer: u64,
}

impl DeviceOptions {
    /// The preview's and export's: wgpu's default limits, and timestamp queries when the adapter has them (the app's
    /// preview samples agent-made effects' passes with them; nothing else is optional).
    pub const DEFAULT: Self = Self {
        timestamps: true,
        max_texture_side: 8192,
        max_buffer: 256 << 20,
    };
    /// The render worker's: timestamps when there are any, textures at most 4096 px a side, buffers 256 MiB.
    pub const WORKER: Self = Self {
        timestamps: true,
        max_texture_side: 4096,
        max_buffer: 256 << 20,
    };
}

impl RenderDevice {
    pub async fn new() -> Result<Arc<Self>, String> {
        Self::with_options(DeviceOptions::DEFAULT).await
    }

    /// A device made with `options` (the render worker's: `DeviceOptions::WORKER`).
    pub async fn with_options(options: DeviceOptions) -> Result<Arc<Self>, String> {
        let wanted = if options.timestamps {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let limits = wgpu::Limits {
            max_texture_dimension_2d: options.max_texture_side,
            max_buffer_size: options.max_buffer,
            ..wgpu::Limits::default()
        };
        let shared = create_shared_device_with(wanted, limits)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "no usable GPU");
                GPU_UNAVAILABLE.to_string()
            })?;
        let fault = Arc::new(Mutex::new(None));
        let lost = Arc::new(AtomicBool::new(false));
        // The device's latches, shared by both callbacks: each kind once per device.
        let reported = Arc::new(DeviceReports::default());
        let reported_lost = reported.clone();
        let on_error = fault.clone();
        shared.device.on_uncaptured_error(Box::new(move |e| {
            tracing::error!(error = %e, "wgpu error");
            *on_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.to_string());
            reported.report(false);
        }));
        let on_lost = lost.clone();
        let lost_at: Arc<Mutex<Option<std::time::Instant>>> = Arc::default();
        let stamp = lost_at.clone();
        shared
            .device
            .set_device_lost_callback(move |reason, message| {
                tracing::error!(?reason, %message, "GPU device lost");
                stamp
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert_with(std::time::Instant::now);
                on_lost.store(true, Ordering::SeqCst);
                // `Destroyed` is Klipolo's own `destroy`, never a fault.
                if matches!(reason, wgpu::DeviceLostReason::Unknown) {
                    reported_lost.report(true);
                }
            });
        let poll = spawn_poller(shared.device.clone());
        Ok(Arc::new(Self {
            device: shared.device,
            queue: shared.queue,
            adapter: shared.adapter,
            software: shared.is_software_adapter,
            fault,
            lost,
            lost_at,
            scopes: Default::default(),
            poll,
        }))
    }

    /// Test hook: the device reads as lost, as after a driver reset (`GpuBackend::ensure_device` makes a new one).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn simulate_loss(&self) {
        self.lost_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(std::time::Instant::now);
        self.lost.store(true, Ordering::SeqCst);
    }

    /// When the device was lost (its callback's own time), if it was.
    pub fn lost_at(&self) -> Option<std::time::Instant> {
        *self.lost_at.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks the poll thread to wait for the GPU work submitted so far; its `map_async` and
    /// `on_submitted_work_done` callbacks then run on that thread. Nothing ever spins or sleeps waiting for the GPU.
    pub fn poll_soon(&self) {
        let _ = self.poll.send(());
    }

    /// The first graphics error since the last call, if any (already logged).
    pub fn take_fault(&self) -> Option<String> {
        self.fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The device is gone (driver reset, GPU removed); a new one must be made.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    pub fn adapter_name(&self) -> String {
        self.adapter.get_info().name
    }

    /// Whether frames can be timed on the GPU (`TIMESTAMP_QUERY`).
    pub fn has_timestamps(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
    }

    /// Whether the render worker times frames with the GPU's timestamps: the device has them, and on
    /// Metal only once a run on Apple silicon has verified them (`METAL_TIMESTAMPS_VERIFIED`); else the fenced wall
    /// clock, which can't read short.
    pub fn timestamps_trusted(&self) -> bool {
        self.has_timestamps()
            && (self.adapter.get_info().backend != wgpu::Backend::Metal
                || METAL_TIMESTAMPS_VERIFIED)
    }

    /// What kind of GPU this is (`platform::gpu`).
    pub fn gpu_class(&self) -> GpuClass {
        GpuClass::of(&self.adapter.get_info())
    }

    /// Preview pixel budget: 1280×720 on Intel and other GPUs (measured to hold 30 fps),
    /// 1920×1080 on Apple GPUs.
    pub fn max_preview_pixels(&self) -> u32 {
        if self.gpu_class() == GpuClass::Apple {
            1920 * 1080
        } else {
            1280 * 720
        }
    }

    /// Playback budget over the socket: 960×540 on Intel and other GPUs, where
    /// the copies into the webview cost about as much as the bytes sent; the full budget on Apple GPUs.
    pub fn socket_playback_pixels(&self) -> u32 {
        if self.gpu_class() == GpuClass::Apple {
            self.max_preview_pixels()
        } else {
            960 * 540
        }
    }
}

/// Whether the GPU work submitted when it was armed has finished (`queue.on_submitted_work_done`, completed by the
/// poll thread). Starts finished.
#[derive(Default)]
pub(crate) struct WorkDone {
    pending: AtomicBool,
    notify: Notify,
}

impl WorkDone {
    /// Tracks the work submitted so far.
    pub(crate) fn arm(self: &Arc<Self>, dev: &RenderDevice) {
        self.arm_frame(dev, None);
    }

    /// The same for a preview frame: when it is done, `frame` (that frame's stage number) is written to the stage's
    /// `GpuDone`, in this same callback (always current, at no extra cost).
    pub(crate) fn arm_frame(
        self: &Arc<Self>,
        dev: &RenderDevice,
        frame: Option<(Arc<crate::render::effects::handshake::GpuDone>, u64)>,
    ) {
        self.pending.store(true, Ordering::SeqCst);
        let done = self.clone();
        dev.queue.on_submitted_work_done(move || {
            if let Some((gpu, frame)) = &frame {
                gpu.done(*frame);
            }
            done.pending.store(false, Ordering::SeqCst);
            done.notify.notify_one();
        });
        dev.poll_soon();
    }

    /// Waits until that work is done (a lost device gives up after two seconds rather than hang the caller).
    pub(crate) async fn wait(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while self.pending.load(Ordering::SeqCst) {
            if tokio::time::timeout_at(deadline, self.notify.notified())
                .await
                .is_err()
            {
                tracing::warn!("GPU work not finished after 2 s");
                self.pending.store(false, Ordering::SeqCst);
            }
        }
    }
}

/// One thread per device that blocks in `device.poll(Wait)` while someone waits for submitted work (perf research
/// polling loops, no idle wakeups when nothing is in flight). It ends with the device.
fn spawn_poller(device: wgpu::Device) -> mpsc::Sender<()> {
    let (tx, rx) = mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("klipolo-gpu-poll".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                // Requests that piled up while the last wait ran are covered by this one (it waits for the newest
                // submission).
                while rx.try_recv().is_ok() {}
                if let Err(error) = device.poll(wgpu::PollType::Wait) {
                    tracing::warn!(%error, "GPU poll");
                }
            }
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "GPU poll thread");
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_lost_after_an_error_is_still_reported_once() {
        // The more serious report is never swallowed by an earlier one; each kind still goes once per device.
        let device = DeviceReports::default();
        let mut heard = Vec::new();
        for lost in [false, false, true, true, false] {
            device.report_to(lost, |r| heard.push(r));
        }
        assert_eq!(heard, [GpuReport { lost: false }, GpuReport { lost: true }]);
    }
}
