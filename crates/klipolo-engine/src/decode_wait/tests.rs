//! The progress wait on a fake decoder: a counter a task ticks, and an answer sent (or not) at a set time.

use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::Instant;

use super::{DecodeBeat, DecodeProgress, DecodeWait, Waited};

const S: fn(u64) -> Duration = Duration::from_secs;

fn progress_wait(stall_after: u64, cap: u64) -> DecodeWait {
    DecodeWait::Progress {
        stall_after: S(stall_after),
        cap: S(cap),
    }
}

/// A fake decoder: ticks its progress at each of `ticks` (seconds from now), and answers at `answer_at` if given.
fn decoder(ticks: Vec<f64>, answer_at: Option<f64>) -> (DecodeProgress, oneshot::Receiver<u32>) {
    let progress = DecodeProgress::default();
    let (tx, rx) = oneshot::channel();
    let ticking = progress.clone();
    tokio::spawn(async move {
        let start = Instant::now();
        for at in ticks {
            tokio::time::sleep_until(start + Duration::from_secs_f64(at)).await;
            ticking.tick();
        }
    });
    tokio::spawn(async move {
        if let Some(at) = answer_at {
            tokio::time::sleep(Duration::from_secs_f64(at)).await;
            let _ = tx.send(210);
        } else {
            // Holds the request without answering (a reader blocked in its call).
            tokio::time::sleep(S(3_600)).await;
            drop(tx);
        }
    });
    (progress, rx)
}

fn every_second(until: u64) -> Vec<f64> {
    (1..=until).map(|s| s as f64).collect()
}

/// Export waits as long as the decoder keeps moving: a frame 20 s away is waited out (no 9 s, let alone 3 s, limit),
/// and the heartbeat hears it waited on a decoder that moves.
#[tokio::test(start_paused = true)]
async fn a_slow_decoder_that_keeps_moving_is_waited_out() {
    let (progress, rx) = decoder(every_second(30), Some(20.0));
    let beat = DecodeBeat::default();
    let started = Instant::now();
    let got = progress_wait(3, 60)
        .wait(rx, S(3), &progress, Some(&beat))
        .await;
    assert!(matches!(got, Waited::Got(210)), "{got:?}");
    assert_eq!(started.elapsed(), S(20));
    assert!(beat.take(), "the heartbeat heard it");
    assert!(!beat.take(), "taken");
}

/// No progress for the stall window: stalled then, not at the cap.
#[tokio::test(start_paused = true)]
async fn a_decoder_that_stops_moving_is_stalled_after_the_window() {
    let (progress, rx) = decoder(Vec::new(), None);
    let beat = DecodeBeat::default();
    let started = Instant::now();
    let got = progress_wait(3, 60)
        .wait(rx, S(3), &progress, Some(&beat))
        .await;
    assert!(
        matches!(got, Waited::Stalled(waited) if waited == S(3)),
        "{got:?}"
    );
    assert_eq!(started.elapsed(), S(3));
    assert!(!beat.take(), "nothing moved");
}

/// Progress restarts the window: ticks at 2 s and 4 s, then nothing, stall after 3 s more (seen within a slice).
#[tokio::test(start_paused = true)]
async fn each_tick_starts_the_stall_window_again() {
    let (progress, rx) = decoder(vec![2.0, 4.0], None);
    let started = Instant::now();
    let got = progress_wait(3, 60).wait(rx, S(3), &progress, None).await;
    let waited = started.elapsed();
    assert!(matches!(got, Waited::Stalled(_)), "{got:?}");
    assert!(
        waited >= S(7) && waited <= Duration::from_millis(7_500),
        "{waited:?}"
    );
}

/// A decoder that moves but never answers is stopped at the cap.
#[tokio::test(start_paused = true)]
async fn the_cap_ends_a_wait_however_the_decoder_moves() {
    let (progress, rx) = decoder(every_second(120), None);
    let started = Instant::now();
    let got = progress_wait(3, 10).wait(rx, S(3), &progress, None).await;
    assert!(
        matches!(got, Waited::Stalled(waited) if waited == S(10)),
        "{got:?}"
    );
    assert_eq!(started.elapsed(), S(10));
}

