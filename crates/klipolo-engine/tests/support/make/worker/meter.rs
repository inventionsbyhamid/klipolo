//! What the worker's meter timed in the tests: the frames at each size and setting, and the combinations measured.

use klipolo_engine::make::measure::Metered;

use super::InProcessWorker;

impl InProcessWorker {
    /// Frames the meter timed at `size` (not at half size).
    pub fn meter_frames_at(&self, size: (u32, u32)) -> usize {
        self.metered()
            .iter()
            .filter(|m| m.size == size && !m.half)
            .count()
    }

    /// Those with the setting `key` at `value`.
    pub fn meter_frames_at_settings(&self, key: &str, value: f64, size: (u32, u32)) -> usize {
        self.metered()
            .iter()
            .filter(|m| m.size == size && !m.half && m.values.get(key) == Some(&value))
            .count()
    }

    /// Every frame the meter timed.
    pub fn frames_timed(&self) -> usize {
        self.metered().len()
    }

    /// The setting combinations measured: each is stepped from 16² first.
    pub fn measured_corners(&self) -> usize {
        self.meter_frames_at((16, 16))
    }

    /// Every value of `key` the meter saw, sorted.
    pub fn measured_values_of(&self, key: &str) -> Vec<u32> {
        let mut values: Vec<u32> = self
            .metered()
            .iter()
            .filter_map(|m| m.values.get(key))
            .map(|v| *v as u32)
            .collect();
        values.sort_unstable();
        values.dedup();
        values
    }

    pub(super) fn metered(&self) -> Vec<Metered> {
        self.log.lock().unwrap().clone()
    }
}
