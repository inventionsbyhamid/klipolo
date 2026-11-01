//! `text_blur.rs`'s tests.

use super::*;

/// A blur of any deviation keeps three to six of its texels to the deviation, a power of two atlas
/// texels apart, so its across and down passes fetch at most 17 times a texel (the brute-force blur took up to
/// 129 taps, each with an `exp()`); its region's margin is whole steps, at least three deviations and a texel.
#[test]
fn a_blurs_passes_take_a_few_fetches_at_any_deviation() {
    let mut sigma = 0.06f32;
    while sigma <= MAX_SIGMA {
        let k = Kernel::of(sigma);
        assert_eq!(k.step, k.step.round(), "{sigma}: {k:?}");
        assert!((k.step as u32).is_power_of_two(), "{sigma}: {k:?}");
        if sigma >= PER_SIGMA {
            assert!(k.sigma >= 3.0 && k.sigma < 6.0, "{sigma}: {k:?}");
        } else {
            assert_eq!(k.step, 1.0, "{sigma}");
        }
        assert!(k.fetches() <= MAX_FETCHES, "{sigma}: {k:?}");
        assert_eq!(k.margin % k.step, 0.0, "{sigma}: {k:?}");
        assert!(k.margin >= 3.0 * sigma + 1.0, "{sigma}: {k:?}");
        assert!(k.margin < 3.0 * sigma + 2.0 + k.step, "{sigma}: {k:?}");
        let (w1, w2) = (k.ratio, k.ratio * k.ratio * k.fall);
        let exact = |t: f32| (-t * t / (2.0 * k.sigma * k.sigma)).exp();
        assert!((w1 - exact(1.0)).abs() < 1e-5 && (w2 - exact(2.0)).abs() < 1e-5);
        sigma *= 1.07;
    }
    assert_eq!(Kernel::of(24.0).step, 8.0);
    assert_eq!(Kernel::of(48.0).step, 16.0);
    assert_eq!(Kernel::of(5.9).step, 1.0);
}

/// A blur-in title's letter at 1080p's peak (a letter's 100 × 150 cell, a deviation of 24 texels): the brute-force blur
/// fetched 121 times a texel of a full-size region across and as often a fragment down, about 18 million a letter;
/// drawn small its three passes take about 30 thousand, and the text pass reads each fragment once.
#[test]
fn a_letter_at_its_blurs_peak_takes_a_few_thousand_fetches() {
    let budget = BlurBudget::default();
    let atlas = (1000, 1000);
    let uv = [0.1, 0.1, 0.2, 0.25];
    budget
        .place(uv, [0.0, 0.0, 100.0, 150.0], 24.0, atlas)
        .unwrap();
    let ours = budget.fetches() as f64;
    let region = (100.0 + 2.0 * 73.0) * (150.0 + 2.0 * 73.0);
    let brute = 2.0 * region * 121.0;
    assert!(ours < brute / 300.0, "{ours} fetches against {brute}");
    assert_eq!(budget.rows(), 64, "its region is 33 × 39 texels");
}

/// A piece wider than the scratch targets at its blur's step is drawn at a coarser one, so a long line
/// blurs to the end of its blur-in rather than snapping sharp; a deviation far under a texel takes no taps (its weights
/// would underflow to nothing).
#[test]
fn a_wide_piece_blurs_to_the_end_of_its_blur_in() {
    let atlas = (4096, 256);
    let uv = [0.0, 0.0, 3000.0 / 4096.0, 100.0 / 256.0];
    for sigma in [8.0f32, 2.0, 0.5, 0.06] {
        let budget = BlurBudget::default();
        let placed = budget.place(uv, [0.0, 0.0, 3000.0, 100.0], sigma, atlas);
        let b = placed.unwrap_or_else(|| panic!("σ {sigma} drew sharp"));
        assert!(b.step >= 2.0, "σ {sigma}: {b:?}");
        assert!(b.b[2] - b.b[0] <= BLUR_WIDTH as f32, "σ {sigma}: {b:?}");
        assert!(!budget.over());
    }
    assert_eq!(Kernel::at_least(0.5, 4.0).taps, 0);
    assert_eq!(Kernel::of(0.06).taps, 0);
    assert!(Kernel::of(0.5).taps > 0);
}
