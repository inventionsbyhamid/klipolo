//! `fit_shared`: one slope through several files of one device, each file with its own intercept.

use klipolo_model::fit::{fit_shared, Anchor};

const RATE: u32 = 48_000;

/// A file of a device whose clock runs `ppm` fast, its frame 0 at `offset_s` on the session clock: an anchor every
/// 2 s over `seconds`, each up to ±0.1 ms off (a fixed pseudo-random pattern per `seed`).
fn file(offset_s: f64, seconds: f64, ppm: f64, seed: u64) -> Vec<Anchor> {
    (0..(seconds / 2.0) as u64)
        .map(|k| {
            let t = k as f64 * 2.0;
            let mixed = (k + seed * 1_000).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
            let jitter_s = (mixed % 2_001) as f64 / 1_000.0 * 1e-4 - 1e-4;
            Anchor {
                session_us: ((offset_s + t) * 1e6).round() as i64,
                frame: ((t * (1.0 + ppm * 1e-6) + jitter_s) * f64::from(RATE)).round() as u64,
            }
        })
        .collect()
}

#[test]
fn one_slope_for_three_files_with_their_own_offsets() {
    // Three 4-minute files of one 120 ppm recorder, started at 0, 300 and 600 s of the session.
    let files = [
        file(0.0, 240.0, 120.0, 1),
        file(300.0, 240.0, 120.0, 2),
        file(600.0, 240.0, 120.0, 3),
    ];
    let groups: Vec<&[Anchor]> = files.iter().map(Vec::as_slice).collect();
    let shared = fit_shared(&groups, RATE).expect("enough anchors");
    assert!((shared.drift_ppm - 120.0).abs() <= 0.5, "{shared:?}");
    for (offset, truth) in shared.offsets_us.iter().zip([0, 300_000_000, 600_000_000]) {
        assert!((offset - truth).abs() <= 1_000, "{offset} vs {truth}");
    }
    assert_eq!(shared.anchors_used, 360);
    assert!(shared.max_residual_us <= 150, "{shared:?}");
    // Under MIN_ANCHORS in all: nothing to fit.
    let few: Vec<&[Anchor]> = files.iter().map(|f| &f[..3]).collect();
    assert_eq!(fit_shared(&few, RATE), None);
}

#[test]
fn fit_shared_rejects_a_late_anchor_in_one_group() {
    // The second file's 40th anchor came 50 ms late (a bad window): it is dropped, and nothing else moves.
    let mut files = [
        file(0.0, 240.0, -80.0, 4),
        file(300.0, 240.0, -80.0, 5),
        file(600.0, 240.0, -80.0, 6),
    ];
    files[1][40].frame += RATE as u64 / 20;
    let groups: Vec<&[Anchor]> = files.iter().map(Vec::as_slice).collect();
    let shared = fit_shared(&groups, RATE).expect("enough anchors");
    assert_eq!(shared.anchors_used, 359, "{shared:?}");
    assert!((shared.drift_ppm + 80.0).abs() <= 0.5, "{shared:?}");
    for (offset, truth) in shared.offsets_us.iter().zip([0, 300_000_000, 600_000_000]) {
        assert!((offset - truth).abs() <= 1_000, "{offset} vs {truth}");
    }
    assert!(shared.max_residual_us <= 150, "{shared:?}");
}
