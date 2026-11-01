//! Timeline facts read from JSON (a planner context). `on_timeline` gives each kind sorted, sane and at most
//! `MAX_FACTS` long, but serde would take any number in any order, so each kind is held to the same when it is read: a
//! fact outside the reader's time window (never negative, at most `TIME_LIMIT_US`), a span ending before it starts, or
//! a place that isn't finite (JSON refuses one; another format might not) goes; the first `MAX_FACTS` are kept, then
//! sorted by time. The summary and the tools then never see an unbounded or unsorted list, nor times whose sums
//! overflow.

use serde::{Deserialize, Deserializer};

use super::{ClickAt, MAX_FACTS, RegionAt, SpanAt};
use crate::Us;
use crate::format::TIME_LIMIT_US;

/// One timeline fact: when it is, and whether its place is a number.
pub(super) trait Fact {
    /// [from, to]; a click's both ends are its time.
    fn span(&self) -> (Us, Us);
    fn finite(&self) -> bool {
        true
    }
}

impl Fact for ClickAt {
    fn span(&self) -> (Us, Us) {
        (self.timeline_us, self.timeline_us)
    }
    fn finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Fact for SpanAt {
    fn span(&self) -> (Us, Us) {
        (self.from_us, self.to_us)
    }
    fn finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Fact for RegionAt {
    fn span(&self) -> (Us, Us) {
        (self.from_us, self.to_us)
    }
    fn finite(&self) -> bool {
        let r = &self.rect;
        [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite())
    }
}

impl Fact for (Us, Us) {
    fn span(&self) -> (Us, Us) {
        *self
    }
}

/// One kind as read, held to what the timeline gives. A stable sort, so facts at the same time keep their order and
/// the result is the same on every run. O(n + k log k) for n read and k kept; the list as read is bounded by the
/// request's size.
pub(super) fn kept<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Fact,
{
    let mut facts = Vec::<T>::deserialize(d)?;
    facts.retain(|f| {
        let (from, to) = f.span();
        (0..=to).contains(&from) && to <= TIME_LIMIT_US && f.finite()
    });
    facts.truncate(MAX_FACTS);
    facts.sort_by_key(Fact::span);
    Ok(facts)
}
