//! A take's screen activity and its follow-crop paths.
//!
//! **Activity.** The take's screen files (a resume or a heal starts a later one `SourceRun::offset_us` in) are read
//! from the activity store as it holds them now (`Screens::read`): their typing, playing media and quiet go into the
//! take's facts on the recording clock (`TakeFacts::with_screens`), so the camera's focus has them, and their tracks
//! into the follow paths. A file's ticks count from its first video frame (each stream's own clock, as its frames are
//! read; never the container's start), so its run's offset alone moves them onto the recording clock. What of each
//! file is made (how far it is whole, and any minute missing past that: one a pass couldn't read) is folded
//! into one fingerprint (`Screens::print`): the facts and the paths carry it, so the facts, the paths and the keys the
//! plan compares never disagree.
//!
//! **Follow paths.** One per crop the take's clips follow with (`Want::crops`), built from the **raw** track with the
//! follow rules' own filter, never the style's smoothing, keyed by `FollowKey`: the crop, the activity's
//! fingerprint and the rules. A new fingerprint builds every path again; the old ones go once the new are in.
//!
//! **Threads and memory.** Built by `prepare_follow` on the loader thread, or export's load before its first frame;
//! never on the actor or the render thread. A path is 4 B a step at 30 Hz (0.43 MB an hour), at most `MAX_FOLLOWS` a
//! take, all counted in the take's bytes against the cache's budget.

use std::sync::Arc;

use klipolo_cursor::Us;
use klipolo_cursor::activity::{ActivityTrack, missing};
use klipolo_cursor::follow::{Crop, FollowInput, FollowPath, FollowRules};
use klipolo_cursor::timeline::TakeFacts;

use super::take::{TakeCursor, lock};

/// Where a take's screen files' activity comes from: the session's activity store (`ActivityStore::get`), or none.
pub type ActivityOf = Arc<dyn Fn(&str) -> Option<Arc<ActivityTrack>> + Send + Sync>;

/// Quiet stretches of the screen shorter than this aren't kept in a take's facts (idle asks for more).
const QUIET_MIN_US: Us = 1_000_000;

/// What a follow path is built for: its crop (the Fill crop's size and `closer`, in thousandths), the
/// take's activity (`Screens::print`) and the follow rules (a fingerprint of `FollowRules`). Never the smoothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FollowKey {
    pub w_milli: u16,
    pub h_milli: u16,
    pub closer_milli: u16,
    pub activity: u64,
    pub rules: u64,
}

impl FollowKey {
    /// The key of `crop` (`Want::crops`) for activity `activity`, with the rules the engine follows by.
    pub fn new((w_milli, h_milli, closer_milli): (u16, u16, u16), activity: u64) -> Self {
        Self {
            w_milli,
            h_milli,
            closer_milli,
            activity,
            rules: rules_print(&FollowRules::default()),
        }
    }

    pub fn crop(&self) -> (u16, u16, u16) {
        (self.w_milli, self.h_milli, self.closer_milli)
    }
}

/// A take's screen files' activity as the store holds it now: each held track with its file's offset, in order, and
/// the fingerprint of what of each file is made.
#[derive(Default)]
pub struct Screens {
    pub print: u64,
    pub held: Vec<(Us, Arc<ActivityTrack>)>,
}

impl Screens {
    /// `files` (each file's offset and path, in order) as `of` holds them now. O(files × (the store's lookup + its
    /// chunks)); no IO.
    pub fn read(files: &[(Us, String)], of: Option<&ActivityOf>) -> Self {
        let mut held = Vec::new();
        let mut print = Fnv::default();
        for (offset, path) in files {
            let track = of.and_then(|of| of(path));
            print.add(*offset as u64);
            match &track {
                Some(t) => {
                    print.add(t.complete_through() as u64);
                    // A minute made past one missing (passed over as unreadable) changes the facts too.
                    let gaps = missing(t);
                    print.add(gaps.len() as u64);
                    gaps.into_iter().for_each(|c| print.add(u64::from(c)));
                }
                None => print.add(u64::MAX),
            }
            held.extend(track.map(|t| (*offset, t)));
        }
        Self {
            print: print.0 | 1,
            held,
        }
    }
}

