// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b:
// crates/timestamp/src/macos.rs and crates/timestamp/src/win.rs (`from_cpal`), adapted by hand; the re-base follows the
// design of crates/recording/src/sources/capture_clock.rs (no code).
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes:
// - Instants become host-clock ns, the clock `klipolo_platform::clock::host_ns` reads: macOS's mach ticks through the
//   timebase (Cap keeps ticks), Windows' 100 ns QPC units times 100 (Cap converts to raw counter ticks).
// - macOS takes cpal's `callback` instant, the input buffer's `mHostTime`, as the first frame's capture time: cpal's
//   `capture` is that minus one buffer (its own TODO), so it would stamp every block a buffer early.
// - The re-base (`Rebase`) anchors once and keeps the device's own spacing, as Cap's `CaptureClock` does, and also
//   re-anchors when the device clock stops advancing; a reported latency outside 0-1 s counts as none.

//! Host-clock stamps for sound captured on a device's clock. Every source stamps on the host clock
//! (`klipolo_platform::clock::host_ns`): the session clock places samples by it, drift fitting reads it, and the core
//! cuts runs where it shows frames were lost. Sound devices report their own instants: CoreAudio's `mHostTime` (macOS)
//! and WASAPI's QPC positions (Windows) are on the host clock already, but a device whose clock sits more than 5 s from
//! the host's (a driver's bogus epoch) is re-based onto it (`Rebase`), so a stamp is never far in the past or the future.

use std::time::Duration;

/// How far a device's instant may sit from the host clock and still be taken as on it (Cap's `MAX_CAPTURE_CLOCK_SKEW`).
pub const MAX_SKEW_NS: u64 = 5_000_000_000;
/// The longest input latency taken as real; a larger `callback − capture` (a garbage capture instant beside a callback
/// on the host clock) counts as none.
pub const MAX_LATENCY_NS: u64 = 1_000_000_000;

/// The host-clock ns of a capture instant on the host clock, else `now − (callback − capture)` when the device's
/// clock is more than 5 s from the host's.
pub fn capture_host_ns(capture: Duration, callback: Duration, now_host_ns: u64) -> u64 {
    let capture = ns(capture);
    if capture.abs_diff(now_host_ns) <= MAX_SKEW_NS {
        return capture;
    }
    let latency = ns(callback).saturating_sub(capture);
    let latency = if latency > MAX_LATENCY_NS { 0 } else { latency };
    now_host_ns.saturating_sub(latency)
}

/// One stream's blocks on the host clock. A device on the host clock is taken as it is. One that isn't is anchored
/// once, at `capture_host_ns`, and its later blocks keep the device's own spacing from that anchor, so callback jitter
/// never shows as a gap or an overlap; it is anchored again when the mapping drifts more than 5 s from the host clock
/// (a device clock that jumped) or the device clock stops advancing. Real-time safe: no allocation, no lock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rebase {
    /// (device ns, host ns) of the anchor block.
    anchor: Option<(u64, u64)>,
    last: Option<u64>,
}

impl Rebase {
    /// The host-clock ns of a block captured at the device's `capture`, reported at its `callback`, read at `now`.
    pub fn stamp(&mut self, capture: Duration, callback: Duration, now_host_ns: u64) -> u64 {
        let device = ns(capture);
        let advanced = self.last.is_none_or(|last| device > last);
        self.last = Some(device);
        match self.anchor {
            Some((d0, h0)) if advanced => {
                let mapped = h0.saturating_add(device - d0.min(device));
                if device >= d0 && mapped.abs_diff(now_host_ns) <= MAX_SKEW_NS {
                    return mapped;
                }
            }
            Some(_) => {}
            None if device.abs_diff(now_host_ns) <= MAX_SKEW_NS => return device,
            None => {}
        }
        let host = capture_host_ns(capture, callback, now_host_ns);
        self.anchor = Some((device, host));
        host
    }
}

fn ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// cpal's instants of one input callback as host-clock durations: (the block's first frame, when the callback had it).
/// macOS: cpal's `callback` is the buffer's `mHostTime` (mach ticks as ns), the first frame's capture time, and its
/// `capture` is that minus one buffer, so the buffer's length is the latency it reports.
#[cfg(all(feature = "real-capture", target_os = "macos"))]
pub fn from_cpal(ts: &cpal::InputStreamTimestamp) -> (Duration, Duration) {
    use cpal::host::coreaudio::StreamInstantExt;

    #[repr(C)]
    #[derive(Default)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut tb = Timebase::default();
    // SAFETY: fills a local struct (libSystem; reads a cached value, no system call after the first).
    unsafe { mach_timebase_info(&mut tb) };
    let ticks = u128::from(ts.callback.as_host_time());
    let first = (ticks * u128::from(tb.numer) / u128::from(tb.denom.max(1))) as u64;
    let buffer = ts.callback.duration_since(&ts.capture).unwrap_or_default();
    let first = Duration::from_nanos(first);
    (first, first.saturating_add(buffer))
}

/// cpal's instants of one input callback as host-clock durations: WASAPI's capture position (the block's first frame)
/// and its clock's position read at the callback, both QPC in 100 ns units.
#[cfg(all(feature = "real-capture", windows))]
pub fn from_cpal(ts: &cpal::InputStreamTimestamp) -> (Duration, Duration) {
    use cpal::host::wasapi::StreamInstantExt;

    let ns = |i: cpal::StreamInstant| {
        let units = u64::try_from(i.as_performance_counter()).unwrap_or(0);
        Duration::from_nanos(units.saturating_mul(100))
    };
    (ns(ts.capture), ns(ts.callback))
}
