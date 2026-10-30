//! The cursor takes one plan context reads: the takes the plan follows (`CursorTracks::fitting_paths`: those
//! of `media::cursor::wanted` that fit the byte budget, read the same way by preview and export), by cursor file, and
//! their follow-crop paths. Both are taken once per context, on the actor, so a frame never waits on the loader, and a
//! path the loader rebuilds while the video plays never swaps in mid-play: the session takes it at the next edit, seek
//! or pause. O(takes × log takes) to take, O(log) a lookup with no allocation.

use std::sync::Arc;

use klipolo_cursor::follow::FollowPath;
use klipolo_cursor::timeline::TakeFacts;
use klipolo_model::project::{Project, Recording};

use crate::media::cursor::{CursorTracks, TakeCursor, take_paths};

/// The takes a context plans with, by cursor file (sorted), each with its facts and their activity's fingerprint as
/// they were when it was taken, so its camera keys and plans read one state (a context made while playing reuses it:
/// The last one).
#[derive(Default)]
pub struct Takes(Vec<Entry>);

struct Entry {
    path: Arc<str>,
    take: Arc<TakeCursor>,
    known: (u64, Arc<TakeFacts>),
}

impl Takes {
    /// `p`'s takes the plan follows now (none without a recording that kept a cursor). They become the cache's most
    /// recently used.
    pub fn of(p: &Project, cursors: &CursorTracks) -> Self {
        if !p.recordings.iter().any(|r| r.cursor.is_some()) {
            return Self::default();
        }
        let mut takes: Vec<Entry> = (cursors.fitting_paths(take_paths(p)))
            .into_iter()
            .map(|(path, take)| Entry {
                path: path.into(),
                known: take.known(),
                take,
            })
            .collect();
        takes.sort_by(|a, b| a.path.cmp(&b.path));
        Self(takes)
    }

    fn entry(&self, path: &str) -> Option<&Entry> {
        let i = self.0.binary_search_by(|e| (*e.path).cmp(path)).ok()?;
        Some(&self.0[i])
    }

    /// The take of cursor file `path`, if the plan follows it.
    pub fn get(&self, path: &str) -> Option<&Arc<TakeCursor>> {
        Some(&self.entry(path)?.take)
    }

    /// `r`'s take's activity fingerprint and facts, as they were when taken.
    pub fn known_of(&self, r: &Recording) -> Option<(u64, &Arc<TakeFacts>)> {
        let known = &self.entry(r.cursor_path()?)?.known;
        Some((known.0, &known.1))
    }

    /// `r`'s take, if it kept a cursor the plan follows.
    pub fn of_recording(&self, r: &Recording) -> Option<&Arc<TakeCursor>> {
        self.get(r.cursor_path()?)
    }

    /// The cursor file the plan knows `r`'s take by (its own shared string).
    pub fn key_of(&self, r: &Recording) -> Option<&Arc<str>> {
        Some(&self.entry(r.cursor_path()?)?.path)
    }

    /// Every take's follow paths as they are now.
    pub fn follows(&self) -> Follows {
        let mut out = Vec::new();
        for e in &self.0 {
            out.extend((e.take.follows().into_iter()).map(|(crop, f)| (e.path.clone(), crop, f)));
        }
        out.sort_by(|a, b| (&*a.0, a.1).cmp(&(&*b.0, b.1)));
        Follows(out)
    }
}

/// A follow-crop's crop key (`plan::framing::follow_crop_key`).
pub type CropKey = (u16, u16, u16);

/// The follow paths a context draws with, by cursor file and crop, sorted.
#[derive(Default)]
pub struct Follows(Vec<(Arc<str>, CropKey, Arc<FollowPath>)>);

impl Follows {
    /// The path of take `path` for `crop`, once built. O(log paths), no allocation.
    pub fn get(&self, path: &str, crop: CropKey) -> Option<&FollowPath> {
        let i = (self.0)
            .binary_search_by(|(p, c, _)| (&**p, *c).cmp(&(path, crop)))
            .ok()?;
        Some(&self.0[i].2)
    }

    /// The same paths as `other`, each the very same one: a still drawn with `other` needs no redraw for them.
    pub fn same(&self, other: &Follows) -> bool {
        let same = |a: &(Arc<str>, CropKey, Arc<FollowPath>),
                    b: &(Arc<str>, CropKey, Arc<FollowPath>)| {
            a.0 == b.0 && a.1 == b.1 && Arc::ptr_eq(&a.2, &b.2)
        };
        self.0.len() == other.0.len() && self.0.iter().zip(&other.0).all(|(a, b)| same(a, b))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