/// FNV-1a over 64-bit words: deterministic everywhere, no allocation.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn add(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }
}

/// The follow rules as one number, so a key built under other rules never matches.
fn rules_print(r: &FollowRules) -> u64 {
    let mut h = Fnv::default();
    for v in [r.dead_zone, r.safe, r.omega, r.speed] {
        h.add(v.to_bits());
    }
    h.add(r.pre_roll_us as u64);
    h.0
}

impl TakeCursor {
    /// Its facts with `screens`' activity, then a follow path for each of `crops` for that activity and no other path:
    /// what the loader leaves (see the module notes). Built only when the activity or the crops changed; a path is
    /// built outside the lock, and what goes is freed after it. O(ticks + still runs) for the facts and O(steps +
    /// events) a path (≈ 20 ms for 2 h in release), O(1) when nothing changed.
    pub fn prepare_follow(&self, crops: &[(u16, u16, u16)], screens: &Screens, aspect: f64) {
        if lock(&self.known).0 != screens.print {
            let runs: Vec<(Us, &ActivityTrack)> =
                screens.held.iter().map(|(o, t)| (*o, &**t)).collect();
            // With no file held there is nothing to add: the cursor file's own facts, shared.
            let facts = match runs.is_empty() {
                true => self.base.clone(),
                false => Arc::new((*self.base).clone().with_screens(
                    &self.still,
                    &runs,
                    QUIET_MIN_US,
                )),
            };
            let gone = std::mem::replace(&mut *lock(&self.known), (screens.print, facts));
            drop(gone);
        }
        let keys: Vec<FollowKey> = crops
            .iter()
            .map(|&c| FollowKey::new(c, screens.print))
            .collect();
        let facts = self.facts();
        let runs: Vec<(Us, &ActivityTrack)> =
            screens.held.iter().map(|(o, t)| (*o, &**t)).collect();
        for &key in &keys {
            if lock(&self.follows).contains(key) {
                continue;
            }
            let closer = (f64::from(key.closer_milli) / 1000.0).max(1.0);
            let input = FollowInput {
                track: &self.track,
                dwells: &facts.dwells,
                typing: facts.typing.as_deref().unwrap_or(&[]),
                media: facts.media.as_deref().unwrap_or(&[]),
                activity: &runs,
                aspect,
                crop: Crop {
                    w: f64::from(key.w_milli) / 1000.0 / closer,
                    h: f64::from(key.h_milli) / 1000.0 / closer,
                },
                rules: FollowRules::default(),
            };
            let path = Arc::new(FollowPath::build(&input));
            let gone = lock(&self.follows).put(key, path);
            drop(gone);
        }
        let gone = lock(&self.follows).keep(&keys);
        drop(gone);
    }

    /// The follow path for `key` once built. O(1), no allocation.
    pub fn follow(&self, key: FollowKey) -> Option<Arc<FollowPath>> {
        lock(&self.follows).get(key)
    }

    /// Every follow path it holds, by crop: what a plan context takes for its frames. O(paths).
    pub fn follows(&self) -> Vec<((u16, u16, u16), Arc<FollowPath>)> {
        let held = lock(&self.follows);
        held.entries().map(|(k, p)| (k.crop(), p.clone())).collect()
    }

    /// The fingerprint of the activity its facts and paths hold (`Screens::print`; 0 before any was read).
    pub fn activity(&self) -> u64 {
        lock(&self.known).0
    }

    /// Its facts and the fingerprint of the activity they hold, read together.
    pub fn known(&self) -> (u64, Arc<TakeFacts>) {
        let known = lock(&self.known);
        (known.0, known.1.clone())
    }

    /// Holds the facts of activity `print` and a path for exactly each of `crops` with it.
    pub(super) fn follows_ready(&self, crops: &[(u16, u16, u16)], print: u64) -> bool {
        let keys: Vec<FollowKey> = crops.iter().map(|&c| FollowKey::new(c, print)).collect();
        self.activity() == print && lock(&self.follows).holds_exactly(&keys)
    }

    /// What its paths hold.
    pub(super) fn follow_bytes(&self) -> usize {
        lock(&self.follows).values().map(|p| p.bytes()).sum()
    }
}
