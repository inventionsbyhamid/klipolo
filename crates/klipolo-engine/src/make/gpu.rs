//! The worker's GPU: its own device, made on the first request that draws and kept warm, with the
//! worker's limits (`DeviceOptions::WORKER`: timestamps when the adapter has them, textures at most 4096 px, buffers
//! 256 MiB), and its transparent `Headless`. No adapter, a GPU the driver won't open, or a panic in the graphics stack
//! while the device is made (metal-rs panics on some missing GPU classes) is a clean failure the worker
//! answers with, never a crash; the device's queue is exercised once before anything is drawn.

use std::sync::Arc;
use std::time::Duration;

use crate::headless::{Headless, HeadlessOptions};
use crate::registry::Registry;
use crate::render::device::{DeviceOptions, RenderDevice};
use crate::render::shared_fonts::SharedFonts;

/// What the worker says when it has no GPU it can use.
pub const NO_GPU: &str = "This computer's graphics card couldn't be used to check the effect.";
/// How long the first empty submission may take before the device counts as unusable.
const QUEUE_ANSWERS_WITHIN: Duration = Duration::from_secs(5);

/// The worker's device, made off the async threads, any panic caught.
pub async fn worker_device() -> Result<Arc<RenderDevice>, String> {
    let made = tokio::task::spawn_blocking(|| {
        futures::executor::block_on(RenderDevice::with_options(DeviceOptions::WORKER))
    })
    .await;
    let dev = match made {
        Ok(Ok(dev)) => dev,
        Ok(Err(e)) => {
            crate::make::say!("render worker: no device: {e}");
            return Err(NO_GPU.into());
        }
        Err(e) => {
            crate::make::say!("render worker: making the device failed: {e}");
            return Err(NO_GPU.into());
        }
    };
    exercise(&dev).await?;
    Ok(dev)
}

/// One empty submission, which must complete.
async fn exercise(dev: &RenderDevice) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    dev.queue.submit([dev
        .device
        .create_command_encoder(&Default::default())
        .finish()]);
    dev.queue.on_submitted_work_done(move || {
        let _ = tx.send(());
    });
    dev.poll_soon();
    match tokio::time::timeout(QUEUE_ANSWERS_WITHIN, rx).await {
        Ok(Ok(())) if !dev.is_lost() => Ok(()),
        _ => {
            crate::make::say!("render worker: the device's queue didn't answer");
            Err(NO_GPU.into())
        }
    }
}

/// The worker's `Headless` on a new device, transparent, texts shaped with `fonts`.
pub async fn worker_headless(fonts: Option<SharedFonts>) -> Result<Headless, String> {
    let dev = worker_device().await?;
    let mut h = Headless::with_device(
        dev,
        Registry::builtin(),
        HeadlessOptions { transparent: true },
    );
    if let Some(fonts) = fonts {
        h.set_fonts(fonts);
    }
    Ok(h)
}
