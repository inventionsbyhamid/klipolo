//! A draft's audit objects, kept between its draws, and what a test
//! may ask of a draw's readback.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::oneshot;

use super::Prepared;
use crate::make::audit::Variant;

/// A draft's compiled audit variants (at most two: raw and trips), shared by its frames and its measurement. A draw
/// takes its variant out and puts it back once it has read its target, so two draws at once
/// never share one; a draw that fails drops them all, so a readback left pending never reaches another draw.
#[derive(Default)]
pub struct AuditCache {
    pub(super) prepared: Mutex<Vec<(Variant, Prepared)>>,
    pub(super) made: AtomicU64,
    fail_next: AtomicBool,
    hold_next: Mutex<Option<oneshot::Receiver<()>>>,
}

/// What a test asked of one draw's readback.
#[derive(Default)]
pub(super) struct TestRead {
    pub fail: bool,
    /// The readback waits for this before it is read (it was submitted and its map requested).
    pub hold: Option<oneshot::Receiver<()>>,
}

impl AuditCache {
    /// Variants compiled so far (a test reads it: one per variant, whatever the draws).
    pub fn made(&self) -> u64 {
        self.made.load(Ordering::Relaxed)
    }

    /// The next draw fails as a readback that timed out does, its map left pending (tests).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn fail_next_read(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    /// The next draw's readback waits, once submitted and its map requested, until the sender sends or is dropped
    /// (tests: two draws at once, deterministically).
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn hold_next_read(&self) -> oneshot::Sender<()> {
        let (send, receive) = oneshot::channel();
        *self
            .hold_next
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(receive);
        send
    }

    /// What a test asked of the next draw (nothing, outside tests).
    pub(super) fn take_test(&self) -> TestRead {
        TestRead {
            fail: self.fail_next.swap(false, Ordering::SeqCst),
            hold: self
                .hold_next
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        }
    }
}
