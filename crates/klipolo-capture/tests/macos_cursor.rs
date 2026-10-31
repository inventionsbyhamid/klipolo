//! The macOS cursor source's pure helpers: its monitor's mask and its self-checks (a
//! monitor that misses buttons or moves, a stalled main thread, the event clock). The shape looks and the schedule are
//! `macos_cursor_looks.rs`'. No monitor is installed and no cursor is read: those run only inside a take the person
//! started. Pure, so every OS runs them.

use klipolo_capture::platform::common::cursor_checks::{
    ClockOffset, Gating, JUDGE_AFTER_NS, Why, ping_due,
};
use klipolo_capture::platform::common::mouse_mask::{NS_KEY_BITS, mouse_mask};

const MS: u64 = 1_000_000;
const S: u64 = 1_000 * MS;

#[test]
fn the_mask_has_mouse_bits_and_no_key_bits() {
    for bit in [1, 2, 3, 4, 5, 6, 7, 25, 26, 27] {
        assert_ne!(mouse_mask() & 1 << bit, 0, "bit {bit}");
    }
    assert_eq!(mouse_mask() & NS_KEY_BITS, 0);
}

#[test]
fn a_button_edge_the_monitor_missed_switches_to_sampled_clicks() {
    let mut g = Gating::new();
    let mut not_ours = |_| false;
    let at = |s: f64| (s * 1e9) as u64;
    // Judged with the main queue live (it ran just now).
    let judge = |g: &mut Gating, t: u64, own: &mut dyn FnMut((f64, f64)) -> bool| {
        let now = t + JUDGE_AFTER_NS;
        g.check(now, (10.0, 10.0), (0, now), &mut |p| own(p))
    };
    // A down the monitor saw a few ms before its 60 Hz sample: matched.
    g.monitor_down(at(1.0) - 5 * MS, 0);
    g.down_edge(at(1.0) - 17 * MS, at(1.0), 0, (10.0, 10.0));
    assert_eq!(judge(&mut g, at(1.0), &mut not_ours), None);
    assert_eq!(g.misses(), 0);
    // A down the monitor never sent, over Klipolo's own HUD (Apple gives a global monitor none): not a miss.
    g.down_edge(at(2.0) - 17 * MS, at(2.0), 0, (10.0, 10.0));
    assert_eq!(judge(&mut g, at(2.0), &mut |_| true), None);
    // A right down the monitor saw as a left one doesn't match; a miss isn't judged before its time.
    g.monitor_down(at(3.0), 0);
    g.down_edge(at(3.0) - 17 * MS, at(3.0), 1, (10.0, 10.0));
    let early = at(3.0) + 10 * MS;
    assert_eq!(
        g.check(early, (10.0, 10.0), (0, early), &mut not_ours),
        None
    );
    assert_eq!(judge(&mut g, at(3.0), &mut not_ours), None);
    assert_eq!(g.misses(), 1);
    // A pause drops a down not judged yet (the click that paused).
    g.down_edge(at(4.0) - 17 * MS, at(4.0), 0, (10.0, 10.0));
    g.reset(at(4.0) + MS, (10.0, 10.0));
    assert_eq!(judge(&mut g, at(4.0), &mut not_ours), None);
    // A main thread stalled since the down (a busy Mac) isn't the monitor missing it: it waits, then gives up.
    g.down_edge(at(5.0) - 17 * MS, at(5.0), 0, (10.0, 10.0));
    let stalled = (0, at(5.0) - 10 * MS);
    let t = at(5.0) + JUDGE_AFTER_NS;
    assert_eq!(g.check(t, (10.0, 10.0), stalled, &mut not_ours), None);
    assert!(g.waiting());
    let t = at(5.0) + 2 * S;
    assert_eq!(g.check(t, (10.0, 10.0), stalled, &mut not_ours), None);
    assert!(!g.waiting() && g.misses() == 1, "dropped unjudged");
    // The second miss switches.
    g.down_edge(at(8.0) - 100 * MS, at(8.0), 0, (10.0, 10.0));
    assert_eq!(
        judge(&mut g, at(8.0), &mut not_ours),
        Some(Why::MissedButtons)
    );
}

#[test]
fn a_stalled_main_queue_gets_no_more_blocks_until_it_runs_the_last() {
    // The sampler's block to the main queue is sent again only once the last one ran.
    assert!(ping_due(0, 0), "none sent yet");
    assert!(!ping_due(S, 0), "sent, not run");
    assert!(
        !ping_due(S, S - MS),
        "still not run: no pile-up during a stall"
    );
    assert!(ping_due(S, S + MS), "it ran");
}

#[test]
fn two_misses_switch_only_within_a_minute() {
    // Two unrelated misses an hour apart (a menu click, the lock screen) never switch a long take.
    let mut g = Gating::new();
    let miss = |g: &mut Gating, at: u64| {
        g.down_edge(at - 17 * MS, at, 0, (10.0, 10.0));
        let now = at + JUDGE_AFTER_NS;
        g.check(now, (10.0, 10.0), (0, now), &mut |_| false)
    };
    assert_eq!(miss(&mut g, 10 * S), None);
    assert_eq!(miss(&mut g, 71 * S), None, "61 s after the first");
    assert_eq!(g.misses(), 2);
    assert_eq!(
        miss(&mut g, 130 * S),
        Some(Why::MissedButtons),
        "59 s after the second"
    );
}

