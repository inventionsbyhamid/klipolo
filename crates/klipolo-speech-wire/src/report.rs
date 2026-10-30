//! A worker's request in flight, as its engine and its stdin reader see it:
//! whether it was aborted, and when its last `progress` line went out, so a line goes out at most every `EVERY_MS`.
//! Only atomics: an engine's callbacks (whisper's, from its decoding threads) read and write it with no lock and no
//! allocation. Each `engine::serve` owns one, shared only with its own stdin reader, so two loops never see each
//! other's aborts. How far a request is decoded is the engine's own business (whisper's frame counters stay in
//! klipolo-speech).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// The least time between two `progress` lines of one request: at most 4 a second.
pub const EVERY_MS: u64 = 250;

/// The request in flight and the worker's abort state.
pub struct Report {
    id: AtomicU64,
    /// The highest id an abort named (0: none). Ids only grow and the app waits only on its newest request, so every
    /// request up to it is one the app gave up on, aborted or queued: a second abort never un-aborts the first, and a
    /// late abort never reaches a later request.
    abort_id: AtomicU64,
    /// Stdin closed: the app is gone, so whatever runs stops.
    closing: AtomicBool,
    /// When the last line went out, in ms since `epoch`.
    said_ms: AtomicU64,
    epoch: Instant,
}

impl Report {
    pub fn new() -> Self {
        Self {
            id: AtomicU64::new(0),
            abort_id: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            said_ms: AtomicU64::new(0),
            epoch: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Request `id` begins; its first `progress` line goes out now (`serve` says it), so the next is due in `EVERY_MS`.
    pub fn begin(&self, id: u64) {
        self.said_ms.store(self.now_ms(), Ordering::SeqCst);
        self.id.store(id, Ordering::SeqCst);
    }

    pub fn id(&self) -> u64 {
        self.id.load(Ordering::Relaxed)
    }

    /// An abort for request `id` (the reader thread).
    pub fn abort(&self, id: u64) {
        self.abort_id.fetch_max(id, Ordering::SeqCst);
    }

    /// Stdin closed (the reader thread).
    pub fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }

    /// Request `id` must stop: an abort named it or a later request, or the app is gone.
    pub fn stopped(&self, id: u64) -> bool {
        id <= self.abort_id.load(Ordering::Relaxed) || self.closing.load(Ordering::Relaxed)
    }

    /// Whether a line may go out now (`EVERY_MS` since the last), taking the turn when it may, so two decoding threads
    /// never both say it.
    pub fn due(&self) -> bool {
        let now = self.now_ms();
        let last = self.said_ms.load(Ordering::Relaxed);
        now >= last + EVERY_MS
            && self
                .said_ms
                .compare_exchange(last, now, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
    }
}

impl Default for Report {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_one_line_goes_out_each_250_ms() {
        let r = Report::new();
        r.begin(1);
        assert!(!r.due(), "the first line went out as it began");
        let mut said = 0;
        let start = Instant::now();
        while start.elapsed().as_millis() < 1_100 {
            said += usize::from(r.due());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!((3..=5).contains(&said), "{said} lines in 1.1 s");
    }

    #[test]
    fn an_abort_stops_its_request_and_those_before_never_a_later_one() {
        let r = Report::new();
        r.begin(4);
        r.abort(3);
        assert!(!r.stopped(4), "a late abort for the request before");
        r.abort(4);
        assert!(r.stopped(4));
        assert!(!r.stopped(5), "nor the next one");
        // Pause, Resume, Pause within one encoder pass: request 4 still runs when 5 is aborted.
        r.abort(5);
        r.abort(3);
        assert!(r.stopped(4), "a second abort never un-aborts the first");
        assert!(r.stopped(5), "and the queued request never runs");
        assert!(!r.stopped(6));
        r.close();
        assert!(r.stopped(6), "the app is gone");
    }
}
