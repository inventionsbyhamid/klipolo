//! The camera's limits and which of them a plan can be made with.

use crate::Us;

/// The camera's limits. Speeds are peaks; durations in µs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// The most a shot zooms (the Auto zoom's strength, 1.25–3).
    pub max_scale: f64,
    /// The widest a widened shot goes.
    pub min_scale: f64,
    /// The safe zone: the view less this much of its width on each side.
    pub safe: f64,
    pub pre_roll_us: Us,
    pub merge_us: Us,
    pub hold_us: Us,
    /// How long a shot holds after its last event.
    pub tail_us: Us,
    pub out_gap_us: Us,
    /// Centres further apart than this (of the frame) zoom out and in rather than pan.
    pub out_dist: f64,
    pub move_min_us: Us,
    pub move_max_us: Us,
    /// View widths a second, at the scale of the moment.
    pub pan_speed: f64,
    /// ln(scale) a second.
    pub zoom_rate: f64,
    pub glide_omega: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            max_scale: 2.0,
            min_scale: 1.25,
            safe: 0.15,
            pre_roll_us: 300_000,
            merge_us: 1_500_000,
            hold_us: 1_200_000,
            tail_us: 1_500_000,
            out_gap_us: 2_500_000,
            out_dist: 0.5,
            move_min_us: 350_000,
            move_max_us: 1_500_000,
            pan_speed: 0.8,
            zoom_rate: 1.2,
            glide_omega: 4.0,
        }
    }
}

impl Rules {
    /// The rules `plan` uses: a strength under `min_scale` zooms at `min_scale`, the smallest zoom a shot takes, so
    /// a person's setting never silently does nothing.
    pub(crate) fn effective(self) -> Self {
        Self {
            max_scale: self.max_scale.max(self.min_scale),
            ..self
        }
    }

    /// Limits a plan can be made with: positive speeds, scales ≥ 1, a safe zone under half the view, no negative
    /// time. With others, `plan` keeps the whole frame.
    pub(crate) fn sane(&self) -> bool {
        let positive = |x: f64| x.is_finite() && x > 0.0;
        let times = [
            self.pre_roll_us,
            self.merge_us,
            self.hold_us,
            self.tail_us,
            self.out_gap_us,
        ];
        self.min_scale >= 1.0
            && self.max_scale.is_finite()
            && self.max_scale >= self.min_scale
            && (0.0..0.5).contains(&self.safe)
            && [self.pan_speed, self.zoom_rate, self.glide_omega]
                .into_iter()
                .all(positive)
            && self.out_dist.is_finite()
            && times.iter().all(|&t| t >= 0)
            && self.move_min_us >= 0
            && self.move_max_us >= self.move_min_us
    }
}
