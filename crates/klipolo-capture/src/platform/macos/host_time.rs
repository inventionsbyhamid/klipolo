//! Core Media times on the host clock as host ns (`klipolo_platform::clock::host_ns`, `CLOCK_UPTIME_RAW`):
//! ScreenCaptureKit stamps its buffers on `CMClockGetHostTimeClock`, and our encoder stamps frames at 1 ns a tick.

use cidre::cm;
use klipolo_platform::clock::host_ns;

/// A presentation time as host ns.
pub(super) fn host_ns_of(t: cm::Time) -> u64 {
    if t.scale == 1_000_000_000 {
        return t.value.max(0) as u64;
    }
    (i128::from(t.value) * 1_000_000_000 / i128::from(t.scale.max(1))).max(0) as u64
}

/// How far Core Media's host time clock reads from ours, in ns (checked once per stream, logged): ScreenCaptureKit's
/// stamps are only comparable with the mic's and the session clock if this is about 0.
pub(super) fn host_clock_skew_ns() -> i64 {
    let theirs = host_ns_of(cm::Clock::host_time_clock().time());
    i64::try_from(theirs).unwrap_or(i64::MAX) - i64::try_from(host_ns()).unwrap_or(i64::MAX)
}
