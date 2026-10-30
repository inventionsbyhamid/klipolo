//! Versions of a person's library effects: `lib.<id>/<kind>.<slug>.v<N>`, its stem and number, and
//! whether two ids are versions of the same effect (`switchEffectVersion`; another version is a modification, not a
//! removal, in `risk`). Kept apart from `meta.rs`, whose text the measurement's version is pinned to.

use super::ContribId;

impl ContribId {
    /// A library version's stem and number: `lib.k3f9q2ma/effect.halo.v2` → (`effect.halo`, 2). None for an id that
    /// isn't a library version (built-ins, packs).
    pub fn version(&self) -> Option<(&str, u32)> {
        if !self.is_library() {
            return None;
        }
        let (_, local) = self.0.split_once('/')?;
        let (stem, n) = local.rsplit_once(".v")?;
        let digits = !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit());
        if stem.is_empty() || !digits || n.starts_with('0') {
            return None;
        }
        Some((stem, n.parse().ok()?))
    }

    /// Whether `other` is a version of the same library effect as this one (same plugin and stem), this one included.
    pub fn same_line(&self, other: &ContribId) -> bool {
        self.plugin() == other.plugin()
            && matches!((self.version(), other.version()), (Some((a, _)), Some((b, _))) if a == b)
    }
}
