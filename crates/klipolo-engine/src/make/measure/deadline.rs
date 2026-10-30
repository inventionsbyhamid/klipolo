//! When a measurement must stop: a budget below the request's
//! deadline, counted from when the request arrived, so the device's creation, the hardening and whatever ran before
//! the measurement are inside it.

use std::time::{Duration, Instant};

/// A compile's measuring budget: 60 % of the worker's 10 s deadline.
pub const COMPILE_BUDGET: Duration = Duration::from_secs(6);
/// A `Measure`'s: 60 % of its 15 s deadline.
pub const MEASURE_BUDGET: Duration = Duration::from_secs(9);

/// When the measurement stops, and the budget that set it (the agent's note names it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    pub at: Instant,
    pub budget: Duration,
}

impl Deadline {
    /// `cap` (the request kind's budget) after the request `arrived`, or the `asked_ms` it asked for when that is less.
    pub fn new(arrived: Instant, asked_ms: u32, cap: Duration) -> Self {
        let budget = match asked_ms {
            0 => cap,
            ms => Duration::from_millis(u64::from(ms)).min(cap),
        };
        Self {
            at: arrived + budget,
            budget,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The budget runs from the request's arrival, and a client never asks for more than the server allows.
    #[test]
    fn the_deadline_runs_from_arrival_and_is_clamped() {
        let arrived = Instant::now();
        let s = Duration::from_secs;
        assert_eq!(Deadline::new(arrived, 0, COMPILE_BUDGET).at, arrived + s(6));
        assert_eq!(Deadline::new(arrived, 0, MEASURE_BUDGET).at, arrived + s(9));
        assert_eq!(
            Deadline::new(arrived, 2_000, MEASURE_BUDGET).at,
            arrived + s(2)
        );
        let asked_too_much = Deadline::new(arrived, 60_000, COMPILE_BUDGET);
        assert_eq!(
            (asked_too_much.at, asked_too_much.budget),
            (arrived + s(6), s(6))
        );
    }
}
