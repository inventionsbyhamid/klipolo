//! Drift fitting: an audio run's offset and clock drift from its anchors.
//!
//! Moved unchanged from `klipolo-capture::drift`: the recorder's audio runs and Sync's anchors share it.
//! `fit_shared` fits one device's several files with one slope.
//!
//! PCM carries no timestamps, so once a second an audio run logs an `Anchor`: the session time of one of its frames.
//! An audio device's clock differs from the host's by ±10–100 ppm (100 ppm is 360 ms an hour), so the run's frames are
//! fitted to a line, `frame = rate × (1 + drift) × (t − offset)`, and the engine resamples by that ratio, so sources
//! sources stay within 10 ms of each other after an hour once corrected.
//!
//! The line is ordinary least squares of `frame` on `session_us`, made robust: an anchor whose residual exceeds
//! `max(3 × the median absolute residual, 1 ms of frames)` (a late callback, a bad stamp) is dropped and the line is
//! fitted again. The rejection runs twice, the second time from the refitted line, because in a short run a few late
//! anchors at one end tilt the first line toward themselves, so one pass can keep one of them. The median is over every
//! anchor each time, so the threshold doesn't shrink as anchors are dropped.
//!
//! Cost: O(n) per round with exact integer means and centred moments (no precision loss at an hour's 10⁸ frames), plus
//! two scratch vectors of n (an hour is 3,600 anchors: about 32 KB). It runs once per audio run, when the manifest is
//! derived, never on the capture path.

use crate::time::Us;

/// Fewer anchors than this fit nothing: the caller places the run by its first sample's time with no drift, and marks
/// it approximate.
pub const MIN_ANCHORS: usize = 10;

/// How many times outliers are rejected and the line refitted.
const REJECTION_ROUNDS: usize = 2;

/// An audio run's frame `frame` (0 is the run's first) was captured at `session_us` on the session clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub session_us: Us,
    pub frame: u64,
}

/// A run's placement on the session clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// The session time of frame 0.
    pub offset_us: Us,
    /// How fast the device's clock runs against the host's: positive gives more frames per host second than `rate`.
    pub drift_ppm: f64,
    /// The largest distance of a kept anchor from the line.
    pub max_residual_us: Us,
    /// The anchors the line was fitted to, after rejection.
    pub anchors_used: usize,
}

/// None when there are fewer than 10 anchors (the caller uses the first sample's time and no drift, "approximate").
/// Also None for a rate of 0 or anchors that don't advance in time.
pub fn fit(anchors: &[Anchor], rate: u32) -> Option<Fit> {
    if anchors.len() < MIN_ANCHORS || rate == 0 {
        return None;
    }
    let floor = f64::from(rate) / 1_000.0;
    let mut keep = vec![true; anchors.len()];
    let mut line = Line::fit(anchors, &keep)?;
    let mut residuals = Vec::with_capacity(anchors.len());
    for _ in 0..REJECTION_ROUNDS {
        residuals.clear();
        residuals.extend(anchors.iter().map(|a| line.residual(a).abs()));
        let mid = (residuals.len() - 1) / 2;
        let (_, median, _) = residuals.select_nth_unstable_by(mid, f64::total_cmp);
        let limit = (3.0 * *median).max(floor);
        let mut changed = false;
        for (k, a) in keep.iter_mut().zip(anchors) {
            let now = line.residual(a).abs() <= limit;
            changed |= now != *k;
            *k = now;
        }
        if !changed {
            break;
        }
        line = Line::fit(anchors, &keep)?;
    }
    let max_residual = kept(anchors, &keep)
        .map(|a| line.residual(a).abs())
        .fold(0.0, f64::max);
    let drift_ppm = (line.slope * 1e6 / f64::from(rate) - 1.0) * 1e6;
    let offset_us = line.t_mean - line.f_mean / line.slope;
    let max_residual_us = max_residual / line.slope;
    if !(drift_ppm.is_finite() && offset_us.is_finite() && max_residual_us.is_finite()) {
        return None;
    }
    Some(Fit {
        offset_us: offset_us.round() as Us,
        drift_ppm,
        max_residual_us: max_residual_us.round() as Us,
        anchors_used: kept(anchors, &keep).count(),
    })
}

/// One device's placement over several of its files: one drift, and each file's frame 0 on the session clock.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedFit {
    /// As `Fit::drift_ppm`, shared by every group.
    pub drift_ppm: f64,
    /// Each group's frame 0 on the session clock, in the groups' order.
    pub offsets_us: Vec<Us>,
    /// The largest distance of a kept anchor from its group's line.
    pub max_residual_us: Us,
    /// The anchors kept after rejection, over every group.
    pub anchors_used: usize,
}

