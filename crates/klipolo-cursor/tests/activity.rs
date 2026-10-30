//! The visual activity track: synthetic 128 × 72 frames through the grid differ, and what the
//! ticks mean (typing, playing media, quiet). The file is `activity_file.rs`'s. This binary counts the allocations its
//! own test thread makes, so it has its own global allocator.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::activity::{
    Frame, S, Scene, activity_of, assert_cells, cells, moving, moving_then_resting, resting,
    tick_of, ticks_of, track_of,
};
use klipolo_cursor::Us;
use klipolo_cursor::activity::{Differ, TICK_US, Tick, media, players, typing};
use klipolo_cursor::facts::spans;

/// Counts the allocations a thread makes while its `COUNTING` flag is set; other threads (the test runner's) aren't.
struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: every call is passed to the system allocator unchanged; counting only touches thread-local cells.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            COUNT.with(|c| c.set(c.get() + 1));
        }
        // SAFETY: the caller's layout, as the system allocator requires.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: a pointer this allocator (the system's) gave, with its layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The allocations `f` makes on this thread.
fn allocations<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = COUNT.with(Cell::get);
    COUNTING.with(|c| c.set(true));
    let out = f();
    COUNTING.with(|c| c.set(false));
    (out, COUNT.with(Cell::get) - before)
}

#[test]
fn a_still_screen_gives_quiet_ticks() {
    let ticks = ticks_of(&Scene::default(), 100);
    assert_eq!(ticks.len(), 100);
    assert!(
        ticks
            .iter()
            .all(|t| t.is_known() && t.quiet() && t.changed == 0),
        "the ±2 noise changed a cell: {ticks:?}"
    );
    let a = activity_of(&ticks);
    assert!(a.is_complete());
    assert_eq!(a.complete_through(), 10 * S);
    assert_eq!(a.quiet_spans(S), vec![(0, 10 * S)]);
    assert_eq!(a.quiet_spans(11 * S), Vec::new());
    assert!(typing(&a, &resting(), 0).is_empty() && media(&a).is_empty());
    assert!(!Tick::UNKNOWN.is_known() && !Tick::UNKNOWN.quiet());
}

#[test]
fn a_typed_line_is_a_small_region_that_moves_right_and_reads_as_typing() {
    let a = track_of(
        &Scene {
            typed: 50,
            ..Scene::default()
        },
        80,
    );
    // Each tick of the 5 s changes the line's next column of cells: one small region, a cell further right each time.
    for i in 0..50u8 {
        let tick = a.tick_at(Us::from(i) * TICK_US).unwrap();
        let r = tick.top[0];
        assert_eq!(
            (r.x0, r.x1, r.y0, r.y1, r.cells, tick.changed),
            (4 + i, 4 + i, 10, 12, 3, 3),
            "tick {i}"
        );
        assert_eq!(tick.top[1].cells, 0);
    }
    assert!(a.tick_at(6 * S).unwrap().quiet());
    let found = typing(&a, &resting(), 0);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].from, found[0].to), (0, 5 * S));
    assert_cells(found[0].rect, (4, 10, 50, 3));
    assert!(media(&a).is_empty());
    // The same changes while the cursor moves aren't typing.
    assert!(typing(&a, &moving(), 0).is_empty());
}

#[test]
fn a_blinking_caret_alone_is_not_typing() {
    let a = track_of(
        &Scene {
            caret: true,
            ..Scene::default()
        },
        200,
    );
    // It toggles at 2 Hz: one lone cell changes every 5 ticks.
    let changed: Vec<Us> = (0..200)
        .filter(|&i| a.tick_at(i * TICK_US).unwrap().changed > 0)
        .collect();
    assert_eq!(changed, (4..200).step_by(5).collect::<Vec<_>>());
    assert!(
        changed
            .iter()
            .all(|&i| a.tick_at(i * TICK_US).unwrap().changed == 1)
    );
    assert!(typing(&a, &resting(), 0).is_empty());
    // Nor with a counter ticking once a second beside it: 3 changes a second between them, but the caret's blinks
    // aren't counted.
    let a = track_of(
        &Scene {
            caret: true,
            counter: true,
            ..Scene::default()
        },
        200,
    );
    assert!(
        typing(&a, &resting(), 0).is_empty(),
        "{:?}",
        typing(&a, &resting(), 0)
    );
    assert!(media(&a).is_empty());
}

#[test]
fn a_playing_video_region_is_media_not_typing() {
    // A video small enough to be typing by its size and rate plays for 15 s; a line is typed above it.
    let a = track_of(
        &Scene {
            typed: 40,
            video: 150,
            ..Scene::default()
        },
        160,
    );
    let playing = media(&a);
    assert_eq!(playing.len(), 1, "{playing:?}");
    assert_eq!((playing[0].from, playing[0].to), (0, 15 * S));
    assert_cells(playing[0].rect, (40, 20, 6, 4));
    let typed = typing(&a, &resting(), 0);
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert_eq!((typed[0].from, typed[0].to), (0, 4 * S));
    assert_cells(typed[0].rect, (4, 10, 40, 3));
    // Under 10 s it isn't media, so, small and busy, it reads as typing.
    let short = track_of(
        &Scene {
            video: 90,
            ..Scene::default()
        },
        100,
    );
    assert!(media(&short).is_empty());
    assert_eq!(typing(&short, &resting(), 0).len(), 1);
}

