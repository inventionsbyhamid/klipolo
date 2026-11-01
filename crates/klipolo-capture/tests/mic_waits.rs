//! A mic that waits for its own device, on a computer with nothing to fall back to (a Mac mini, a desktop with only a
//! USB mic): the core's poll of the wait costs nothing per input, and the HUD flashes for the loss at most once a
//! minute. Virtual time; this binary counts the allocations its own
//! test thread makes, so it has its own global allocator.
#![cfg(feature = "synthetic")]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::harness::{Harness, Src, cfg, mics};
use klipolo_capture::pipe::PipeEvent;
use klipolo_capture::protocol::Event;
use klipolo_capture::shell::{Flash, ShellInput, ShellOutput};

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

/// Recording with the USB mic as the only mic: nothing to fall back to.
fn only_the_usb_mic() -> Harness {
    let only_mine = mics().into_iter().filter(|m| m.id == "mic-usb").collect();
    let mut h = Harness::new_with_mics(cfg(&[Src::Screen, Src::Mic], 3), only_mine);
    h.start_recording();
    h.shell_out.clear();
    h
}

/// How many times the mic was said to be lost.
fn lost(h: &Harness) -> usize {
    let lost =
        |e: &&Event| matches!(e, Event::Health { note, .. } if note.code == "capture.health.lost");
    h.events().iter().filter(lost).count()
}

#[test]
fn a_mic_waiting_for_its_device_costs_no_allocation_per_input() {
    let mut h = only_the_usb_mic();
    h.feed_for(2.0);
    h.pipe_event("mic", PipeEvent::DeviceLost);
    // Past the 3 s: it said it is lost, and waits for its own device, polled on every input.
    h.feed_for(5.0);
    assert_eq!(lost(&h), 1, "noted once");
    // An input with nothing to do (the frontmost app changed; no game to pause for), between ticks.
    h.now += 1_000_000;
    let now = h.now;
    let (fx, n) = allocations(|| h.core.shell(ShellInput::Focus { app_id: None }, now));
    assert!(fx.is_empty(), "{fx:?}");
    assert_eq!(n, 0, "the wait's poll allocated");
}

#[test]
fn a_mic_with_nothing_to_fall_back_to_flashes_for_its_losses_at_most_once_a_minute() {
    let mut h = only_the_usb_mic();
    h.feed_for(2.0);
    // A gamer's USB mic dropping for 4 s, three times in a minute: each loss is noted, the HUD flashes once.
    for _ in 0..3 {
        h.pipe_event("mic", PipeEvent::DeviceLost);
        h.feed_for(4.0);
        h.pipe_event("mic", PipeEvent::DeviceBack);
        h.feed_for(10.0);
    }
    let problems = |h: &Harness| {
        (h.shell_out.iter())
            .filter(|o| matches!(o, ShellOutput::Flash(Flash::Problem)))
            .count()
    };
    assert_eq!((lost(&h), problems(&h)), (3, 1));
    // A minute after the first flash, the next loss flashes again.
    h.feed_for(20.0);
    h.pipe_event("mic", PipeEvent::DeviceLost);
    h.feed_for(4.0);
    assert_eq!(problems(&h), 2);
}