/// One slope through several groups of anchors (one device's files), with one intercept per group: the same robust
/// rejection as `fit`, per group (each group's own median residual sets its limit). None when the groups together have
/// fewer than MIN_ANCHORS, a group is empty, the rate is 0, or the anchors don't advance in time within any group.
///
/// The intercepts are free, so the slope comes only from each group's own spread in time: files far apart don't make
/// it surer, longer files do. Cost: O(n) per round, as `fit`.
pub fn fit_shared(groups: &[&[Anchor]], rate: u32) -> Option<SharedFit> {
    let total: usize = groups.iter().map(|g| g.len()).sum();
    if total < MIN_ANCHORS || rate == 0 || groups.iter().any(|g| g.is_empty()) {
        return None;
    }
    let floor = f64::from(rate) / 1_000.0;
    let mut keep: Vec<Vec<bool>> = groups.iter().map(|g| vec![true; g.len()]).collect();
    let mut lines = Shared::fit(groups, &keep)?;
    let mut residuals = Vec::new();
    for _ in 0..REJECTION_ROUNDS {
        let mut changed = false;
        for (g, (anchors, keep)) in groups.iter().zip(&mut keep).enumerate() {
            residuals.clear();
            residuals.extend(anchors.iter().map(|a| lines.residual(g, a).abs()));
            let mid = (residuals.len() - 1) / 2;
            let (_, median, _) = residuals.select_nth_unstable_by(mid, f64::total_cmp);
            let limit = (3.0 * *median).max(floor);
            for (k, a) in keep.iter_mut().zip(anchors.iter()) {
                let now = lines.residual(g, a).abs() <= limit;
                changed |= now != *k;
                *k = now;
            }
        }
        if !changed {
            break;
        }
        lines = Shared::fit(groups, &keep)?;
    }
    let mut max_residual: f64 = 0.0;
    for (g, (anchors, keep)) in groups.iter().zip(&keep).enumerate() {
        for a in kept(anchors, keep) {
            max_residual = max_residual.max(lines.residual(g, a).abs());
        }
    }
    let slope = lines.slope;
    let drift_ppm = (slope * 1e6 / f64::from(rate) - 1.0) * 1e6;
    let offsets: Vec<f64> = lines.means.iter().map(|&(t, f)| t - f / slope).collect();
    let max_residual_us = max_residual / slope;
    if !(drift_ppm.is_finite()
        && max_residual_us.is_finite()
        && offsets.iter().all(|o| o.is_finite()))
    {
        return None;
    }
    Some(SharedFit {
        drift_ppm,
        offsets_us: offsets.iter().map(|o| o.round() as Us).collect(),
        max_residual_us: max_residual_us.round() as Us,
        anchors_used: keep.iter().flatten().filter(|k| **k).count(),
    })
}

/// Lines with one slope (frames per µs) and each group's own centre, `frame = f_mean + slope × (t − t_mean)`.
struct Shared {
    slope: f64,
    means: Vec<(f64, f64)>,
}

impl Shared {
    /// Least squares over the kept anchors, each group about its own means. None for a group with none kept, or a slope
    /// that doesn't rise.
    fn fit(groups: &[&[Anchor]], keep: &[Vec<bool>]) -> Option<Self> {
        let mut means = Vec::with_capacity(groups.len());
        let (mut tt, mut tf) = (0.0, 0.0);
        for (anchors, keep) in groups.iter().zip(keep) {
            let (mut n, mut t_sum, mut f_sum) = (0_i128, 0_i128, 0_i128);
            for a in kept(anchors, keep) {
                n += 1;
                t_sum += i128::from(a.session_us);
                f_sum += i128::from(a.frame);
            }
            if n == 0 {
                return None;
            }
            let (t_mean, f_mean) = (t_sum as f64 / n as f64, f_sum as f64 / n as f64);
            for a in kept(anchors, keep) {
                let dt = a.session_us as f64 - t_mean;
                tt += dt * dt;
                tf += dt * (a.frame as f64 - f_mean);
            }
            means.push((t_mean, f_mean));
        }
        let slope = tf / tt;
        (slope.is_finite() && slope > 0.0).then_some(Self { slope, means })
    }

    /// The anchor's distance from its group's line, in frames.
    fn residual(&self, group: usize, a: &Anchor) -> f64 {
        let (t_mean, f_mean) = self.means[group];
        a.frame as f64 - f_mean - self.slope * (a.session_us as f64 - t_mean)
    }
}

/// `frame = f_mean + slope × (t − t_mean)`, with the slope in frames per µs. Kept centred, so an hour's large values
/// never lose precision.
struct Line {
    slope: f64,
    t_mean: f64,
    f_mean: f64,
}

impl Line {
    /// Least squares over the kept anchors. None for fewer than two, or a line that doesn't rise.
    fn fit(anchors: &[Anchor], keep: &[bool]) -> Option<Self> {
        let (mut n, mut t_sum, mut f_sum) = (0_i128, 0_i128, 0_i128);
        for a in kept(anchors, keep) {
            n += 1;
            t_sum += i128::from(a.session_us);
            f_sum += i128::from(a.frame);
        }
        if n < 2 {
            return None;
        }
        let (t_mean, f_mean) = (t_sum as f64 / n as f64, f_sum as f64 / n as f64);
        let (mut tt, mut tf) = (0.0, 0.0);
        for a in kept(anchors, keep) {
            let dt = a.session_us as f64 - t_mean;
            tt += dt * dt;
            tf += dt * (a.frame as f64 - f_mean);
        }
        let slope = tf / tt;
        (slope.is_finite() && slope > 0.0).then_some(Self {
            slope,
            t_mean,
            f_mean,
        })
    }

    /// The anchor's distance from the line, in frames.
    fn residual(&self, a: &Anchor) -> f64 {
        a.frame as f64 - self.f_mean - self.slope * (a.session_us as f64 - self.t_mean)
    }
}

/// The anchors `keep` marks.
fn kept<'a>(anchors: &'a [Anchor], keep: &'a [bool]) -> impl Iterator<Item = &'a Anchor> {
    anchors
        .iter()
        .zip(keep)
        .filter(|(_, k)| **k)
        .map(|(a, _)| a)
}