#[test]
fn the_differ_never_allocates_after_new() {
    let busy = Scene {
        typed: 50,
        caret: true,
        counter: true,
        video: 1_000,
    };
    let mut rng = common::Rng::new(3);
    let frames: Vec<Box<Frame>> = (0..8)
        .map(|k| {
            let mut f = Box::new([0; 128 * 72]);
            busy.frame(k * 3, &mut rng, &mut f);
            f
        })
        .collect();
    // A whole-screen change too: every cell in one region.
    let white = Box::new([255; 128 * 72]);
    let mut differ = Differ::new();
    let (ticks, n) = allocations(|| {
        let mut ticks = 0;
        for k in 0..10_000 {
            if k % 1_000 == 500 {
                differ.reset();
            }
            let frame = if k % 9 == 8 { &white } else { &frames[k % 8] };
            ticks += usize::from(differ.push(std::hint::black_box(frame)).is_some());
        }
        ticks
    });
    assert_eq!(n, 0, "10,000 pushes allocated {n} times");
    // The first push, and the first after each of the 10 resets, only prime it.
    assert_eq!(ticks, 10_000 - 11);
}

#[test]
fn spans_of_a_kind_never_overlap_in_time() {
    // Two videos at once: a large one for 15 s, and a small one from 3 s to 20 s.
    let (large, small) = (cells(2, 2, 20, 12), cells(40, 20, 8, 6));
    let videos: Vec<Tick> = (0..200)
        .map(|k| {
            let playing = [(k < 150).then_some(large), (k >= 30).then_some(small)];
            tick_of(&playing.into_iter().flatten().collect::<Vec<_>>())
        })
        .collect();
    let playing = media(&activity_of(&videos));
    assert_eq!(playing.len(), 1, "{playing:?}");
    assert_eq!((playing[0].from, playing[0].to), (0, 20 * S));
    assert_cells(playing[0].rect, (2, 2, 20, 12));
    // Each player on its own, as the follow-crop's typed part will read them.
    let each = players(&activity_of(&videos));
    assert_eq!(each.len(), 2, "{each:?}");
    assert_eq!((each[0].from, each[0].to, each[1].from), (0, 15 * S, 3 * S));
    assert_cells(each[1].rect, (40, 20, 8, 6));
    // Two lines typed at once: 30 cells across from 0 s, and 40 from 1 s to 5 s, further down.
    let typed: Vec<Tick> = (0..60u8)
        .map(|k| {
            let a = (k < 30).then(|| cells(2 + k, 5, 1, 3));
            let b = (10..50).contains(&k).then(|| cells(10 + k, 25, 1, 3));
            tick_of(&[a, b].into_iter().flatten().collect::<Vec<_>>())
        })
        .collect();
    let found = typing(&activity_of(&typed), &resting(), 0);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].from, found[0].to), (0, 5 * S));
    assert_cells(found[0].rect, (20, 25, 40, 3));
}

#[test]
fn a_later_screen_file_finds_typing_on_its_own_clock() {
    // The take's cursor moves until 60 s, then rests. A screen file starting 60 s into the take (a heal's, or a
    // resume's) shows a line typed over its first 5 s.
    let still = moving_then_resting(60 * S);
    let a = track_of(
        &Scene {
            typed: 50,
            ..Scene::default()
        },
        80,
    );
    let found = typing(&a, &still, 60 * S);
    assert_eq!(found.len(), 1, "{found:?}");
    // On the file's clock: the caller adds the 60 s.
    assert_eq!((found[0].from, found[0].to), (0, 5 * S));
    // Read against the take's first 5 s instead, the cursor was moving.
    assert!(typing(&a, &still, 0).is_empty());
}

#[test]
fn one_cell_keystrokes_are_typing_and_never_idle() {
    // At real text sizes a keystroke changes one cell: here one every 3 ticks, a cell further right each time, for 8 s,
    // then 5 s of nothing. Every tick is quiet (≤ 2 cells), the cursor rests and the sound is quiet.
    let ticks: Vec<Tick> = (0..130u8)
        .map(|k| match k {
            k if k < 80 && k % 3 == 0 => tick_of(&[cells(4 + k / 3, 10, 1, 1)]),
            _ => Tick::default(),
        })
        .collect();
    let a = activity_of(&ticks);
    let typed = typing(&a, &resting(), 0);
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert_eq!((typed[0].from, typed[0].to), (0, 7_900_000));
    assert_eq!(a.quiet_spans(S), vec![(0, 13 * S)]);
    let typed: Vec<(Us, Us)> = typed.iter().map(|t| (t.from, t.to)).collect();
    let idle = spans::idle(
        &[(0, Us::MAX)],
        &a.quiet_spans(S),
        &[(0, 30 * S)],
        &typed,
        S,
    );
    assert_eq!(idle, vec![(7_900_000, 13 * S)]);
}
