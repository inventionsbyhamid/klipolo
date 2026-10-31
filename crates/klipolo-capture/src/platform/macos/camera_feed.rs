//! What a camera pipe does with each buffer, on the session's queue, never blocking it: count it, put its
//! time on the host clock, copy it into our pool on the GPU and hand it to the encoder thread; and how a suspended
//! camera counts as blank pictures (`Suspension`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, OnceLock};

use klipolo_platform::clock::host_ns;

use super::camera::{Delivered, OutputDelegateCallback, SessionClock};
use super::handoff::{Shared, ToEncoder};
use super::host_time::host_ns_of;
use super::pool::SurfacePool;
use crate::platform::common::camera::suspended_blanks;

/// What the session's queue does with each buffer.
pub(super) fn feed(
    shared: Arc<Shared>,
    mut pool: SurfacePool,
    to_encoder: SyncSender<ToEncoder>,
    clock: Arc<OnceLock<SessionClock>>,
) -> OutputDelegateCallback {
    let mut first = true;
    Box::new(move |delivered| {
        if shared.stopped.load(Ordering::Acquire) {
            return;
        }
        shared.delivered.fetch_add(1, Ordering::Relaxed);
        let sample = match delivered {
            Delivered::Frame(sample) => sample,
            Delivered::Dropped => {
                shared.dropped_encoder.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        let Some(image) = sample.image_buf() else {
            return;
        };
        shared.complete.fetch_add(1, Ordering::Relaxed);
        // Before the session's clock is known (the first moments of a start) a frame can't be placed.
        let Some(clock) = clock.get() else { return };
        if shared.paused.load(Ordering::Acquire) {
            return;
        }
        let at = clock.host_ns(sample.pts());
        if std::mem::take(&mut first) {
            let (own_ns, now_ns) = (host_ns_of(sample.pts()), host_ns());
            tracing::info!(
                own_ns,
                at,
                now_ns,
                "camera: the first frame's time, its own and on the host clock"
            );
        }
        if let Some(frame) = shared.copy(&mut pool, image) {
            shared.send(&to_encoder, frame, at);
        }
    })
}

/// A suspended camera's blank pictures (`suspended_blanks`): those of earlier suspensions and of the current one.
#[derive(Default)]
pub(super) struct Suspension {
    /// When the current suspension began (host ns); 0 while not suspended.
    since: AtomicU64,
    counted: AtomicU64,
}

impl Suspension {
    pub(super) fn set(&self, suspended: bool, now: u64) {
        if suspended {
            let _ =
                (self.since).compare_exchange(0, now.max(1), Ordering::AcqRel, Ordering::Acquire);
        } else {
            let since = self.since.swap(0, Ordering::AcqRel);
            if since > 0 {
                (self.counted).fetch_add(suspended_blanks(since, now), Ordering::Relaxed);
            }
        }
    }

    pub(super) fn blanks(&self, now: u64) -> u64 {
        let since = self.since.load(Ordering::Acquire);
        let current = if since > 0 {
            suspended_blanks(since, now)
        } else {
            0
        };
        self.counted.load(Ordering::Relaxed) + current
    }
}
