use klipolo_capture::drift::{Anchor, fit};

/// One anchor a second for `seconds`: a device `ppm` fast, frame 0 at `offset_us`, the host stamps jittered by up to
/// ±2 ms (a pseudo-random but fixed pattern).
fn anchors(seconds: u64, ppm: f64, offset_us: i64) -> Vec<Anchor> {
    (0..seconds)
        .map(|s| {
            let jitter = ((s * 7_919) % 4_001) as i64 - 2_000;
            let session_us = offset_us + (s as i64) * 1_000_000 + jitter;
            let frame = ((s as f64) * 48_000.0 * (1.0 + ppm * 1e-6)).round() as u64;
            Anchor { session_us, frame }
        })
        .collect()
}

#[test]
fn an_hour_of_a_100_ppm_fast_mic_fits_within_half_a_ppm_and_a_millisecond() {
    let f = fit(&anchors(3_600, 100.0, 40_000), 48_000).unwrap();
    assert!((f.drift_ppm - 100.0).abs() < 0.5, "{f:?}");
    assert!((f.offset_us - 40_000).abs() < 1_000, "{f:?}");
}

#[test]
fn after_an_hour_the_corrected_position_is_within_ten_ms() {
    let f = fit(&anchors(3_600, -80.0, 0), 48_000).unwrap();
    let true_frame: f64 = 3_600.0 * 48_000.0 * (1.0 - 80e-6);
    let predicted =
        (3_600_000_000.0 - f.offset_us as f64) * 1e-6 * 48_000.0 * (1.0 + f.drift_ppm * 1e-6);
    assert!(
        ((predicted - true_frame) / 48.0).abs() < 10.0,
        "ms off: {}",
        (predicted - true_frame) / 48.0
    );
}

#[test]
fn a_late_callback_is_rejected_not_fitted() {
    let mut a = anchors(600, 0.0, 0);
    a[300].session_us += 300_000; // a callback 300 ms late
    let f = fit(&a, 48_000).unwrap();
    assert!(f.drift_ppm.abs() < 0.5 && f.anchors_used == 599, "{f:?}");
}

#[test]
fn two_late_callbacks_at_the_end_of_a_short_run_are_both_rejected() {
    // A 15 s run (say, a mic a heal swapped in) whose last two callbacks came 500 ms late. The first line leans toward
    // them, so one rejection pass keeps one, which puts the run's end 154 ms off; a second pass from the refitted line
    // drops both.
    let mut a = anchors(15, 0.0, 0);
    a[13].session_us += 500_000;
    a[14].session_us += 500_000;
    let f = fit(&a, 48_000).unwrap();
    let predicted =
        (14_000_000.0 - f.offset_us as f64) * 1e-6 * 48_000.0 * (1.0 + f.drift_ppm * 1e-6);
    let end_ms_off = (predicted - 14.0 * 48_000.0) / 48.0;
    assert!(
        f.anchors_used == 13 && end_ms_off.abs() < 3.0,
        "{end_ms_off} ms off: {f:?}"
    );
}

#[test]
fn too_few_anchors_fit_nothing() {
    assert!(fit(&anchors(5, 0.0, 0), 48_000).is_none());
}
