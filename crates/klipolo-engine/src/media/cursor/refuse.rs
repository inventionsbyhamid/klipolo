//! What the cache couldn't use: a shape image found unusable after its take loaded (its header
//! passed `shapes`' check, but it didn't decode), which export's preload and the preview's decoder say here, so the take
//! plans the Arrow look for it from then on, in preview and export alike; and a take whose file couldn't be read (export
//! words that apart from one the budget passes over).

use super::state::lock;
use super::tracks::CursorTracks;

impl CursorTracks {
    /// `take`'s shape `shape` can't be used (its image didn't decode): the take plans the Arrow look for it from now
    /// on, and the session plans again (`wake`). Nothing for a take that isn't held. O(takes), no allocation.
    pub fn refuse_shape(&self, take: &str, shape: u16) {
        let refused = lock(&self.shared.state)
            .take(take)
            .is_some_and(|t| t.refuse_shape(shape));
        if refused {
            self.shared.loaded.notify_one();
        }
    }

    /// `path`'s file was read and couldn't be used. False when it is held, or was never read (a take the takes before
    /// it fill the budget). O(takes).
    pub fn unreadable(&self, path: &str) -> bool {
        let s = lock(&self.shared.state);
        s.takes.iter().any(|(p, t)| &**p == path && t.is_none())
    }
}
