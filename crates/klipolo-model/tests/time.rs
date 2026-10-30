use klipolo_model::time::*;

#[test]
fn seconds_round_trip() {
    assert_eq!(secs_to_us(1.5), 1_500_000);
    assert_eq!(secs_to_us(0.000_000_4), 0);
    assert!((us_to_secs(2_250_000) - 2.25).abs() < 1e-12);
}

#[test]
fn snaps_to_nearest_frame() {
    assert_eq!(snap_to_frame(1_016_000, 30), 1_000_000);
    assert_eq!(snap_to_frame(1_020_000, 30), 1_033_333);
    assert_eq!(snap_to_frame(500_000, 60), 500_000);
}

#[test]
fn speed_scaling() {
    assert_eq!(scaled(10_000_000, 2.0), 5_000_000);
    assert_eq!(unscaled(5_000_000, 2.0), 10_000_000);
    assert_eq!(scaled(1_000_000, 0.5), 2_000_000);
}

#[test]
fn merges_clamps_and_sorts_ranges() {
    let r = |s, e| RangeUs { start: s, end: e };
    let merged = merge_ranges(&[r(8, 12), r(-5, 2), r(1, 3), r(50, 40), r(11, 99)], 20);
    assert_eq!(merged, vec![r(0, 3), r(8, 20)]);
}

#[test]
fn negative_limit_merges_to_nothing() {
    assert_eq!(merge_ranges(&[RangeUs { start: 0, end: 5 }], -1), vec![]);
}

#[test]
fn ripple_map_closes_gaps() {
    let ranges = [
        RangeUs { start: 10, end: 20 },
        RangeUs { start: 30, end: 35 },
    ];
    assert_eq!(ripple_map(&ranges, 5), 5);
    assert_eq!(ripple_map(&ranges, 15), 10);
    assert_eq!(ripple_map(&ranges, 20), 10);
    assert_eq!(ripple_map(&ranges, 25), 15);
    assert_eq!(ripple_map(&ranges, 33), 20);
    assert_eq!(ripple_map(&ranges, 40), 25);
}

#[test]
fn extreme_ranges_saturate() {
    let r = |s, e| RangeUs { start: s, end: e };
    assert_eq!(r(i64::MIN, i64::MAX).len(), i64::MAX);
    assert_eq!(r(i64::MAX, i64::MIN).len(), i64::MIN);
    assert_eq!(ripple_map(&[r(i64::MIN, 0), r(1, i64::MAX)], i64::MAX), 0);
    assert_eq!(ripple_map(&[r(0, 10), r(i64::MIN, i64::MAX)], 20), i64::MIN);
    assert_eq!(ripple_map(&[r(10, 0)], i64::MAX), i64::MAX);
}

#[test]
fn kept_parts_excludes_ranges() {
    let ranges = [
        RangeUs { start: 10, end: 20 },
        RangeUs { start: 30, end: 35 },
    ];
    let r = |s, e| RangeUs { start: s, end: e };
    assert_eq!(
        kept_parts(0, 40, &ranges),
        vec![r(0, 10), r(20, 30), r(35, 40)]
    );
    assert_eq!(kept_parts(12, 18, &ranges), vec![]);
    assert_eq!(kept_parts(15, 32, &ranges), vec![r(20, 30)]);
}

#[test]
fn ripple_agrees_with_the_one_range_helpers() {
    let ranges: Vec<RangeUs> = (0..200)
        .map(|i| RangeUs {
            start: i * 1_000_000 + 300_000,
            end: i * 1_000_000 + 700_000,
        })
        .collect();
    let merged = merge_ranges(&ranges, 300_000_000);
    let ripple = Ripple::new(&merged);
    assert_eq!(ripple.ranges(), merged.as_slice());
    for t in (0..201_000_000).step_by(37_337) {
        assert_eq!(ripple.map(t), ripple_map(&merged, t), "map {t}");
        let kept = merged
            .iter()
            .find(|r| r.start <= t && t < r.end)
            .map_or(t, |r| r.end);
        assert_eq!(ripple.first_kept(t), kept, "first kept {t}");
    }
    for (a, b) in [
        (0, 5_000_000),
        (1_350_000, 1_650_000),
        (12_500_000, 99_100_000),
        (199_800_000, 200_000_000),
    ] {
        assert_eq!(
            ripple.kept(a, b),
            kept_parts(a, b, &merged),
            "kept {a}..{b}"
        );
    }
}
