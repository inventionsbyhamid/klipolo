//! How long the computer slept, by the wall clock: the host clock stops during sleep on macOS, so the
//! shell measures `PowerEvent::Woke { slept_ms }` with `SystemTime` between the OS's will-sleep and did-wake notices.
//! The session stops and saves a take after a long sleep (`run::inputs::LONG_SLEEP_MS`). Every OS's shell can use it.

use std::time::SystemTime;

use crate::shell::PowerEvent;

/// The wall time the computer went to sleep, until it wakes.
#[derive(Clone, Copy, Debug, Default)]
pub struct SleepClock {
    since: Option<SystemTime>,
}

impl SleepClock {
    /// The computer is going to sleep at `at`.
    pub fn will_sleep(&mut self, at: SystemTime) -> PowerEvent {
        self.since = Some(at);
        PowerEvent::WillSleep
    }

    /// The computer woke at `at`: how long it slept. A wake with no sleep seen, or a clock set back, counts as 0.
    pub fn woke(&mut self, at: SystemTime) -> PowerEvent {
        let slept = (self.since.take()).and_then(|since| at.duration_since(since).ok());
        PowerEvent::Woke {
            slept_ms: slept.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn woke_reports_the_wall_clock_sleep() {
        let mut clock = SleepClock::default();
        let went = SystemTime::UNIX_EPOCH + Duration::from_secs(1_760_600_000);
        assert_eq!(clock.will_sleep(went), PowerEvent::WillSleep);
        let back = went + Duration::from_secs(11 * 60);
        assert_eq!(clock.woke(back), PowerEvent::Woke { slept_ms: 660_000 });
        // A wake with no sleep seen says nothing slept.
        assert_eq!(clock.woke(back), PowerEvent::Woke { slept_ms: 0 });
    }
}
