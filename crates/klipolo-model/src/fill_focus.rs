//! Where a main clip's Fill crop keeps its focus in a format, beyond its framing's own fixed point (`Clip::fill_focus`):
//! in the centre, or on the speaker along a path made from where they are in each shot. No entry keeps the framing's
//! `focus_x` and `focus_y` (Fixed), as every project saved before this did.
//!
//! A path's keys are in the clip's source time, so a cut, a trim, a split or a speed change keeps it on the footage. The
//! crop's centre moves straight from one key to the next, except at a key that starts a new shot (`FocusKey::cut`):
//! there it holds the shot's last key up to the cut and then jumps, so it never pans across a cut.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::time::Us;

/// A Fill crop's focus in one format, beyond its framing's fixed point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum FillFocus {
    /// The crop stays in the centre of the picture.
    Center,
    /// The crop follows the speaker: its centre moves along `path`.
    Speaker { path: FocusPath },
}

/// Where a following crop's centre is over the clip's source time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FocusPath {
    /// In source time order, each later than the one before.
    pub keys: Vec<FocusKey>,
}

/// One point of a [`FocusPath`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FocusKey {
    /// Source time.
    pub at_us: Us,
    /// The crop's centre, 0 to 1 across and down the picture as the file holds it (before any mirror).
    pub x: f64,
    pub y: f64,
    /// A new shot starts here: the crop jumps to this key at `at_us` instead of moving to it from the one before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

impl FocusPath {
    /// The most keys a path holds: four a second for over an hour of footage.
    pub const MAX_KEYS: usize = 20_000;

    /// The crop's centre at source time `at_us`: the nearest key before the first and after the last, straight from
    /// one key to the next between them, and the shot's last key up to a cut. None for an empty path. O(log keys).
    pub fn centre_at(&self, at_us: Us) -> Option<(f64, f64)> {
        let first = self.keys.first()?;
        // The keys at or before `at_us`.
        let n = self.keys.partition_point(|k| k.at_us <= at_us);
        if n == 0 {
            return Some((first.x, first.y));
        }
        let a = self.keys[n - 1];
        let Some(b) = self.keys.get(n) else {
            return Some((a.x, a.y));
        };
        if b.cut || b.at_us <= a.at_us {
            return Some((a.x, a.y));
        }
        let t = (at_us - a.at_us) as f64 / (b.at_us - a.at_us) as f64;
        Some((a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t))
    }

    /// Whether it may be saved: at most [`Self::MAX_KEYS`] keys, each later than the one before, inside the picture.
    pub fn is_valid(&self) -> bool {
        let inside = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        self.keys.len() <= Self::MAX_KEYS
            && self.keys.windows(2).all(|w| w[0].at_us < w[1].at_us)
            && self
                .keys
                .iter()
                .all(|k| k.at_us >= 0 && inside(k.x) && inside(k.y))
    }
}

impl FillFocus {
    /// Whether it may be saved (a speaker's path, [`FocusPath::is_valid`]).
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Center => true,
            Self::Speaker { path } => path.is_valid(),
        }
    }
}
