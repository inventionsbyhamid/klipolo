//! How the Mac's stills (`platform::macos::thumbs`) spare the system's capture service, apart so it is tested on every
//! OS:
//! - `RoundCache`: one content list a round, counted from the end of its read, a failed read included, so a service
//!   that fails at once isn't asked again at every still.

use std::time::{Duration, Instant};

use crate::pipe::PipeError;

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

    use super::*;

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
