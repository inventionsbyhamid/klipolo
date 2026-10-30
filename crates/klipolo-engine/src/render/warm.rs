//! The GPU warm-up. The first project opened after an install or update waited seconds while Metal
//! compiled the renderer's shaders. The app starts this at launch (`main`), before Home shows, on a thread of its own
//! (no main-thread work): it makes the GPU device and hands a backend on it over at once (`GpuBackend::coming`), so the
//! engine's first start takes it without waiting and its first frame opens its decoders meanwhile. The thread then
//! makes the compositor with what a typical project's first frame draws with (the NV12 video composite, background,
//! image quad and text, and the native presenter's copy in Metal's shader cache), at user-initiated QoS, and hands it
//! over: the first frame draws as soon as it is there, without waiting for the rest.
//!
//! The rest (effects, generators and transitions, video without an IOSurface and its YUV converters, overlay video's
//! scaler, the socket's NV12 conversion: `GpuBackend::warm_all`) then compiles at utility QoS on a device of its own
//! and is dropped: it stays in Metal's shader cache, and the compositor makes each from it the first time a frame
//! needs it (about 40 ms for all of them, against 4 s cold on the Intel Mac). Its own device keeps those compiles'
//! wgpu error scopes (one stack per device in wgpu 25) away from the engine's frames. Holding the backend costs about
//! 8 MB from launch.
//!
//! It reads no project, opens no sound, never touches capture and holds no power assertion: if the Mac sleeps, it
//! carries on afterwards. A warm-up that fails (no usable GPU) hands back nothing, and the engine's own start reports
//! the problem as before; one that fails after handing the backend over leaves the compositor to the engine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::sync::{Notify, oneshot};

use super::GpuBackend;
use super::compositor::Compositor;
use super::device::RenderDevice;
use super::mode::RenderMode;
use crate::qos::{self, Qos};

/// A warm-up in progress, or with its backend handed over and waiting to be taken.
pub struct Warmup {
    done: oneshot::Receiver<Option<GpuBackend>>,
    /// The engine took the backend (logged: whether an open came before the first frame's pipelines).
    waited: Arc<AtomicBool>,
}

impl Warmup {
    /// Starts warming on a thread of its own, for a backend whose compositors draw in `mode` (the app's preview:
    /// `Interactive`; no default). Returns at once.
    pub fn start(mode: RenderMode) -> Self {
        let (tx, done) = oneshot::channel();
        let waited = Arc::new(AtomicBool::new(false));
        let flag = waited.clone();
        let spawned = std::thread::Builder::new()
            .name("klipolo-gpu-warm".into())
            .spawn(move || warm(&flag, tx, mode));
        if let Err(error) = spawned {
            // The sender went with the closure: `finish` resolves to None and the engine starts as before.
            tracing::warn!(%error, "GPU warm-up thread");
        }
        Self { done, waited }
    }

    /// The backend, as soon as its device exists (None if the warm-up failed); its compositor follows once a first
    /// frame's pipelines exist (`GpuBackend::coming`), and the rest keep compiling on the warm-up's thread.
    pub async fn finish(self) -> Option<GpuBackend> {
        self.waited.store(true, Ordering::SeqCst);
        self.done.await.ok().flatten()
    }
}

fn warm(waited: &AtomicBool, tx: oneshot::Sender<Option<GpuBackend>>, mode: RenderMode) {
    let started = Instant::now();
    // What the first frame needs runs at the class of work someone is about to wait for: it is all off the main
    // thread, and Home's own drawing is user-interactive.
    qos::set_current(Qos::UserInitiated);
    let Some(dev) = device() else {
        let _ = tx.send(None);
        return;
    };
    let device_ms = ms(started);
    let redraw = Arc::new(Notify::new());
    let (give, coming) = oneshot::channel();
    if tx
        .send(Some(GpuBackend::coming(
            dev.clone(),
            redraw.clone(),
            coming,
            mode,
        )))
        .is_err()
    {
        return;
    }
    crate::present::native::warm(&dev.device);
    // The backend's own mode, as every compositor it gets.
    let mut compositor = Compositor::new(dev.clone(), redraw, mode);
    compositor.warm_first_frame();
    let first_frame_ms = ms(started);
    let waited_then = waited.load(Ordering::SeqCst);
    tracing::info!(
        device_ms,
        first_frame_ms,
        waited = waited_then,
        "GPU warm-up: first frame pipelines ready"
    );
    drop(dev);
    if give.send(compositor).is_err() {
        // The backend went (the app is quitting, or its device was lost).
        return;
    }
    // Nobody waits for the rest.
    qos::set_current(Qos::Utility);
    let Some(own) = device() else { return };
    let mut primer = GpuBackend::with_device(own, mode);
    primer.warm_all();
    drop(primer);
    tracing::info!(
        device_ms,
        first_frame_ms,
        total_ms = ms(started),
        waited = waited_then,
        "GPU warm-up done"
    );
}

/// A device (wgpu's native futures are ready when first polled; nothing here runs inside another executor).
fn device() -> Option<Arc<RenderDevice>> {
    futures::executor::block_on(RenderDevice::new())
        .map_err(|error| tracing::warn!(%error, "GPU warm-up: no device"))
        .ok()
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
