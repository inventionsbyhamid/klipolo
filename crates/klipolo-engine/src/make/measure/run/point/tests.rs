use super::{decide, over, over_count_for_p95};
use crate::make::budget::{Budget, CostClass, OVER_AT_PREVIEW_MS};
use crate::make::envelope::Params;
use crate::make::measure::{Decided, Pcts};

/// A point that may run at half size, over Normal at full size, whose half-size frames
/// stopped at a frame over `OVER_AT_PREVIEW_MS` (none came back, and the time didn't cut them) is decided at half
/// size, at that stop, never by its full-size time; a `resolution: half` generator's figures are half-size ones,
/// also where a step or the 24 ms stop ended it.
#[test]
fn a_half_size_stop_is_decided_at_half_size() {
    let b = Budget::DEFAULT;
    let decided = |full: f64, half: Option<f64>, may_halve, always| {
        decide(
            Pcts::flat(full),
            half.map(Pcts::flat),
            may_halve,
            always,
            &b,
        )
    };
    assert_eq!(
        decided(2.3, None, true, false),
        (CostClass::Over, Decided::Half(OVER_AT_PREVIEW_MS))
    );
    assert_eq!(
        decided(2.3, Some(1.0), true, false),
        (CostClass::Heavy, Decided::Half(1.0))
    );
    assert_eq!(
        decided(1.5, None, true, false),
        (CostClass::Normal, Decided::Preview(1.5))
    );
    assert_eq!(
        decided(4.0, Some(4.0), true, true),
        (CostClass::Heavy, Decided::Half(4.0))
    );
    assert_eq!(
        decided(3.0, None, false, false),
        (CostClass::Over, Decided::Preview(3.0))
    );
    let stopped = over(Params::new(), OVER_AT_PREVIEW_MS, true);
    assert_eq!(stopped.decided, Decided::Half(OVER_AT_PREVIEW_MS));
    assert_eq!(
        over(Params::new(), 30.0, false).decided,
        Decided::Preview(30.0)
    );
    assert_eq!(
        over(Params::new(), f64::INFINITY, true).decided,
        Decided::Unmeasured
    );
}

/// That many samples over a limit put the p95 over it, one fewer doesn't, for every frame count a point takes.
#[test]
fn the_samples_that_prove_a_p95_over() {
    for n in 1..=60 {
        let k = over_count_for_p95(n);
        let with = |over: usize| -> Vec<f64> {
            (0..n).map(|i| if i < over { 10.0 } else { 0.0 }).collect()
        };
        assert!(Pcts::of(&with(k)).p95 > 5.0, "{n}: {k}");
        assert!(Pcts::of(&with(k - 1)).p95 < 5.0, "{n}: {k} − 1");
    }
    assert_eq!(over_count_for_p95(30), 2);
}
