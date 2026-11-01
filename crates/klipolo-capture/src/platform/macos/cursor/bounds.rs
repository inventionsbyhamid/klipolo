//! Where the captured picture is: its global rect in points, Core Graphics' space (the
//! main display's top left at 0, 0, y down), kept in an `AreaCell` the monitor's handler, the poll and the sampler read
//! without a lock. A display's bounds, or a window's from `CGWindowListCopyWindowInfo` (4 Hz on the sampler thread,
//! so a window moved or resized mid-take keeps its cursor: Beats Cap 6).
//!
//! Window info is read for its bounds, its number and (only to tell Klipolo's own windows, `own_window_at`) its owner's
//! process id: never a title key (the consent scan bans them in the cursor files).
//!
//! While the recorded window is dragged, positions map against bounds up to 250 ms old: a sawtooth as large as the
//! drag's move in that time (the cursor's benches measure the cost of refreshing faster, the screen smoke the error).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::backend::Target;

/// Core Graphics' rect: origin and size, f64 each (`CGRect`'s layout).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// `kCGWindowListOptionOnScreenOnly`, `kCGWindowListOptionIncludingWindow`, `kCGWindowListExcludeDesktopElements`.
const ON_SCREEN: u32 = 1 << 0;
const INCLUDING_WINDOW: u32 = 1 << 3;
const NO_DESKTOP: u32 = 1 << 4;
/// `kCFNumberSInt64Type`.
const SINT64: isize = 4;
/// A reader of the area that overlapped a write this many times yields its core.
const SPINS_BEFORE_YIELD: u32 = 100;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGWindowBounds: *const c_void;
    static kCGWindowNumber: *const c_void;
    static kCGWindowOwnerPID: *const c_void;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
    fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut Rect) -> bool;
    fn CGDisplayBounds(display: u32) -> Rect;
    fn CGMainDisplayID() -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, at: isize) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: *const c_void, kind: isize, out: *mut c_void) -> bool;
    fn CFRelease(object: *const c_void);
}

/// The captured area's global rect in points, swapped atomically: a seqlock over four f64s (one writer, the sampler;
/// readers retry the rare read that overlapped a write). Zero size until known.
#[derive(Debug, Default)]
pub(super) struct AreaCell {
    seq: AtomicU64,
    v: [AtomicU64; 4],
}

impl AreaCell {
    /// The rect, retried while a write is under way; after `SPINS_BEFORE_YIELD` tries a reader yields its core, so a
    /// preempted writer gets to finish.
    pub(super) fn get(&self) -> (f64, f64, f64, f64) {
        let mut tries = 0u32;
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 0 {
                let v = self
                    .v
                    .each_ref()
                    .map(|a| f64::from_bits(a.load(Ordering::Relaxed)));
                fence(Ordering::Acquire);
                if self.seq.load(Ordering::Relaxed) == s1 {
                    return (v[0], v[1], v[2], v[3]);
                }
            }
            tries = tries.wrapping_add(1);
            if tries.is_multiple_of(SPINS_BEFORE_YIELD) {
                std::thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        }
    }

    /// The one writer's (the sampler's, and the source's before it starts).
    pub(super) fn set(&self, rect: (f64, f64, f64, f64)) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (a, v) in self.v.iter().zip([rect.0, rect.1, rect.2, rect.3]) {
            a.store(v.to_bits(), Ordering::Relaxed);
        }
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }
}

/// A display's bounds (a display, an app's windows on it), or a window's; None for a window that is gone, a display
/// id that isn't one, or a camera.
pub(super) fn area_of(target: &Target) -> Option<(f64, f64, f64, f64)> {
    let display = |id: &str| -> Option<(f64, f64, f64, f64)> {
        let id: u32 = id.parse().ok()?;
        // SAFETY: a plain query by id; an id that isn't a display gives an empty rect.
        let r = unsafe { CGDisplayBounds(id) };
        (r.w > 0.0 && r.h > 0.0).then_some((r.x, r.y, r.w, r.h))
    };
    match target {
        Target::Display { id } | Target::DisplayWithoutKlipolo { id } => display(id),
        Target::App { display: id, .. } => display(id),
        Target::Window { id } => window(id.parse().ok()?),
        Target::Camera { .. } | Target::Portal { .. } => None,
    }
}

/// The main display's height in points (AppKit's points flip against it).
pub(super) fn main_height() -> f64 {
    // SAFETY: plain queries.
    unsafe { CGDisplayBounds(CGMainDisplayID()).h }
}

/// Window `id`'s bounds, by its number.
fn window(id: u32) -> Option<(f64, f64, f64, f64)> {
    let mut found = None;
    windows(INCLUDING_WINDOW, id, |info| {
        if number(info, unsafe { kCGWindowNumber }) == Some(i64::from(id)) {
            found = bounds(info);
        }
    });
    found.filter(|r| r.2 > 0.0 && r.3 > 0.0)
}

/// Whether one of this process's on-screen windows (the HUD, the menu-bar item, an open menu) is at `point` (global
/// points): a global monitor gets no event there. Read only to judge a self-check's miss.
pub(super) fn own_window_at(point: (f64, f64)) -> bool {
    let pid = i64::from(std::process::id());
    let mut ours = false;
    windows(ON_SCREEN | NO_DESKTOP, 0, |info| {
        if !ours && number(info, unsafe { kCGWindowOwnerPID }) == Some(pid) {
            ours = bounds(info).is_some_and(|(x, y, w, h)| {
                (x..=x + w).contains(&point.0) && (y..=y + h).contains(&point.1)
            });
        }
    });
    ours
}

/// Each window's info dictionary `CGWindowListCopyWindowInfo(option, relative_to)` lists, to `each`.
fn windows(option: u32, relative_to: u32, mut each: impl FnMut(*const c_void)) {
    // SAFETY: the copy is +1 (released below) or null; its items are dictionaries it owns while it lives.
    unsafe {
        let list = CGWindowListCopyWindowInfo(option, relative_to);
        if list.is_null() {
            return;
        }
        for i in 0..CFArrayGetCount(list) {
            let info = CFArrayGetValueAtIndex(list, i);
            if !info.is_null() {
                each(info);
            }
        }
        CFRelease(list);
    }
}

/// A window info's `kCGWindowBounds`.
fn bounds(info: *const c_void) -> Option<(f64, f64, f64, f64)> {
    let mut r = Rect::default();
    // SAFETY: `info` is a live window info dictionary; the bounds value is a dictionary (or null, refused below).
    let ok = unsafe {
        let value = CFDictionaryGetValue(info, kCGWindowBounds);
        !value.is_null() && CGRectMakeWithDictionaryRepresentation(value, &mut r)
    };
    ok.then_some((r.x, r.y, r.w, r.h))
}

/// A window info's number under `key` (`kCGWindowNumber`, `kCGWindowOwnerPID`).
fn number(info: *const c_void, key: *const c_void) -> Option<i64> {
    let mut n = 0i64;
    // SAFETY: `info` is a live window info dictionary; both keys hold CFNumbers.
    let ok = unsafe {
        let value = CFDictionaryGetValue(info, key);
        !value.is_null() && CFNumberGetValue(value, SINT64, (&raw mut n).cast())
    };
    ok.then_some(n)
}
