use klipolo_capture::clock::SessionClock;

const S: u64 = 1_000_000_000;

#[test]
fn samples_before_the_start_and_inside_a_pause_are_dropped() {
    let mut c = SessionClock::new();
    assert_eq!(c.session_us(5 * S), None);
    c.start(100 * S);
    assert_eq!(c.session_us(99 * S), None);
    c.pause(110 * S);
    c.resume(115 * S);
    assert_eq!(c.session_us(112 * S), None, "inside the pause");
    assert_eq!(
        c.session_us(116 * S),
        Some(11_000_000),
        "a pause leaves no gap"
    );
}

#[test]
fn a_late_sample_stamped_before_the_pause_keeps_its_time() {
    let mut c = SessionClock::new();
    c.start(100 * S);
    c.pause(110 * S);
    assert_eq!(c.session_us(109 * S + S / 2), Some(9_500_000));
}

#[test]
fn every_pause_of_a_long_take_leaves_its_time_out_and_repeated_presses_change_nothing() {
    // An hour of gameplay with an auto-pause every 10 s: 1 s paused each time, 360 pauses.
    let mut c = SessionClock::new();
    c.pause(S); // before the start: ignored
    c.start(0);
    for k in 0..360 {
        c.pause(k * 10 * S + 9 * S);
        c.pause(k * 10 * S + 9 * S + S / 2); // a second press while paused: ignored
        c.resume(k * 10 * S + 10 * S);
        c.resume(k * 10 * S + 10 * S + S / 2); // resume while running: ignored
    }
    assert_eq!(
        c.session_us(1_805 * S),
        Some(1_625_000_000),
        "180 pauses before it"
    );
    assert_eq!(c.session_us(1_809 * S + S / 2), None, "inside pause 181");
    assert_eq!(
        c.elapsed_us(1_809 * S + S / 2),
        1_629_000_000,
        "frozen at pause 181"
    );
    assert_eq!(
        c.session_us(1_808 * S),
        Some(1_628_000_000),
        "a late sample from before it"
    );
    assert_eq!(
        c.session_us(3_601 * S),
        Some(3_241_000_000),
        "after the last"
    );
    assert!(!c.paused());
}

#[test]
fn elapsed_time_freezes_while_paused() {
    let mut c = SessionClock::new();
    c.start(0);
    c.pause(10 * S);
    assert_eq!(c.elapsed_us(30 * S), 10_000_000);
    c.resume(30 * S);
    assert_eq!(c.elapsed_us(31 * S), 11_000_000);
}
