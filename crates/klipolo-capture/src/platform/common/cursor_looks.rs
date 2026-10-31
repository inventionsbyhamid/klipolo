//! The macOS cursor's shape looks, their decisions (the shape
//! budget and the animated cursor). Pure, built on every OS, so every OS's tests run them.
//! - `NilRun`: no current system cursor is "shape unknown", never hidden; nil for 5 s straight falls back to
//!   the arrow look.
//! - `SeedReads`, `SeedTrust` (option 1): Core Graphics' cursor seed, read before each look and peeked at after
//!   it (a peek leaves a move during the look for the next pass). A new key while the seed stayed put
//!   both times means it can't be trusted: the take drops it. A look with no key before it (the first, or the first
//!   after no current cursor) is never a miss. It keeps counting after a drop, so the take's log says how often the
//!   question ("key changed, same seed").
//! - `ShapeSends`: each new shape is sent once with its picture; past `MAX_SHAPES` (an
//!   animated cursor's frames fill it) a new key is never sent, so the cursor keeps its last defined shape, and the
//!   writer is told once.
//! - `Animation`: seed looks that keep finding a new frame (4 within a second) mean an animated cursor (a spinning
//!   or busy one): its seed looks slow to 3 Hz until a look finds the key it had.

use klipolo_cursor::MAX_SHAPES;

/// No current system cursor for this long: the arrow look.
pub const NIL_FALLBACK_NS: u64 = 5_000_000_000;
/// Seed looks at most this often normally (30 Hz) and while the cursor animates (3 Hz).
pub const SEED_LOOK_NS: u64 = 33_333_333;
pub const ANIMATED_SEED_LOOK_NS: u64 = 333_333_333;
/// New frames, each found by a seed look, within `ANIMATION_WITHIN_NS`, that make an animation.
pub const ANIMATION_FRAMES: u32 = 4;
pub const ANIMATION_WITHIN_NS: u64 = 1_000_000_000;

/// See the module notes.
#[derive(Clone, Copy, Debug, Default)]
pub struct NilRun {
    since: Option<u64>,
    fallen_back: bool,
}

impl NilRun {
    /// A look at `now_ns` found no current system cursor: true once, when it has been so for 5 s straight.
    pub fn nil(&mut self, now_ns: u64) -> bool {
        let since = *self.since.get_or_insert(now_ns);
        let due = !self.fallen_back && now_ns.saturating_sub(since) >= NIL_FALLBACK_NS;
        self.fallen_back |= due;
        due
    }

    /// A look found one: true when the shapes were on the arrow look until now.
    pub fn found(&mut self) -> bool {
        self.since = None;
        std::mem::take(&mut self.fallen_back)
    }
}

/// The seed's last value a pass took (see the module notes).
#[derive(Clone, Copy, Debug, Default)]
pub struct SeedReads {
    last: Option<i32>,
}

impl SeedReads {
    /// A pass's read: whether the seed moved since the last one taken (the first counts as moved); it is kept.
    pub fn take(&mut self, now: i32) -> bool {
        self.last.replace(now) != Some(now)
    }

    /// A look's second read: whether the seed moved since the last one taken; nothing is kept, so the next pass sees
    /// the same move.
    pub fn peek(&self, now: i32) -> bool {
        self.last.is_some_and(|last| last != now)
    }
}

/// See the module notes.
#[derive(Clone, Copy, Debug)]
pub struct SeedTrust {
    present: bool,
    trusted: bool,
    missed: u32,
    seen: u32,
}

impl SeedTrust {
    /// `present`: the seed's symbol was found.
    pub fn new(present: bool) -> Self {
        Self {
            present,
            trusted: present,
            missed: 0,
            seen: 0,
        }
    }

    pub fn trusted(&self) -> bool {
        self.trusted
    }

    /// Key changes the seed didn't show ("key changed, same seed").
    pub fn missed(&self) -> u32 {
        self.missed
    }

    /// Key changes the seed showed.
    pub fn seen(&self) -> u32 {
        self.seen
    }

    /// A look's result: whether the seed moved before it (`before`, the look it asked for) and during it (`after`, a
    /// peek once the key was made), and the key it had (`from`: None for the first look, or the first after no current
    /// cursor) and found (`to`). True when it just dropped the seed.
    pub fn look(&mut self, before: bool, after: bool, from: Option<u64>, to: u64) -> bool {
        if !self.present || from.is_none_or(|from| from == to) {
            return false;
        }
        if before || after {
            self.seen += 1;
            return false;
        }
        self.missed += 1;
        std::mem::replace(&mut self.trusted, false)
    }
}

/// What a look does with the current shape's key (`ShapeSends::next`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    /// The shape didn't change.
    Same,
    /// A shape sent before: `Shape`.
    Known(u64),
    /// A new one: its picture goes with it (`NewShape`).
    New(u64),
    /// The first new one past the limit: the writer is told (`ShapesFull`), and the last shape stays.
    Full,
    /// Another new one past the limit: nothing, the last shape stays.
    Keep,
}

/// See the module notes.
#[derive(Clone, Debug, Default)]
pub struct ShapeSends {
    sent: Vec<u64>,
    key: Option<u64>,
    full: bool,
}

impl ShapeSends {
    /// The current shape's key is `key`.
    pub fn next(&mut self, key: u64) -> Sent {
        if self.key.replace(key) == Some(key) {
            return Sent::Same;
        }
        if self.sent.contains(&key) {
            return Sent::Known(key);
        }
        if self.sent.len() < MAX_SHAPES {
            self.sent.push(key);
            return Sent::New(key);
        }
        if std::mem::replace(&mut self.full, true) {
            Sent::Keep
        } else {
            Sent::Full
        }
    }

    /// The new shape `key`'s picture couldn't be read (it went as `Shape`, drawn as the Arrow look): not sent.
    pub fn unread(&mut self, key: u64) {
        self.sent.retain(|k| *k != key);
    }

    /// Forget the current key (no current cursor, then one again): the next look sends it whatever it is.
    pub fn forget_current(&mut self) {
        self.key = None;
    }

    /// The current shape's key (None: none yet, or forgotten).
    pub fn current(&self) -> Option<u64> {
        self.key
    }
}

/// See the module notes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Animation {
    /// The new frames found by seed looks in a row, and when the first of them was.
    frames: u32,
    first_at: u64,
    on: bool,
    started: u32,
}

impl Animation {
    /// A look at `now_ns`: whether the key `changed`, and whether the seed asked for it (`seed`). True when an animation
    /// just began (for the take's log).
    pub fn look(&mut self, now_ns: u64, changed: bool, seed: bool) -> bool {
        if !changed {
            (self.frames, self.on) = (0, false);
            return false;
        }
        if !seed {
            return false;
        }
        if self.frames == 0 || now_ns.saturating_sub(self.first_at) > ANIMATION_WITHIN_NS {
            (self.frames, self.first_at) = (0, now_ns);
        }
        self.frames += 1;
        let begins = !self.on && self.frames >= ANIMATION_FRAMES;
        if begins {
            self.on = true;
            self.started += 1;
        }
        begins
    }

    pub fn on(&self) -> bool {
        self.on
    }

    /// Animations this take (for its log).
    pub fn started(&self) -> u32 {
        self.started
    }

    /// The least time between two seed looks now.
    pub fn seed_look_ns(&self) -> u64 {
        if self.on {
            ANIMATED_SEED_LOOK_NS
        } else {
            SEED_LOOK_NS
        }
    }
}
