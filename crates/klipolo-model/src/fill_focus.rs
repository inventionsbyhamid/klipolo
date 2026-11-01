//! Where a main clip's Fill crop keeps its focus in a format, beyond its framing's own fixed point (`Clip::fill_focus`):
//! in the centre, or on the speaker along a path made from where they are in each shot. No entry keeps the framing's
//! `focus_x` and `focus_y` (Fixed), as every project saved before this did.
//!
//! A path's keys are in the clip's source time, so a cut, a trim, a split or a speed change keeps it on the footage. The
//! crop's centre moves straight from one key to the next, except at a key that starts a new shot (`FocusKey::cut`):
//! there it holds the shot's last key up to the cut and then jumps, so it never pans across a cut.
//!
//! Where two or more people are in a shot, the speaker's focus also keeps who its path frames (`Keep`: whoever is
//! speaking, one person, everyone, or who when as chat set it) and the people it chose between (`Person`, left to
//! right, with the transcript's voices that are theirs), so the clip card and chat can name them.

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
    /// The crop follows the speaker: its centre moves along `path`, made for `keep` among `people`.
    Speaker {
        path: FocusPath,
        /// Who the path keeps in frame where two or more people are in a shot.
        #[serde(default, skip_serializing_if = "Keep::is_speaker")]
        keep: Keep,
        /// The people it chooses between, left to right; none where one person or nobody is there.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        people: Vec<Person>,
    },
}

/// The most people a following crop chooses between: P1 to P4, from the left.
pub const MAX_PEOPLE: u8 = 4;
/// The most ranges a [`Keep::Ranges`] holds.
pub const MAX_KEEP_RANGES: usize = 200;
/// A voice label's most characters.
const MAX_LABEL_CHARS: usize = 16;

/// Who a following crop keeps in frame where two or more people are in a shot. People count from 1 at the left.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Keep {
    /// Whoever is speaking.
    #[default]
    Speaker,
    /// One person all along.
    Person { person: u8 },
    /// Everyone, where they fit together; elsewhere whoever speaks more.
    Both,
    /// Who is kept when, in source time, in order and apart; whoever is speaking outside them.
    Ranges { ranges: Vec<KeepRange> },
}

/// Who is kept in frame from `start_us` to `end_us` (source time).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeepRange {
    pub start_us: Us,
    pub end_us: Us,
    pub who: Who,
}

/// Who is kept in frame at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Who {
    Speaker,
    Person { person: u8 },
    Both,
}

/// One of the people a following crop chooses between.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Person {
    /// The transcript's voices that are theirs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voices: Vec<Voice>,
    /// Their share of the clip's speech, 0 to 100.
    pub share: u8,
}

/// A voice as a file's transcript labels it: `label` ("A") means one voice only within one transcription `part`.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Voice {
    pub part: u32,
    pub label: String,
}

impl Keep {
    pub fn is_speaker(&self) -> bool {
        *self == Self::Speaker
    }

    /// Who is kept at source time `at_us`.
    pub fn who_at(&self, at_us: Us) -> Who {
        match self {
            Self::Speaker => Who::Speaker,
            Self::Person { person } => Who::Person { person: *person },
            Self::Both => Who::Both,
            Self::Ranges { ranges } => (ranges.iter())
                .find(|r| r.start_us <= at_us && at_us < r.end_us)
                .map_or(Who::Speaker, |r| r.who),
        }
    }

    /// Whether it may be saved: people from 1 to [`MAX_PEOPLE`], and ranges that aren't empty, in order and apart.
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Speaker | Self::Both => true,
            Self::Person { person } => Who::Person { person: *person }.is_valid(),
            Self::Ranges { ranges } => {
                ranges.len() <= MAX_KEEP_RANGES
                    && (ranges.iter())
                        .all(|r| r.start_us >= 0 && r.start_us < r.end_us && r.who.is_valid())
                    && ranges.windows(2).all(|w| w[0].end_us <= w[1].start_us)
            }
        }
    }
}

impl Who {
    fn is_valid(&self) -> bool {
        match self {
            Self::Person { person } => (1..=MAX_PEOPLE).contains(person),
            Self::Speaker | Self::Both => true,
        }
    }
}

impl Person {
    fn is_valid(&self) -> bool {
        self.share <= 100
            && self.voices.len() <= 64
            && (self.voices.iter())
                .all(|v| (1..=MAX_LABEL_CHARS).contains(&v.label.chars().count()))
    }
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

    /// Just the keys source time `from` to `to` needs: from the last key at or before `from` to the first at or after
    /// `to`, so it draws the same crop there. Its first key is never a cut (nothing comes before it).
    pub fn trimmed(&self, from: Us, to: Us) -> FocusPath {
        let first = self
            .keys
            .partition_point(|k| k.at_us <= from)
            .saturating_sub(1);
        let last = self
            .keys
            .partition_point(|k| k.at_us < to)
            .min(self.keys.len().saturating_sub(1));
        let mut keys = self
            .keys
            .get(first..=last)
            .map_or_else(Vec::new, <[FocusKey]>::to_vec);
        if let Some(k) = keys.first_mut() {
            k.cut = false;
        }
        FocusPath { keys }
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

/// `clip`'s speaker paths kept to the footage it plays (`FocusPath::trimmed`): a piece of a split or a cut carries
/// its own part, not the whole talk's.
pub fn trim_to_clip(clip: &mut crate::timeline::Clip) {
    let (from, to) = (clip.source_in_us, clip.source_out_us);
    for focus in clip.fill_focus.values_mut() {
        if let FillFocus::Speaker { path, .. } = focus {
            *path = path.trimmed(from, to);
        }
    }
}

impl FillFocus {
    /// Following the speaker along `path`, with nobody to choose between.
    pub fn speaker(path: FocusPath) -> Self {
        Self::Speaker {
            path,
            keep: Keep::Speaker,
            people: Vec::new(),
        }
    }

    /// Whether it may be saved (a speaker's path, [`FocusPath::is_valid`], its keep and at most [`MAX_PEOPLE`]
    /// people).
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Center => true,
            Self::Speaker { path, keep, people } => {
                path.is_valid()
                    && keep.is_valid()
                    && people.len() <= usize::from(MAX_PEOPLE)
                    && people.iter().all(Person::is_valid)
            }
        }
    }
}
