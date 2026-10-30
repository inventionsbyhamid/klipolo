//! The activity tracks held in memory, least recently used first out, under a byte budget. A project shows a handful of
//! screen files, so a list searched in order is the right size (O(tracks) a call, no hashing). What leaves is handed
//! back so the caller frees it off the store's lock.

use std::sync::Arc;

use klipolo_cursor::activity::ActivityTrack;

#[derive(Default)]
pub(super) struct Lru {
    /// Least recently used first.
    tracks: Vec<(String, Arc<ActivityTrack>)>,
    bytes: usize,
}

impl Lru {
    /// `path`'s track, now the most recently used.
    pub(super) fn get(&mut self, path: &str) -> Option<Arc<ActivityTrack>> {
        let i = self.tracks.iter().position(|(p, _)| p == path)?;
        let entry = self.tracks.remove(i);
        let track = entry.1.clone();
        self.tracks.push(entry);
        Some(track)
    }

    /// `path`'s track, its place unchanged.
    pub(super) fn peek(&self, path: &str) -> Option<&Arc<ActivityTrack>> {
        self.tracks.iter().find(|(p, _)| p == path).map(|(_, t)| t)
    }

    /// Holds `track` as `path`'s, the most recently used; the least recently used others go while the total passes
    /// `budget` (one track larger than it stays alone). Returns what went.
    pub(super) fn put(
        &mut self,
        path: &str,
        track: Arc<ActivityTrack>,
        budget: usize,
    ) -> Vec<Arc<ActivityTrack>> {
        let mut gone = Vec::new();
        if let Some(i) = self.tracks.iter().position(|(p, _)| p == path) {
            let (_, old) = self.tracks.remove(i);
            self.bytes -= old.bytes();
            gone.push(old);
        }
        self.bytes += track.bytes();
        self.tracks.push((path.to_owned(), track));
        while self.bytes > budget && self.tracks.len() > 1 {
            let (_, old) = self.tracks.remove(0);
            self.bytes -= old.bytes();
            gone.push(old);
        }
        gone
    }

    /// `path`'s track goes, if it is held (returned, for the caller to free off the lock).
    pub(super) fn remove(&mut self, path: &str) -> Option<Arc<ActivityTrack>> {
        let i = self.tracks.iter().position(|(p, _)| p == path)?;
        let (_, track) = self.tracks.remove(i);
        self.bytes -= track.bytes();
        Some(track)
    }

    /// Every track goes.
    pub(super) fn clear(&mut self) -> Vec<Arc<ActivityTrack>> {
        self.bytes = 0;
        self.tracks.drain(..).map(|(_, t)| t).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klipolo_cursor::activity::decode;

    /// The budget holds: the least recently used track goes first, and a read makes a track recent.
    #[test]
    fn the_least_recently_used_track_goes_first() {
        let hour = || Arc::new(decode(&[], 3_600_000_000));
        let each = hour().bytes();
        let mut lru = Lru::default();
        for path in ["a", "b", "c"] {
            assert!(lru.put(path, hour(), 3 * each).is_empty());
        }
        assert!(lru.get("a").is_some());
        let gone = lru.put("d", hour(), 3 * each);
        assert_eq!(gone.len(), 1);
        assert!(lru.peek("b").is_none() && lru.peek("a").is_some());
        assert_eq!(lru.clear().len(), 3);
    }
}
