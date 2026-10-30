//! A take's variants: what it builds from its track for the ways its clips draw it, by key (a smoothing,
//! or a follow-crop's `FollowKey`), each kind in its own set. At most `MAX` a set: past it the least recently used goes
//! first. A take never wants more of a kind than that (three smoothings build a path; `want::MAX_FOLLOWS` crops), so
//! it is never built again on every publish. A variant no clip uses goes
//! as soon as the loader prepares the take again (`keep`), so preview holds what export builds and both count the same
//! bytes against the budget.
//!
//! Small (≤ 4 entries), so a linear search: O(1) in practice, no allocation after the first insert.

/// The most variants a take holds.
pub(super) const MAX: usize = 4;

/// Variants by key, least recently used first.
#[derive(Debug)]
pub(super) struct Variants<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for Variants<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: Copy + PartialEq, V: Clone> Variants<K, V> {
    /// `key`'s variant, now the most recently used.
    pub(super) fn get(&mut self, key: K) -> Option<V> {
        self.get_ref(key).cloned()
    }

    /// `key`'s variant by reference (a frame reads it under the lock), now the most recently used.
    pub(super) fn get_ref(&mut self, key: K) -> Option<&V> {
        let i = self.entries.iter().position(|(k, _)| *k == key)?;
        let entry = self.entries.remove(i);
        self.entries.push(entry);
        self.entries.last().map(|(_, v)| v)
    }

    /// The variant whose key `rank` puts first (the lowest; the earliest held on a tie), keys it gives None skipped.
    pub(super) fn best_by(&self, rank: impl Fn(K) -> Option<u32>) -> Option<&V> {
        self.entries
            .iter()
            .filter_map(|(k, v)| Some((rank(*k)?, v)))
            .min_by_key(|(r, _)| *r)
            .map(|(_, v)| v)
    }

    pub(super) fn contains(&self, key: K) -> bool {
        self.entries.iter().any(|(k, _)| *k == key)
    }

    /// Holds `value` for `key` as the most recently used; past `MAX`, the least recently used goes. What it lets go
    /// comes back, for the caller to drop after its lock (a path is megabytes).
    #[must_use = "drop what goes after the lock"]
    pub(super) fn put(&mut self, key: K, value: V) -> Vec<V> {
        let mut gone = self.remove_where(|k| k == key);
        self.entries.push((key, value));
        if self.entries.len() > MAX {
            gone.push(self.entries.remove(0).1);
        }
        gone
    }

    /// Only the variants of `keys` stay; the others come back, as `put`'s.
    #[must_use = "drop what goes after the lock"]
    pub(super) fn keep(&mut self, keys: &[K]) -> Vec<V> {
        self.remove_where(|k| !keys.contains(&k))
    }

    fn remove_where(&mut self, gone: impl Fn(K) -> bool) -> Vec<V> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            if gone(self.entries[i].0) {
                out.push(self.entries.remove(i).1);
            } else {
                i += 1;
            }
        }
        out
    }

    /// Holds a variant for each of `keys` and no other.
    pub(super) fn holds_exactly(&self, keys: &[K]) -> bool {
        keys.iter().all(|k| self.contains(*k)) && self.entries.iter().all(|(k, _)| keys.contains(k))
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    /// Each key and its variant, least recently used first.
    pub(super) fn entries(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_four_and_the_least_recently_used_goes_first() {
        let mut v: Variants<u8, u8> = Variants::default();
        for k in 1..=4 {
            assert!(v.put(k, k * 10).is_empty());
        }
        // 1 is read, so 2 is now the least recently used.
        assert_eq!(v.get(1), Some(10));
        assert_eq!(v.put(5, 50), [20], "what goes comes back");
        assert_eq!(v.len(), MAX);
        assert!(!v.contains(2) && v.contains(1) && v.contains(5));
        assert_eq!(v.put(6, 60), [30]);
        assert_eq!(v.put(6, 61), [60], "replaced");
        assert_eq!(v.keep(&[1, 6]), [40, 50]);
        assert!(v.holds_exactly(&[6, 1]) && !v.holds_exactly(&[6]));
        assert_eq!(v.values().copied().collect::<Vec<_>>(), [10, 61]);
        assert_eq!(v.best_by(|k| (k != 1).then_some(0)), Some(&61));
    }
}
