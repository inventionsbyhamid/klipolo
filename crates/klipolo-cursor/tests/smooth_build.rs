//! How a smoothed path is built: its memory while building, the cap's hand-back, and times
//! at the edge of what the reader keeps.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::{file, head, read};
use klipolo_cursor::format::{Line, TIME_LIMIT_US};
use klipolo_cursor::smooth::{Rests, STEP_US, SmoothedPath};
use klipolo_model::cursor_style::Smoothing;

const PX: f32 = 1.0 / 1080.0;

/// Counts this thread's live heap bytes and their peak. A growing realloc counts as a copy (both blocks live), a
/// shrinking one as in place.
struct Counting;

thread_local! {
    static NOW: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

fn add(n: usize) {
    let _ = NOW.try_with(|now| {
        now.set(now.get() + n);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now.get())));
    });
}

fn sub(n: usize) {
    let _ = NOW.try_with(|now| now.set(now.get().saturating_sub(n)));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            add(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        sub(layout.size());
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, size) };
        if !q.is_null() {
            if size > layout.size() {
                add(size);
                sub(layout.size());
            } else {
                sub(layout.size());
                add(size);
            }
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What `f` returns, and the most heap it held at once on this thread beyond what was live before it.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = NOW.with(Cell::get);
    PEAK.with(|peak| peak.set(base));
    let out = f();
    (out, PEAK.with(Cell::get) - base)
}

#[test]
fn a_path_is_built_into_the_room_its_motion_needs_never_doubling() {
    // A 4 MB path must not hold 8 MB while its buffer doubles. Half an hour of motion, the take's rests found once.
    let track = common::random_path(11, 1_800_000_000);
    let rests = Rests::of(&track);
    for s in [Smoothing::Light, Smoothing::Medium, Smoothing::Strong] {
        let (path, peak) = peak_of(|| SmoothedPath::build_with(&track, s, &rests));
        let kept = path.bytes();
        assert!(kept > 500_000, "{s:?}: {kept}");
        assert!(
            peak <= kept + kept / 4 + 64 * 1024,
            "{s:?}: peaked at {peak} B for {kept} B"
        );
        eprintln!("{s:?}: {kept} B kept, {peak} B at the peak");
    }
}

#[test]
fn a_path_cut_at_its_cap_hands_back_to_the_raw_path_without_a_jump() {
    // At the cap (here 600 steps, 5 s) the drawn cursor must not snap from the smoothed path to the raw one.
    let track = common::random_path(5, 30_000_000);
    let rests = Rests::of(&track);
    let capped = SmoothedPath::build_capped(&track, Smoothing::Strong, &rests, 600);
    let whole = SmoothedPath::build_with(&track, Smoothing::Strong, &rests);
    assert_eq!(capped.len(), 600);
    // Where the capped path's last segment ends.
    let end = (0..30_000_000)
        .step_by(1_000)
        .filter_map(|t| capped.segment_at(t))
        .last()
        .map(|(_, end)| end)
        .unwrap();
    let ease = (4.0 / 12.0 * 1e6) as i64;
    // Somewhere in the ease the whole path is off the raw one, so a snap would show…
    let off = (end - ease..end)
        .step_by(STEP_US as usize)
        .map(|t| {
            let (a, b) = (
                whole.position(&track, t).unwrap(),
                track.position(t).unwrap(),
            );
            (a.0 - b.0).hypot(a.1 - b.1)
        })
        .fold(0.0, f32::max);
    assert!(off > PX, "the uncapped path is {off} off there");
    // …and the capped one lands on it, with nothing bigger than the raw path's own step either side.
    for t in (end - ease..end + 50_000).step_by(1_000) {
        let (a, b) = (
            capped.position(&track, t).unwrap(),
            capped.position(&track, t + 1_000).unwrap(),
        );
        let (ra, rb) = (
            track.position(t).unwrap(),
            track.position(t + 1_000).unwrap(),
        );
        let (drawn, raw) = (
            (a.0 - b.0).hypot(a.1 - b.1),
            (ra.0 - rb.0).hypot(ra.1 - rb.1),
        );
        assert!(
            drawn <= raw + off / 4.0 + 0.25 * PX,
            "{t}: {drawn} against {raw}"
        );
    }
    let (a, b) = (
        capped.position(&track, end).unwrap(),
        track.position(end).unwrap(),
    );
    assert!(
        (a.0 - b.0).abs() <= 0.25 * PX && (a.1 - b.1).abs() <= 0.25 * PX,
        "{a:?} {b:?}"
    );
}

#[test]
fn times_at_the_readers_limit_smooth_without_overflowing() {
    // `t + lead`, the step and a segment's end must not be plain adds, which a debug build's loader thread would
    // panic on. The reader keeps times up to `TIME_LIMIT_US`; a button held to the end keeps the last segment open.
    let mut lines = vec![head()];
    let from = TIME_LIMIT_US - 2_000_000;
    for k in 0..=500 {
        let t = from + k * 4_000;
        lines.push(Line::Move {
            t,
            x: 0.2 + 0.001 * (k % 50) as f32,
            y: 0.5,
        });
    }
    lines.push(Line::Button {
        t: from + 1_000_000,
        btn: 0,
        d: 1,
        x: 0.25,
        y: 0.5,
    });
    let track = read(&file(&lines));
    assert_eq!(track.span(), Some((from, TIME_LIMIT_US)));
    for s in [Smoothing::Light, Smoothing::Medium, Smoothing::Strong] {
        let path = SmoothedPath::build(&track, s);
        for t in [
            from,
            from + 1_500_000,
            TIME_LIMIT_US,
            TIME_LIMIT_US + 10_000_000,
            i64::MAX,
        ] {
            assert!(path.position(&track, t).is_some(), "{s:?} at {t}");
            let _ = path.velocity(&track, t);
            let _ = path.segment_at(t);
        }
    }
}
