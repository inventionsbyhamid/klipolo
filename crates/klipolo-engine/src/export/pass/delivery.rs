//! The delivery stage: once a frame's GPU work is done, each format that still needs it gets its picture. While a frame
//! waits on a decoder that is still making progress, it also tells the parent the worker is alive (`Heartbeat`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::time::{Interval, MissedTickBehavior};

use super::gpu;
use crate::decode_wait::DecodeBeat;
use crate::export::encode::{EncoderThread, Input, VideoIn};
use crate::export::shared::WorkerError;
use crate::render::export::FrameDone;
use crate::render::yuv::{PlaneLayout, YuvPixels};

/// Where delivery's time goes, per frame (logged to stderr with the drawing stage's). The stages overlap: each
/// figure is the time its stage waited or worked, not a share of the frame.
#[derive(Default)]
pub(super) struct Timing {
    /// Waiting for a frame's GPU work before delivering it.
    pub(super) gpu: f64,
    /// Waiting for room in a full encoder queue.
    pub(super) queue: f64,
}

/// A frame drawn and submitted: its GPU completion, every format's picture and which formats take it.
pub(super) struct Drawn {
    pub(super) index: u64,
    pub(super) done: FrameDone,
    pub(super) pixels: Vec<YuvPixels>,
    /// Bit k: the pass's format k takes it.
    pub(super) wanted: u32,
}

/// Waits for `drawn`'s GPU work, then queues each wanted format's picture. Returns how many formats took it.
pub(super) async fn deliver(
    drawn: Drawn,
    layouts: &[PlaneLayout],
    encoders: &mut [EncoderThread],
    timing: &mut Timing,
) -> Result<u64, WorkerError> {
    let at = Instant::now();
    drawn.done.wait().await.map_err(gpu)?;
    let queued_at = Instant::now();
    timing.gpu += (queued_at - at).as_secs_f64();
    let mut delivered = 0;
    for (k, ((frame, layout), encoder)) in drawn
        .pixels
        .into_iter()
        .zip(layouts)
        .zip(encoders.iter_mut())
        .enumerate()
    {
        if drawn.wanted & (1 << k) == 0 {
            continue;
        }
        let video = match frame {
            #[cfg(target_os = "macos")]
            YuvPixels::Surface(buffer) => VideoIn::Surface(buffer),
            YuvPixels::Readback(mut pending) => {
                VideoIn::Planes(pending.wait().await.map_err(gpu)?, *layout)
            }
        };
        encoder
            .push(Input {
                index: drawn.index,
                video,
            })
            .await?;
        delivered += 1;
    }
    timing.queue += queued_at.elapsed().as_secs_f64();
    Ok(delivered)
}

/// While a frame waits on a decoder that is still making progress, the parent hears from the worker this often: no
/// progress line comes while a frame waits, and the parent's watchdog stops a worker silent for 120 s.
const HEARTBEAT: Duration = Duration::from_secs(5);

/// The delivery stage's clock: every `HEARTBEAT`, whether a frame waited on a progressing decoder since the last.
pub(super) struct Heartbeat {
    every: Interval,
    beat: Arc<DecodeBeat>,
}

impl Heartbeat {
    pub(super) fn new(beat: Arc<DecodeBeat>) -> Self {
        let mut every =
            tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        every.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self { every, beat }
    }

    /// The next beat: true when a frame waited on a decoder that kept making progress since the last one.
    pub(super) async fn due(&mut self) -> bool {
        self.every.tick().await;
        self.beat.take()
    }
}
