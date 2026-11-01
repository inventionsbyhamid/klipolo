//! The disk outlook while recording: warn below 10 GB free or below 30 minutes
//! left at the measured write rate; stop and save at the 2 GB floor. Time left is counted above the floor, since the
//! take stops there. The free space comes from `durable::free_space` (Cap polls every 2 s; the session's tick is 1 s);
//! the rate from `RateMeter`, over the last 30 s. Pure: every time is an argument.

use std::collections::VecDeque;

use klipolo_model::coded::ErrorParam;

use crate::english::code;
use crate::health::HealthNote;
use crate::shell::Level;

pub const WARN_BYTES: u64 = 10_000_000_000;
pub const STOP_BYTES: u64 = 2_000_000_000;
pub const WARN_MINUTES: u32 = 30;
/// The rate is measured over this much recording.
pub const RATE_WINDOW_NS: u64 = 30_000_000_000;
/// `RateMeter` keeps one sample per this much time, so it stays small however often it is fed.
const RATE_STEP_NS: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskLevel {
    Ok,
    /// Below 10 GB, or below 30 minutes at this rate: `capture.health.diskLow`.
    Warn,
    /// At or below the floor: stop and save (`capture.stop.disk`).
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskOutlook {
    pub level: DiskLevel,
    /// Whole minutes until the floor at this rate (`u32::MAX` with no rate: nothing is being written).
    pub minutes_left: u32,
}

impl DiskOutlook {
    /// `capture.health.diskLow` ("About {minutes} min of space left.") for a warning with a measured rate. The session
    /// gives it when the level changes, not every tick.
    pub fn note(&self) -> Option<HealthNote> {
        (self.level == DiskLevel::Warn && self.minutes_left != u32::MAX).then(|| HealthNote {
            code: code!("capture.health.diskLow"),
            source: None,
            params: vec![("minutes", ErrorParam::from(self.minutes_left))],
            level: Level::Warning,
        })
    }
}

/// `bytes_per_s` is the measured rate (`RateMeter`), or the preset's own until there is one.
pub fn outlook(free_bytes: u64, bytes_per_s: f64) -> DiskOutlook {
    let above = free_bytes.saturating_sub(STOP_BYTES) as f64;
    let minutes_left = if bytes_per_s > 0.0 && bytes_per_s.is_finite() {
        // `as` saturates: a huge disk at a trickle reads as u32::MAX minutes.
        (above / bytes_per_s / 60.0).floor() as u32
    } else {
        u32::MAX
    };
    let level = if free_bytes <= STOP_BYTES {
        DiskLevel::Stop
    } else if free_bytes < WARN_BYTES || minutes_left < WARN_MINUTES {
        DiskLevel::Warn
    } else {
        DiskLevel::Ok
    };
    DiskOutlook {
        level,
        minutes_left,
    }
}

/// The take's write rate over the last 30 s, from the writer's running total. `at_ns` is on a clock that leaves
/// paused time out (nothing is written while paused, and a pause mustn't read as a slow rate). Memory: about 32
/// samples, one a second, however often it is fed; O(1) amortised per `add`.
#[derive(Clone, Debug, Default)]
pub struct RateMeter {
    /// (at, total bytes), at least a second apart except the newest; the oldest at or before the window's start.
    samples: VecDeque<(u64, u64)>,
}

impl RateMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `total_bytes` written by `at_ns`. A total that goes down starts again; a time that goes back is ignored.
    pub fn add(&mut self, at_ns: u64, total_bytes: u64) {
        if let Some(&(at, total)) = self.samples.back() {
            if at_ns < at {
                return;
            }
            if total_bytes < total {
                self.samples.clear();
            }
        }
        let n = self.samples.len();
        if n >= 2 && self.samples[n - 1].0 - self.samples[n - 2].0 < RATE_STEP_NS {
            self.samples[n - 1] = (at_ns, total_bytes);
        } else {
            self.samples.push_back((at_ns, total_bytes));
        }
        let start = at_ns.saturating_sub(RATE_WINDOW_NS);
        while self.samples.len() >= 2 && self.samples[1].0 <= start {
            self.samples.pop_front();
        }
    }

    /// None until two samples span some time.
    pub fn bytes_per_s(&self) -> Option<f64> {
        let (&(t0, b0), &(t1, b1)) = (self.samples.front()?, self.samples.back()?);
        (t1 > t0).then(|| (b1 - b0) as f64 / ((t1 - t0) as f64 / 1e9))
    }
}
