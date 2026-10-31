//! The macOS cursor's shape looks and schedule, pure: its shapes' sizes, no current cursor, the cursor seed,
//! the 64-shape limit, animated cursors, and the sampler's pace. No cursor is read: that runs only inside a take the
//! person started. Pure, so every OS runs them.

use klipolo_capture::platform::common::cursor_looks::{
    Animation, NilRun, SeedReads, SeedTrust, Sent, ShapeSends,
};
use klipolo_capture::platform::common::cursor_pace::{
    FAST_PASS_NS, Pace, SLOW_PASS_NS, beat_due, shape_px, top_left, unpremultiply,
};

const MS: u64 = 1_000_000;
const S: u64 = 1_000 * MS;

#[test]
fn a_vector_cursor_renders_at_eight_times_its_points_capped_at_256() {
    assert_eq!(shape_px((17.0, 23.0), 8.0), (136, 184));
    assert_eq!(shape_px((32.0, 32.0), 8.0), (256, 256));
    // 28 × 40 points is 224 × 320 at 8×: the longer side fits 256, the shape keeps its proportions.
    assert_eq!(shape_px((28.0, 40.0), 8.0), (179, 256));
    // A bitmap's own pixels (this Mac's 2× `currentSystem`, a 10× arrow) only ever shrink.
    assert_eq!(shape_px((18.0, 36.0), 1.0), (18, 36));
    assert_eq!(shape_px((170.0, 230.0), 1.0), (170, 230));
    assert_eq!(shape_px((0.0, 12.0), 8.0), (1, 1));
    // Drawn premultiplied, stored straight; AppKit's points flipped to Core Graphics' top-left space.
    let mut px = [64, 32, 0, 128, 10, 20, 30, 255, 9, 9, 9, 0];
    unpremultiply(&mut px);
    assert_eq!(px, [128, 64, 0, 128, 10, 20, 30, 255, 9, 9, 9, 0]);
    assert_eq!(top_left((100.0, 800.0), 900.0), (100.0, 100.0));
}

#[test]
fn no_current_cursor_is_unknown_for_five_seconds_then_the_arrow_and_a_seed_that_missed_a_change_is_dropped()
 {
    let mut nil = NilRun::default();
    assert!(!nil.nil(S) && !nil.nil(5_900 * MS));
    assert!(nil.nil(6 * S), "nil for 5 s straight");
    assert!(!nil.nil(7 * S), "once");
    assert!(nil.found(), "back from the arrow");
    assert!(!nil.nil(8 * S), "a new run of nils starts again");
    // A look's key against the seed read before it and again after it.
    let mut seed = SeedTrust::new(true);
    assert!(
        !seed.look(true, false, Some(1), 2),
        "the seed moved: a change it saw"
    );
    assert!(!seed.look(false, false, Some(2), 2), "nothing changed");
    assert!(
        !seed.look(false, true, Some(2), 3),
        "it moved during the look: the change raced the read"
    );
    assert!(seed.trusted());
    assert!(
        seed.look(false, false, Some(3), 4),
        "the key changed, the seed didn't"
    );
    assert!(!seed.trusted() && seed.missed() == 1);
    // It keeps counting once dropped, for the question.
    assert!(!seed.look(false, false, Some(4), 5));
    assert_eq!((seed.missed(), seed.seen()), (2, 2));
    assert!(
        !SeedTrust::new(false).trusted(),
        "no symbol: the public looks"
    );
}

#[test]
fn the_seed_read_after_a_look_leaves_its_move_for_the_next_pass_and_a_first_key_is_no_miss() {
    // The read after a look is a peek, so a change during the look still asks for a seed look.
    let mut reads = SeedReads::default();
    assert!(reads.take(1), "the first read counts as moved");
    assert!(!reads.peek(1));
    assert!(reads.peek(2), "it moved during the look");
    assert!(reads.take(2), "the next pass still sees the move");
    assert!(!reads.take(2));
    // The first look after no current cursor (or the first of all) has no key to compare: never a miss.
    let mut seed = SeedTrust::new(true);
    assert!(!seed.look(false, false, None, 5));
    assert_eq!((seed.missed(), seed.seen()), (0, 0));
    assert!(
        seed.look(false, false, Some(5), 6),
        "a real change the seed didn't show"
    );
}

#[test]
fn past_64_shapes_the_cursor_keeps_its_last_one_and_says_so_once() {
    // An animated cursor's frames fill the store.
    let mut s = ShapeSends::default();
    for key in 1..=64 {
        assert_eq!(s.next(key), Sent::New(key));
    }
    assert_eq!(s.next(64), Sent::Same);
    assert_eq!(s.next(3), Sent::Known(3));
    assert_eq!(s.next(65), Sent::Full, "told once");
    assert_eq!(s.next(66), Sent::Keep, "the last defined shape stays");
    assert_eq!(s.next(3), Sent::Known(3), "a known one still shows");
    assert_eq!(s.next(67), Sent::Keep);
    // A new key whose picture couldn't be read isn't counted as sent.
    let mut s = ShapeSends::default();
    assert_eq!(s.next(9), Sent::New(9));
    s.unread(9);
    assert_eq!(s.next(10), Sent::New(10));
    assert_eq!(s.next(9), Sent::New(9));
}

#[test]
fn an_animated_cursor_slows_its_seed_looks_until_its_key_repeats() {
    let mut a = Animation::default();
    // Four seed looks within a second, each a new frame: animating.
    let frames = [0, 40, 80, 120].map(|ms| ms * MS);
    let began: Vec<bool> = frames.iter().map(|&t| a.look(t, true, true)).collect();
    assert_eq!(began, [false, false, false, true]);
    assert!(a.on() && a.seed_look_ns() >= 333 * MS, "≤ 3 Hz");
    assert!(!a.look(500 * MS, true, true), "begins once");
    assert!(
        !a.look(900 * MS, false, false) && !a.on(),
        "the key repeated"
    );
    assert_eq!(a.seed_look_ns(), 33_333_333);
    // A person moving over a link, text and a button changes it four times in three seconds: not an animation.
    let mut a = Animation::default();
    for t in [0, 1_000, 2_000, 3_000] {
        assert!(!a.look(t * MS, true, true));
    }
    assert_eq!(a.started(), 0);
}

#[test]
fn the_sampler_slows_down_when_still_and_beats_after_half_a_second_of_nothing() {
    let mut p = Pace::default();
    assert!(p.look_due(0, true), "the first pass looks");
    assert_eq!(p.pass_ns(0, false), SLOW_PASS_NS);
    assert_eq!(p.pass_ns(0, true), FAST_PASS_NS, "a down to judge");
    p.moved(S);
    assert_eq!(p.pass_ns(1_500 * MS, false), FAST_PASS_NS);
    assert_eq!(p.pass_ns(2 * S, false), SLOW_PASS_NS, "still for a second");
    // With the seed: a safety look at 3 Hz moving, 1 Hz still; without it, 10 Hz and 3 Hz.
    let mut p = Pace::default();
    p.moved(0);
    assert!(p.look_due(0, true) && !p.look_due(300 * MS, true) && p.look_due(340 * MS, true));
    assert!(!p.look_due(430 * MS, false) && p.look_due(440 * MS, false));
    assert!(!p.look_due(1_430 * MS, true) && p.look_due(1_440 * MS, true));
    assert!(p.area_due(0) && !p.area_due(249 * MS) && p.area_due(250 * MS));
    assert!(!beat_due(S, 1_499 * MS) && beat_due(S, 1_500 * MS));
}
