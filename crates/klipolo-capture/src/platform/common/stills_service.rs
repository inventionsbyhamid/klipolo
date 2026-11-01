//! How the Mac's stills (`platform::macos::thumbs`) spare the system's capture service, apart so it is tested on every
//! OS:
//! - `InFlight`: one request at a time. A request the stills stopped waiting for still runs in the service, so no new
//!   one is sent until it has answered: a stalled service gets no queue of screenshots and content lists to finish in
//!   a burst when it recovers.
//! - `RoundCache`: one content list a round, counted from the end of its read, a failed read included, so a service
//!   that fails at once isn't asked again at every still.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::pipe::PipeError;

/// One request to the service in flight at a time.
#[derive(Clone, Debug, Default)]
pub struct InFlight(Arc<AtomicBool>);

impl InFlight {
    /// A new request's `Answered`, for its completion handler; `Os` while the last one hasn't answered (the caller's
    /// turn fails as a still that didn't come, and is tried again after its rest).
    pub fn begin(&self) -> Result<Answered, PipeError> {
        if self.0.swap(true, Ordering::AcqRel) {
            return Err(PipeError::Os(
                "the capture service hasn't answered the last request yet".into(),
            ));
        }
        Ok(Answered {
            service: self.0.clone(),
            done: false,
        })
    }
}

/// A request's completion handler holds it: `answer` as the answer comes, before it is passed on. A handler the
/// service lets go of without calling it ends the request too; one let go of after it answered changes nothing (the
/// next request may be in flight by then).
#[derive(Debug)]
pub struct Answered {
    service: Arc<AtomicBool>,
    done: bool,
}

impl Answered {
    pub fn answer(&mut self) {
        if !std::mem::replace(&mut self.done, true) {
            self.service.store(false, Ordering::Release);
        }
    }
}

impl Drop for Answered {
    fn drop(&mut self) {
        self.answer();
    }
}

/// One value a round: read again once `round` has passed since its last read ended, and a failed read's error kept as
/// long.
#[derive(Debug)]
pub struct RoundCache<T> {
    round: Duration,
    got: Option<(Instant, Result<T, PipeError>)>,
}

impl<T> RoundCache<T> {
    pub fn new(round: Duration) -> Self {
        Self { round, got: None }
    }

    /// This round's value, or its read's error; `read` runs when there is none, or the last read ended `round` or more
    /// ago (`now`). The old value goes before the new read.
    pub fn get(
        &mut self,
        now: &dyn Fn() -> Instant,
        read: impl FnOnce() -> Result<T, PipeError>,
    ) -> Result<&mut T, PipeError> {
        let stale = (self.got.as_ref())
            .is_none_or(|(at, _)| now().saturating_duration_since(*at) >= self.round);
        if stale {
            self.got = None;
        }
        let (_, got) = self.got.get_or_insert_with(|| {
            let value = read();
            (now(), value)
        });
        got.as_mut().map_err(|e| e.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn one_request_is_in_flight_until_it_answers() {
        let service = InFlight::default();
        let mut first = service.begin().expect("nothing in flight");
        assert!(service.begin().is_err(), "a second while the first runs");
        first.answer();
        let second = service.begin().expect("the first answered");
        // The first's handler let go of after it answered: the second is still in flight.
        drop(first);
        assert!(service.begin().is_err(), "the second still runs");
        // A handler the service lets go of without calling it ends its request.
        drop(second);
        let mut third = service.begin().expect("the second was let go");
        // One the stills stopped waiting for answers later, on the service's queue: nothing is sent meanwhile. It
        // answers only once this thread has seen that, whatever the machine's load.
        let (go_tx, go_rx) = channel::<()>();
        let (answered_tx, answered_rx) = channel();
        let late = std::thread::spawn(move || {
            go_rx.recv().unwrap();
            third.answer();
            answered_tx.send(()).unwrap();
            third
        });
        assert!(
            service.begin().is_err(),
            "the abandoned one hasn't answered"
        );
        go_tx.send(()).unwrap();
        answered_rx.recv().unwrap();
        let held = late.join().unwrap();
        let fourth = service.begin().expect("it answered");
        drop(held);
        assert!(service.begin().is_err(), "the fourth still runs");
        drop(fourth);
    }

    /// A clock the test moves.
    fn clock() -> (Instant, Cell<Duration>) {
        (Instant::now(), Cell::new(Duration::ZERO))
    }

    #[test]
    fn a_read_serves_a_round_counted_from_its_end() {
        let (base, at) = clock();
        let now = || base + at.get();
        let mut cache = RoundCache::<u32>::new(Duration::from_secs(1));
        let reads = Cell::new(0);
        // A read that takes 1.5 s (the service is slow): its round begins as it ends.
        let slow = || {
            reads.set(reads.get() + 1);
            at.set(at.get() + Duration::from_millis(1_500));
            Ok(reads.get())
        };
        assert_eq!(cache.get(&now, slow).copied(), Ok(1));
        at.set(Duration::from_millis(2_400));
        let again = || -> Result<u32, PipeError> { panic!("read again within the round") };
        assert_eq!(cache.get(&now, again).copied(), Ok(1));
        at.set(Duration::from_millis(2_500));
        let next = || Ok(7);
        assert_eq!(cache.get(&now, next).copied(), Ok(7));
    }

    #[test]
    fn a_failed_read_is_kept_for_its_round() {
        let (base, at) = clock();
        let now = || base + at.get();
        let mut cache = RoundCache::<u32>::new(Duration::from_secs(1));
        let reads = Cell::new(0);
        let fails = || {
            reads.set(reads.get() + 1);
            Err(PipeError::Os("the service failed at once".into()))
        };
        let error = Err(PipeError::Os("the service failed at once".into()));
        assert_eq!(cache.get(&now, fails).copied(), error);
        // 8 stills in the round: the same error, and the service isn't asked again.
        for k in 1..=8 {
            at.set(Duration::from_millis(k * 110));
            assert_eq!(cache.get(&now, fails).copied(), error);
        }
        assert_eq!(reads.get(), 1);
        at.set(Duration::from_millis(1_000));
        assert_eq!(cache.get(&now, || Ok(3)).copied(), Ok(3));
    }
}