/// The preview's fixed deadline doesn't move with progress: 3 s, as Cap's.
#[tokio::test(start_paused = true)]
async fn the_fixed_deadline_ignores_progress() {
    let (progress, rx) = decoder(every_second(30), Some(6.0));
    let started = Instant::now();
    let got = DecodeWait::Fixed.wait(rx, S(3), &progress, None).await;
    assert!(
        matches!(got, Waited::Stalled(waited) if waited == S(3)),
        "{got:?}"
    );
    assert_eq!(started.elapsed(), S(3));
}

/// A decoder that lets the request go is said at once, apart from a stall.
#[tokio::test(start_paused = true)]
async fn a_dropped_request_is_not_a_stall() {
    let progress = DecodeProgress::default();
    let (tx, rx) = oneshot::channel::<u32>();
    drop(tx);
    let started = Instant::now();
    let got = progress_wait(3, 60).wait(rx, S(3), &progress, None).await;
    assert!(matches!(got, Waited::Dropped), "{got:?}");
    assert_eq!(started.elapsed(), Duration::ZERO);
}

/// A frame answered at once never sets the heartbeat (only waits on a moving decoder do).
#[tokio::test(start_paused = true)]
async fn a_quick_answer_sets_no_heartbeat() {
    let (progress, rx) = decoder(vec![0.01, 0.02], Some(0.05));
    let beat = DecodeBeat::default();
    let got = progress_wait(3, 60)
        .wait(rx, S(3), &progress, Some(&beat))
        .await;
    assert!(matches!(got, Waited::Got(210)));
    assert!(!beat.take());
}

/// A decoder's start gets its 30 s window whatever export's stall window is (the open ticks nothing until it
/// is done), and the heartbeat hears it meanwhile.
#[tokio::test(start_paused = true)]
async fn a_start_gets_its_window_before_progress_counts() {
    let (progress, rx) = decoder(Vec::new(), None);
    let beat = DecodeBeat::default();
    let started = Instant::now();
    let got = progress_wait(3, 45)
        .wait_start(rx, &progress, Some(&beat))
        .await;
    assert!(matches!(got, Waited::Stalled(_)), "{got:?}");
    assert_eq!(started.elapsed(), super::START_WINDOW);
    assert!(beat.take(), "a bounded start keeps the heartbeat");
}

/// A start answered inside its window, with no progress at all, is answered.
#[tokio::test(start_paused = true)]
async fn a_start_answered_inside_its_window_is_got() {
    let (progress, rx) = decoder(Vec::new(), Some(20.0));
    let started = Instant::now();
    let got = progress_wait(3, 45).wait_start(rx, &progress, None).await;
    assert!(matches!(got, Waited::Got(210)), "{got:?}");
    assert_eq!(started.elapsed(), S(20));
}

/// After its window a start waits as a request does: on while it moves (to the cap), stalled once it stops.
#[tokio::test(start_paused = true)]
async fn after_its_window_a_start_waits_while_it_moves() {
    let (progress, rx) = decoder((28..=60).map(f64::from).collect(), None);
    let started = Instant::now();
    let got = progress_wait(3, 45).wait_start(rx, &progress, None).await;
    assert!(
        matches!(got, Waited::Stalled(waited) if waited == S(45)),
        "{got:?}"
    );
    assert_eq!(started.elapsed(), S(45));

    // Moving at the window's end: on to 3 s after its last tick.
    let (progress, rx) = decoder(vec![29.0, 31.0, 32.0], None);
    let started = Instant::now();
    let got = progress_wait(3, 45).wait_start(rx, &progress, None).await;
    let waited = started.elapsed();
    assert!(matches!(got, Waited::Stalled(_)), "{got:?}");
    assert!(
        waited >= S(35) && waited <= Duration::from_millis(35_500),
        "{waited:?}"
    );
}

/// A bounded step of a recovery (a stat, an open) keeps the heartbeat while it runs.
#[tokio::test(start_paused = true)]
async fn a_bounded_step_keeps_the_heartbeat() {
    let beat = DecodeBeat::default();
    let value = beat
        .alive_while(async {
            tokio::time::sleep(S(7)).await;
            7
        })
        .await;
    assert_eq!(value, 7);
    assert!(beat.take());
    assert!(!beat.take());
}
