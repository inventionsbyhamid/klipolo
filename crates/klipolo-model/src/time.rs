//! Integer-microsecond time helpers. All model times use [`Us`].

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Microseconds. Integers so splitting and snapping never drift.
pub type Us = i64;

pub const US_PER_SEC: Us = 1_000_000;

/// Shortest clip or item the model keeps (10 ms).
pub const MIN_DURATION_US: Us = 10_000;

/// Largest time the model stores (24 h). Validation keeps every stored time in
/// `0..=MAX_TIME_US` (recording offsets in `-MAX_TIME_US..=MAX_TIME_US`), so arithmetic on
/// validated times can't overflow.
pub const MAX_TIME_US: Us = 86_400_000_000;

/// Attached items resolved through slowed clips can land up to ceil(0.5 / speed) µs apart from
/// where they were placed (integer rounding at speed < 1). Overlaps this small count as touching.
pub const OVERLAP_TOLERANCE_US: Us = 4;

pub fn secs_to_us(secs: f64) -> Us {
    (secs * US_PER_SEC as f64).round() as Us
}

pub fn us_to_secs(us: Us) -> f64 {
    us as f64 / US_PER_SEC as f64
}

/// Nearest frame boundary at `fps` frames per second.
pub fn snap_to_frame(us: Us, fps: u32) -> Us {
    let fps = f64::from(fps.max(1));
    let frame = (us as f64 * fps / US_PER_SEC as f64).round();
    (frame * US_PER_SEC as f64 / fps).round() as Us
}

/// Timeline length of `source_len` source microseconds played at `speed`.
pub fn scaled(source_len: Us, speed: f64) -> Us {
    (source_len as f64 / speed).round() as Us
}

/// Source length consumed by `timeline_len` microseconds played at `speed`.
pub fn unscaled(timeline_len: Us, speed: f64) -> Us {
    (timeline_len as f64 * speed).round() as Us
}

/// A file's own time over `us` of recording time when its clock runs `ppm` fast (`SourceRun::local_us`'s rate): what a
/// separated recording source's sound layer plays per timeline µs.
pub fn drifted(us: Us, ppm: f64) -> Us {
    if ppm == 0.0 {
        us
    } else {
        (us as f64 * (1.0 + ppm * 1e-6)).round() as Us
    }
}

/// "0:07", "3:00", "1:02:05": a time as people read it.
pub fn clock(us: Us) -> String {
    let total = us.max(0).saturating_add(500_000) / 1_000_000;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Half-open range `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct RangeUs {
    pub start: Us,
    pub end: Us,
}

impl RangeUs {
    pub fn len(&self) -> Us {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Clamp to `[0, limit]`, drop empty ranges, sort, and merge overlapping/touching ranges.
pub fn merge_ranges(ranges: &[RangeUs], limit: Us) -> Vec<RangeUs> {
    let limit = limit.max(0);
    let mut sorted: Vec<RangeUs> = ranges
        .iter()
        .map(|r| RangeUs {
            start: r.start.clamp(0, limit),
            end: r.end.clamp(0, limit),
        })
        .filter(|r| !r.is_empty())
        .collect();
    sorted.sort_by_key(|r| r.start);
    let mut merged: Vec<RangeUs> = Vec::with_capacity(sorted.len());
    for r in sorted {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// Where time `t` lands once the merged, sorted `ranges` are removed and the gaps closed.
pub fn ripple_map(ranges: &[RangeUs], t: Us) -> Us {
    let mut removed: Us = 0;
    for r in ranges {
        if t >= r.end {
            removed = removed.saturating_add(r.len());
        } else if t > r.start {
            return r.start.saturating_sub(removed);
        } else {
            break;
        }
    }
    t.saturating_sub(removed)
}

/// Merged, sorted ranges being removed, with the length removed before each: every lookup is a binary search (perf
/// audit #16), so a batch with R ranges over C clips and L layers costs O((C + L) log R) instead of O((C + L) × R).
/// Memory: three words a range (24 B), 120 KB for 5,000.
#[derive(Clone, Debug)]
pub struct Ripple {
    ranges: Vec<RangeUs>,
    /// `before[i]`: the total length of `ranges[..i]` (one more entry than `ranges`; the last is the whole).
    before: Vec<Us>,
}

impl Ripple {
    /// `merged` must come from [`merge_ranges`] (sorted, never touching).
    pub fn new(merged: &[RangeUs]) -> Self {
        debug_assert!(
            merged.iter().all(|r| !r.is_empty())
                && merged.windows(2).all(|w| w[0].end < w[1].start),
            "Ripple::new takes merged ranges"
        );
        let mut before = Vec::with_capacity(merged.len().saturating_add(1));
        let mut sum: Us = 0;
        before.push(sum);
        for r in merged {
            sum = sum.saturating_add(r.len());
            before.push(sum);
        }
        Self {
            ranges: merged.to_vec(),
            before,
        }
    }

    pub fn ranges(&self) -> &[RangeUs] {
        &self.ranges
    }

    /// The first range that ends after `t`.
    fn after(&self, t: Us) -> usize {
        self.ranges.partition_point(|r| r.end <= t)
    }

    /// [`ripple_map`]: where `t` lands once the ranges are gone.
    pub fn map(&self, t: Us) -> Us {
        let i = self.after(t);
        match self.ranges.get(i) {
            Some(r) if r.start < t => r.start.saturating_sub(self.before[i]),
            _ => t.saturating_sub(self.before[i]),
        }
    }

    /// [`kept_parts`]: the parts of `[start, end)` the ranges leave.
    pub fn kept(&self, start: Us, end: Us) -> Vec<RangeUs> {
        kept_parts(start, end, &self.ranges[self.after(start)..])
    }

    /// The first moment at or after `t` that isn't removed.
    pub fn first_kept(&self, t: Us) -> Us {
        self.ranges
            .get(self.after(t))
            .filter(|r| r.start <= t)
            .map_or(t, |r| r.end)
    }
}

/// Parts of `[start, end)` not covered by the merged, sorted `ranges`.
pub fn kept_parts(start: Us, end: Us, ranges: &[RangeUs]) -> Vec<RangeUs> {
    let mut parts = Vec::new();
    let mut cursor = start;
    for r in ranges {
        if r.end <= cursor {
            continue;
        }
        if r.start >= end {
            break;
        }
        if r.start > cursor {
            parts.push(RangeUs {
                start: cursor,
                end: r.start.min(end),
            });
        }
        cursor = cursor.max(r.end);
        if cursor >= end {
            break;
        }
    }
    if cursor < end {
        parts.push(RangeUs { start: cursor, end });
    }
    parts
}
