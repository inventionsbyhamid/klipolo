//! The style the cursor draws with is resolved for every frame (`p.cursor_for(clip).effective()`), inside a path
//! that allocates nothing. This binary counts the allocations its own test thread makes, so it has its
//! own global allocator.
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::fixture;
use klipolo_model::cursor_style::CursorStyle;

/// Counts the allocations a thread makes while its `COUNTING` flag is set; other threads (the test runner's) aren't.
struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

// SAFETY: every call is passed to the system allocator unchanged; counting only reads a thread-local flag.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
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
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    COUNTING.with(|c| c.set(true));
    let out = f();
    COUNTING.with(|c| c.set(false));
    (out, ALLOCATIONS.load(Ordering::Relaxed) - before)
}

#[test]
fn resolving_the_drawn_style_every_frame_allocates_nothing() {
    let mut p = fixture();
    // Clip "a" draws as recorded (its own style); clip "b" the project's.
    p.timeline.main[0].cursor = Some(CursorStyle {
        as_recorded: true,
        ..CursorStyle::default()
    });
    let (sizes, n) = allocations(|| {
        let mut sizes = 0.0;
        for frame in 0..1_000 {
            let clip = &p.timeline.main[frame % 2];
            // Both ends are black-boxed, so an optimised build can't elide an allocation the result would make.
            let drawn = std::hint::black_box(std::hint::black_box(p.cursor_for(clip)).effective());
            sizes += drawn.size;
        }
        sizes
    });
    assert_eq!(n, 0, "1,000 frames allocated {n} times");
    assert_eq!(sizes, 500.0 * 1.0 + 500.0 * 1.25);
}
