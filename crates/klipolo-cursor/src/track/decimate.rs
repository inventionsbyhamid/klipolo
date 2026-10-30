//! Which positions a track keeps: those at least `gap` apart, plus where the cursor came to rest (the last position
//! before a pause of at least `gap`) and the file's last, so a resting cursor holds where it really stopped. At most
//! two kept per `gap` (a rest point and the move after it); deterministic, O(1) a position. The recorder's writer thins
//! by the same rule, so it writes what a read would keep.

use crate::Us;

/// A position: its time and place.
pub type Sample = (Us, f32, f32);

/// The thinning (see the module notes).
#[derive(Clone, Debug)]
pub struct Decimate {
    gap: Us,
    last_kept: Option<Us>,
    /// The latest position passed over since the last one kept.
    pending: Option<Sample>,
}

impl Decimate {
    pub fn new(gap: Us) -> Self {
        Self {
            gap,
            last_kept: None,
            pending: None,
        }
    }

    /// The positions to keep, in order, now that `s` (no older than the last) has arrived.
    pub fn next(&mut self, s: Sample) -> [Option<Sample>; 2] {
        if self
            .last_kept
            .is_some_and(|last| s.0 < last.saturating_add(self.gap))
        {
            self.pending = Some(s);
            return [None, None];
        }
        let rest = self
            .pending
            .take()
            .filter(|p| s.0 >= p.0.saturating_add(self.gap));
        self.last_kept = Some(s.0);
        [rest, Some(s)]
    }

    /// The file's last position, if it was passed over.
    pub fn last(&mut self) -> Option<Sample> {
        self.pending.take()
    }

    /// The position passed over last, once `now` is `gap` past it with nothing newer: where the cursor came to rest,
    /// kept now rather than when the next position comes (the writer's tick).
    pub fn rest_due(&mut self, now: Us) -> Option<Sample> {
        let due = self
            .pending
            .filter(|p| now >= p.0.saturating_add(self.gap))?;
        self.pending = None;
        self.last_kept = Some(due.0);
        Some(due)
    }
}

#[cfg(test)]
mod tests {
    use super::Decimate;

    #[test]
    fn a_rest_point_is_kept_once_its_pause_is_long_enough() {
        let mut d = Decimate::new(4_000);
        assert_eq!(d.next((0, 0.1, 0.1)), [None, Some((0, 0.1, 0.1))]);
        assert_eq!(d.next((1_000, 0.2, 0.2)), [None, None]);
        assert_eq!(d.rest_due(4_999), None, "not a pause yet");
        assert_eq!(d.rest_due(5_000), Some((1_000, 0.2, 0.2)));
        assert_eq!((d.rest_due(9_000), d.last()), (None, None), "kept once");
        assert_eq!(d.next((5_000, 0.3, 0.3)), [None, Some((5_000, 0.3, 0.3))]);
    }
}
