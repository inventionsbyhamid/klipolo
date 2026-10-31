//! The cursor's shapes and visibility, on the sampler thread (the
//! shape check's budget and the animated cursor). The decisions are `platform::common::cursor_looks`'.
//! - **When to look.** `currentSystem` costs about 0.3 ms a call, so Core Graphics' cursor seed (a private counter,
//!   16 ns) says when the cursor changed, and a look runs then; a safety look (1 Hz still, 3 Hz moving) also runs. The
//!   seed is read again after each look: a new shape it didn't show either time drops it for the take (`SeedTrust`).
//!   Without the seed's symbol, the public looks run at 3 Hz still and 10 Hz moving, the key hashed only when the
//!   identity changed or once a second. An animated cursor (`Animation`) slows the seed looks to 3 Hz.
//! - **A look.** Visibility from `CGCursorIsVisible` (the only "hidden"); then the current cursor's identity
//!   and key: a `Shape` for a key sent before, a `NewShape` with its picture for a new one (`image`). Past `MAX_SHAPES`
//!   nothing new is sent, so the last shape stays; one `ShapesFull` tells the take (`ShapeSends`). A system shape is
//!   named when its key is one of the class cursors' (`table`).
//! - **No current cursor** is "shape unknown": nothing is sent. Nil for 5 s straight sends the arrow class cursor
//!   (`sys: arrow`), logged as the take's shapes falling back (`NilRun`).

use klipolo_cursor::SystemShape;
use objc2::rc::{AutoreleasePool, autoreleasepool};
use objc2::runtime::AnyObject;

use super::image::{self, Identity};
use super::table::{class_cursor, system_table};
use crate::cursor::CursorEvent;
use crate::platform::common::cursor_looks::{
    Animation, NilRun, SeedReads, SeedTrust, Sent, ShapeSends,
};

/// Without the seed, a look whose identity is unchanged hashes the key again this often.
const KEY_AGAIN_NS: u64 = 1_000_000_000;

/// See the module notes.
pub(super) struct Shapes {
    table: Vec<(u64, SystemShape)>,
    seed: Option<extern "C" fn() -> i32>,
    reads: SeedReads,
    trust: SeedTrust,
    identity: Option<Identity>,
    key_at: u64,
    sends: ShapeSends,
    pub(super) animation: Animation,
    nil: NilRun,
    visible: Option<bool>,
    pub(super) looks: u64,
}

impl Shapes {
    pub(super) fn new() -> Self {
        let seed = image::seed();
        Self {
            table: system_table(),
            seed,
            reads: SeedReads::default(),
            trust: SeedTrust::new(seed.is_some()),
            identity: None,
            key_at: 0,
            sends: ShapeSends::default(),
            animation: Animation::default(),
            nil: NilRun::default(),
            visible: None,
            looks: 0,
        }
    }

    /// Whether looks follow the seed (it exists and wasn't dropped).
    pub(super) fn seeded(&self) -> bool {
        self.trust.trusted()
    }

    /// The system table's size, and the seed's misses and hits (for the take's log).
    pub(super) fn counts(&self) -> (usize, u32, u32) {
        (self.table.len(), self.trust.missed(), self.trust.seen())
    }

    /// Whether the seed moved since a pass last took it (the first counts as moved); false without its symbol. Read
    /// even once the seed is dropped, to count.
    pub(super) fn seed_moved(&mut self) -> bool {
        self.seed.is_some_and(|seed| self.reads.take(seed()))
    }

    /// The same, as a peek: the move stays for the next pass.
    fn seed_peek(&self) -> bool {
        self.seed.is_some_and(|seed| self.reads.peek(seed()))
    }

    /// One look at `now`, after the seed moved (`seed_moved`) or as a scheduled look; what changed goes to `send`.
    pub(super) fn look(&mut self, now: u64, seed_moved: bool, send: &mut impl FnMut(CursorEvent)) {
        self.looks += 1;
        autoreleasepool(|pool| self.look_in(pool, now, seed_moved, send));
    }

    fn look_in(
        &mut self,
        pool: AutoreleasePool<'_>,
        now: u64,
        seed_moved: bool,
        send: &mut impl FnMut(CursorEvent),
    ) {
        let on = image::visible();
        if self.visible.replace(on) != Some(on) {
            send(CursorEvent::Visible { host_ns: now, on });
        }
        let Some(cursor) = image::current_system(pool) else {
            if self.nil.nil(now) {
                tracing::warn!(
                    "cursor: no current system cursor for 5 s: the shapes are the arrow look from here (fallback)"
                );
                if let Some(arrow) = class_cursor(pool, c"arrowCursor") {
                    let id = image::identity(arrow);
                    self.show(arrow, id, now, Some(SystemShape::Arrow), send);
                }
            }
            return;
        };
        if self.nil.found() {
            tracing::info!("cursor: the current system cursor is back: real shapes again");
            self.identity = None;
            self.sends.forget_current();
        }
        let id = image::identity(cursor);
        let again = now.saturating_sub(self.key_at) >= KEY_AGAIN_NS;
        if !self.seeded() && self.identity == Some(id) && !again {
            return;
        }
        self.identity = Some(id);
        // The key before this look: None for the first, or the first after no current cursor (never a miss).
        let from = self.sends.current();
        let Some(key) = self.show(cursor, id, now, None, send) else {
            return;
        };
        let changed = from != Some(key);
        // The seed again, as a peek: a change racing the look is no miss, and the next pass still sees it.
        let after = self.seed_peek();
        if self.trust.look(seed_moved, after, from, key) {
            tracing::warn!(
                "cursor: a shape changed with the cursor seed unchanged: this take looks with the public API"
            );
        }
        if self.animation.look(now, changed, seed_moved) && self.animation.started() == 1 {
            tracing::warn!(
                "cursor: the cursor is animating: its shape looks slow to 3 Hz until it rests"
            );
        }
    }

    /// `cursor` (of identity `id`) as the current shape at `now`: its key (None without one), and a `Shape`, `NewShape`
    /// or `ShapesFull` as `ShapeSends` decides. `sys`: what it is (else looked up in the table).
    fn show(
        &mut self,
        cursor: &AnyObject,
        id: Identity,
        now: u64,
        sys: Option<SystemShape>,
        send: &mut impl FnMut(CursorEvent),
    ) -> Option<u64> {
        let key = image::key(cursor, id)?;
        self.key_at = now;
        let host_ns = now;
        match self.sends.next(key) {
            Sent::Same => {}
            Sent::Known(key) => send(CursorEvent::Shape { host_ns, key }),
            Sent::New(key) => match image::picture(cursor, id) {
                Some(mut image) => {
                    let named = || self.table.iter().find(|(k, _)| *k == key).map(|(_, s)| *s);
                    image.sys = sys.or_else(named);
                    send(CursorEvent::NewShape {
                        host_ns,
                        key,
                        image,
                    });
                }
                None => {
                    self.sends.unread(key);
                    send(CursorEvent::Shape { host_ns, key });
                }
            },
            Sent::Full => {
                tracing::warn!(
                    animated = self.animation.on(),
                    "cursor: the take has all the shapes it keeps (animated?): new ones aren't recorded, the last one stays"
                );
                send(CursorEvent::ShapesFull { host_ns });
            }
            Sent::Keep => {}
        }
        Some(key)
    }
}
