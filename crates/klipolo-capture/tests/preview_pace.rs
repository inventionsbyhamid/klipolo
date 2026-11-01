//! The previews' pace: Check camera's about 15 frames a second out of a camera's own
//! jittery spacing, the live view's 10 (more in a big view), never a frame closer than 0.8 of the interval
//! to the last.

use klipolo_capture::pace::{LIVE_VIEW_FPS, PREVIEW_FPS, PreviewPacer};

const MS: u64 = 1_000_000;

/// Check camera's frames taken from `fps` arrivals jittered by up to ±`jitter_ms`, over 10 s (`taken_at`).
fn taken(fps: u64, jitter_ms: i64) -> (usize, u64, u64) {
    taken_at(PreviewPacer::default(), fps, jitter_ms)
}

/// The frames `pacer` takes from `fps` arrivals jittered by up to ±`jitter_ms` (a fixed pseudo-random sequence), over
/// 10 s, and the shortest and longest gaps between two taken.
fn taken_at(mut pacer: PreviewPacer, fps: u64, jitter_ms: i64) -> (usize, u64, u64) {
    let mut seed = 0x2545_f491_u64;
    let (mut times, mut last) = (vec![], None::<u64>);
    for n in 0..fps * 10 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let jitter = ((seed >> 33) % (2 * jitter_ms as u64 + 1)) as i64 - jitter_ms;
        let at = (1_000 * MS + n * 1_000 * MS / fps).saturating_add_signed(jitter * MS as i64);
        if last.is_some_and(|l| at <= l) {
            continue;
        }
        last = Some(at);
        if pacer.take(at) {
            times.push(at);
        }
    }
    let gaps: Vec<u64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    let shortest = gaps.iter().copied().min().unwrap_or(u64::MAX);
    (
        times.len(),
        shortest,
        gaps.iter().copied().max().unwrap_or(0),
    )
}

#[test]
fn a_jittery_camera_gives_about_fifteen_frames_a_second() {
    let floor = 1_000 * MS / u64::from(PREVIEW_FPS) * 4 / 5;
    // A 30 fps camera, ±5 ms: every other frame.
    let (n, shortest, _) = taken(30, 5);
    assert!((147..=151).contains(&n), "{n} frames in 10 s");
    assert!(shortest >= floor, "{} ms apart", shortest / MS);
    // A 15 fps camera (the FaceTime camera indoors), ±5 ms: every frame.
    assert_eq!(taken(15, 5).0, 150);
}

#[test]
fn a_camera_between_15_and_18_75_fps_gives_an_even_preview() {
    // 18 fps, 55.6 ms apart (over the 53.3 ms floor): every frame, never a 55/111 ms alternation.
    let (n, shortest, longest) = taken(18, 1);
    assert_eq!(n, 180);
    assert!(longest < 60 * MS, "{} ms", longest / MS);
    assert!(shortest >= 1_000 * MS / u64::from(PREVIEW_FPS) * 4 / 5);
    // 20 fps, 50 ms apart (under the floor): every other frame, evenly.
    let (n, shortest, longest) = taken(20, 1);
    assert!((99..=101).contains(&n), "{n} frames in 10 s");
    assert!(
        shortest >= 97 * MS && longest <= 103 * MS,
        "{shortest} {longest}"
    );
}

#[test]
fn the_live_view_takes_ten_frames_a_second() {
    // ScreenCaptureKit's own 10 fps (its minimum frame interval), ±5 ms: every frame.
    let live = || PreviewPacer::at(LIVE_VIEW_FPS);
    assert_eq!(taken_at(live(), 10, 5).0, 100);
    // A faster stream, 30 fps ±2 ms: every third frame, never closer than 80 ms.
    let (n, shortest, _) = taken_at(live(), 30, 2);
    assert!((99..=101).contains(&n), "{n} frames in 10 s");
    assert!(shortest >= 80 * MS, "{} ms apart", shortest / MS);
}

#[test]
fn the_live_view_s_rate_follows_its_view() {
    // A big view's 30 a second, then the small view's 10 again, from the stream's own 30 fps.
    let mut pacer = PreviewPacer::at(LIVE_VIEW_FPS);
    let frame = |n: u64| 1_000 * MS + n * 1_000 * MS / 30;
    pacer.retime(30);
    assert!(
        (0..30).all(|n| pacer.take(frame(n))),
        "every frame while big"
    );
    pacer.retime(LIVE_VIEW_FPS);
    let small = (30..60).filter(|&n| pacer.take(frame(n))).count();
    assert_eq!(small, 10, "every third once small again");
}
