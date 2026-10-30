//! The host clock: raw, monotonic nanoseconds on the clock the OS's display and audio stacks stamp with, so presented
//! times compare with it directly.
//! - macOS: `CLOCK_UPTIME_RAW` (`mach_absolute_time`), what `MTLDrawable.presentedTime` and `presentDrawable:atTime:`
//!   use;
//! - Windows: `QueryPerformanceCounter` (the DXGI and WASAPI clock);
//! - Linux: `CLOCK_MONOTONIC`.

/// The host clock in ns.
pub fn host_ns() -> u64 {
    crate::imp::host_ns()
}

/// A clock tests can replace (`fake::FakeClock`).
pub trait Clock: Send + Sync {
    fn now_ns(&self) -> u64;
}

/// The real host clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostClock;

impl Clock for HostClock {
    fn now_ns(&self) -> u64 {
        host_ns()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeClock;

    #[test]
    fn the_host_clock_only_moves_forward_and_a_fake_one_moves_when_told() {
        let mut last = host_ns();
        assert!(last > 0);
        for _ in 0..1_000 {
            let now = HostClock.now_ns();
            assert!(now >= last);
            last = now;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(host_ns() - last >= 1_000_000, "it counts nanoseconds");

        let fake = FakeClock::new(5);
        fake.advance(10);
        assert_eq!(fake.now_ns(), 15);
        fake.set(3);
        assert_eq!(fake.now_ns(), 3);
    }
}
