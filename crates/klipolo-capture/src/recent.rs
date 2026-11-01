//! The last `N` measurements of something, and their quantiles: the writer's flush ticks (`tick_p99_ms`) and an
//! encoder's frame latencies (`VideoStats::encode_p99_ms`, the probe's p95). A fixed array, so recording one allocates
//! nothing and costs O(1); a quantile copies the array on the stack and selects in O(N), so it is for status lines and
//! the probe, never per frame.

use std::cmp::Ordering;

pub(crate) struct Recent<T, const N: usize> {
    values: [T; N],
    len: usize,
    next: usize,
}

impl<T: Copy + Default, const N: usize> Default for Recent<T, N> {
    fn default() -> Self {
        Self {
            values: [T::default(); N],
            len: 0,
            next: 0,
        }
    }
}

impl<T: Copy + Default + PartialOrd, const N: usize> Recent<T, N> {
    /// Keeps `value`, forgetting the oldest once `N` are kept.
    pub(crate) fn push(&mut self, value: T) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % N;
        self.len = (self.len + 1).min(N);
    }

    /// The `q` quantile (0-1) of what is kept, by nearest rank (the smallest value with at least `q` of the values at or
    /// below it); `T::default()` with nothing kept.
    pub(crate) fn quantile(&self, q: f32) -> T {
        if self.len == 0 {
            return T::default();
        }
        let mut copy = self.values;
        let kept = &mut copy[..self.len];
        // In f64 to six places: 0.99_f32 is a hair over 0.99, which would round 100 values' 99th up to the 100th.
        let q = (f64::from(q.clamp(0.0, 1.0)) * 1e6).round() / 1e6;
        let rank = (self.len as f64 * q).ceil() as usize;
        let at = rank.clamp(1, self.len) - 1;
        let by = |a: &T, b: &T| a.partial_cmp(b).unwrap_or(Ordering::Equal);
        *kept.select_nth_unstable_by(at, by).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_by_nearest_rank_over_the_last_n() {
        let mut r: Recent<u32, 100> = Recent::default();
        assert_eq!(r.quantile(0.99), 0, "nothing yet");
        for ms in 1..=100 {
            r.push(ms);
        }
        assert_eq!(
            (r.quantile(0.5), r.quantile(0.99), r.quantile(1.0)),
            (50, 99, 100)
        );
        for _ in 0..100 {
            r.push(7);
        }
        assert_eq!(r.quantile(0.99), 7, "the oldest are forgotten");
        let mut f: Recent<f32, 4> = Recent::default();
        for ms in [3.0, 1.0, 2.0] {
            f.push(ms);
        }
        assert_eq!((f.quantile(0.0), f.quantile(0.95)), (1.0, 3.0));
    }
}
