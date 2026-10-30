//! The visual activity track at real scale: the grid differ's cost a frame, and two hours of ticks through
//! the file and the derivations, in release.

mod common;

use common::activity::{Frame, S, Scene, best, file_of, two_hours};
use klipolo_cursor::activity::{ActivityTrack, Differ, TICK_BYTES, decode, media, typing};
use klipolo_cursor::facts::StillRuns;

#[test]
#[ignore = "real scale: two hours of activity"]
fn two_hours_of_activity_are_made_read_and_understood_quickly() {
    let release = |budget: u128| {
        if cfg!(debug_assertions) {
            budget * 50
        } else {
            budget
        }
    };
    // The differ: a busy screen (a typed line, a caret, a counter and a video), and the worst case, every cell
    // changing every frame (a whole-screen flash).
    let busy = Scene {
        typed: 50,
        caret: true,
        counter: true,
        video: 1_000,
    };
    let mut rng = common::Rng::new(9);
    let screen: Vec<Box<Frame>> = (0..8)
        .map(|k| {
            let mut f = Box::new([0; 128 * 72]);
            busy.frame(k * 7, &mut rng, &mut f);
            f
        })
        .collect();
    let flash = [Box::new([0; 128 * 72]), Box::new([255; 128 * 72])];
    let per_frame = |frames: &[Box<Frame>]| {
        let mut differ = Differ::new();
        let pushes = best(&mut || {
            for k in 0..10_000 {
                let frame = std::hint::black_box(&frames[k % frames.len()]);
                std::hint::black_box(differ.push(frame));
            }
        });
        pushes / 10_000
    };
    let (typical, worst) = (per_frame(&screen), per_frame(&flash));

    // Two hours through the file, and what the ticks mean.
    let ticks = two_hours();
    let bytes = file_of(&ticks);
    let mut track = ActivityTrack::default();
    let read = best(&mut || track = decode(std::hint::black_box(&bytes), 7_200 * S));
    let still = StillRuns::of(&common::random_path(11, 7_200 * S));
    let mut found = (0, 0, 0);
    let derive = best(&mut || {
        found = (
            typing(&track, &still, 0).len(),
            media(&track).len(),
            track.quiet_spans(S).len(),
        );
    });
    eprintln!(
        "differ {typical:?} a frame ({worst:?} when every cell changes); 2 h: {} file bytes, {} in memory, read in {read:?}; typing, media and quiet \
         {found:?} in {derive:?}",
        bytes.len(),
        track.bytes()
    );
    assert!(track.is_complete() && found.0 > 0 && found.1 > 0 && found.2 > 0);
    // Measured on a 2013 Xeon server, shared: 6 µs and 30 µs a frame; 1.1 ms to read; 6.3 ms to derive.
    assert!(
        typical.as_micros() < release(25) && worst.as_micros() < release(100),
        "the differ: {typical:?} a frame, {worst:?} at worst"
    );
    assert!(read.as_millis() < release(5), "reading 2 h: {read:?}");
    assert!(derive.as_millis() < release(20), "deriving 2 h: {derive:?}");
    // ≤ 0.5 MB an hour, on disk and in memory.
    assert!(bytes.len() <= 2 * (36_000 * TICK_BYTES + 60 * 10) + 20);
    assert!(track.bytes() <= 2 * 36_000 * TICK_BYTES + 1_024);
}