#[test]
fn a_pointer_that_moves_while_the_monitor_says_nothing_for_a_second_switches_to_sampled_clicks() {
    let mut g = Gating::new();
    let mut own = |p: (f64, f64)| p.0 > 1_000.0;
    // The monitor's last event at `monitor`, the main queue running now.
    let check = |g: &mut Gating,
                 now: u64,
                 x: f64,
                 monitor: u64,
                 own: &mut dyn FnMut((f64, f64)) -> bool| {
        g.check(now, (x, 100.0), (monitor, now), &mut |p| own(p))
    };
    // The monitor's last event at 1 s; the pointer then moves 1.5 points: not enough.
    assert_eq!(check(&mut g, S, 100.0, S, &mut own), None);
    assert_eq!(check(&mut g, 3 * S, 101.5, S, &mut own), None);
    // The monitor speaks again at 3 s; then 3 points of motion over 0.9 s: too soon.
    assert_eq!(check(&mut g, 3 * S, 101.5, 3 * S, &mut own), None);
    assert_eq!(check(&mut g, 3_900 * MS, 104.5, 3 * S, &mut own), None);
    // Over Klipolo's own window the monitor hears nothing: the quiet starts again there.
    assert_eq!(check(&mut g, 4 * S, 1_500.0, 3 * S, &mut own), None);
    assert_eq!(check(&mut g, 4_500 * MS, 1_510.0, 3 * S, &mut own), None);
    // A stalled main thread (it last ran at 4.2 s) can't deliver anything: no verdict.
    let stalled = (3 * S, 4_200 * MS);
    assert_eq!(g.check(5_100 * MS, (200.0, 100.0), stalled, &mut own), None);
    // A full second of motion with no event, the main queue running.
    assert_eq!(
        check(&mut g, 5_100 * MS, 200.0, 3 * S, &mut own),
        Some(Why::NoMoves)
    );
}

#[test]
fn the_event_clock_is_corrected_only_past_a_second() {
    // The expected case: the uptime clock, events 5-15 ms old on arrival. Times are kept as stamped.
    let mut c = ClockOffset::default();
    for k in 0..30u64 {
        let ts = 10 * S + k * 20 * MS;
        assert_eq!(c.stamp(ts, ts + (5 + k % 10) * MS), ts);
    }
    assert_eq!(c.take_decided(), Some(0));
    assert_eq!(c.take_decided(), None, "told once");
    // A clock 40 s behind (time asleep): the first 19 events take their arrival; the 20th decides, and from it each
    // event takes its time plus the smallest lag.
    let mut c = ClockOffset::default();
    let lag = |k: u64| 40 * S + (k % 7) * MS;
    for k in 0..19u64 {
        let ts = S + k * 10 * MS;
        assert_eq!(c.stamp(ts, ts + lag(k)), ts + lag(k));
    }
    let ts = 2 * S;
    let first = c.stamp(ts, ts + lag(19));
    let offset = c.take_decided().unwrap();
    assert!((40 * S as i64..40 * S as i64 + 7 * MS as i64).contains(&offset));
    assert_eq!(first, ts + offset as u64);
    // Never after its arrival.
    assert_eq!(c.stamp(ts, ts + 40 * S), ts + 40 * S);
}

#[test]
fn a_stall_over_the_first_events_never_decides_a_false_offset() {
    // A 1.3 s stall at the take's start queues the first events; all 20 arrive over 1 s late.
    let mut c = ClockOffset::default();
    let (start, arrive) = (10 * S, 10 * S + 1_300 * MS);
    for k in 0..20u64 {
        c.stamp(start + k * 8 * MS, arrive);
    }
    // Then prompt deliveries: a stall only adds lag, so the smallest lag is the clock's.
    for k in 0..5u64 {
        let ts = arrive + 50 * MS + k * 8 * MS;
        c.stamp(ts, ts + 6 * MS);
    }
    let down = 20 * S;
    assert_eq!(
        c.stamp(down, down + 1_050 * MS),
        down,
        "a later stalled down keeps its time"
    );
    assert_eq!(c.offset(), Some(0));
    // A stale stamp, far from the clock's lag, takes its arrival.
    assert_eq!(c.stamp(down, down + 30 * S), down + 30 * S);
}

#[test]
fn a_stalled_main_thread_keeps_a_click_in_order_and_counts_no_miss() {
    // The expected clock (decided 0); then a 1.05 s stall delivers a down and its up back to back.
    let mut c = ClockOffset::default();
    for k in 0..20u64 {
        let ts = S + k * 10 * MS;
        c.stamp(ts, ts + 8 * MS);
    }
    assert_eq!(c.take_decided(), Some(0));
    let (down, up) = (5 * S, 5 * S + 150 * MS);
    let arrive = down + 1_050 * MS;
    assert_eq!(
        (c.stamp(down, arrive), c.stamp(up, arrive)),
        (down, up),
        "each its own time: the down before the up"
    );
    // The self-check: the edge sampled 10 ms after the down, the monitor's down delivered 1.5 s late.
    let mut g = Gating::new();
    g.down_edge(down - 7 * MS, down + 10 * MS, 0, (10.0, 10.0));
    let stalled = (0, down - 20 * MS);
    assert_eq!(
        g.check(down + 200 * MS, (10.0, 10.0), stalled, &mut |_| false),
        None
    );
    let late = down + 1_500 * MS;
    g.monitor_down(c.stamp(down, late), 0);
    assert_eq!(
        g.check(late + 10 * MS, (10.0, 10.0), (late, late), &mut |_| false),
        None
    );
    assert_eq!(g.misses(), 0);
    // Before the offset is decided, only a sleep-sized lag (another clock base) takes its arrival.
    let mut c = ClockOffset::default();
    assert_eq!(c.stamp(S, 12 * S), 12 * S);
    assert_eq!(c.stamp(S, 3 * S), S, "2 s isn't sleep-sized");
}
