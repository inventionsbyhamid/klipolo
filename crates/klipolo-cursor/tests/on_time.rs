//! A thinned track reads like the dense one: the recorder drops moves a straight line
//! reproduces, so what reads the path (still-run edges, dwells, the Auto zoom's window mean) works on time, not on how
//! many samples were kept.

mod common;

use common::{file, head, read};
use klipolo_cursor::facts::{StillRuns, dwells};
use klipolo_cursor::format::Line;
use klipolo_cursor::{CursorTrack, Us};

const S: Us = 1_000_000;

/// The path's corners: at rest at A to 2 s, a slow drift to B (2 % of the width over a second), at rest to 5 s, a
/// quick move to C over 0.2 s, at rest to 8 s.
const CORNERS: [(Us, f32, f32); 6] = [
    (0, 0.3, 0.4),
    (2 * S, 0.3, 0.4),
    (3 * S, 0.32, 0.4),
    (5 * S, 0.32, 0.4),
    (5_200_000, 0.7, 0.6),
    (8 * S, 0.7, 0.6),
];

fn place(t: Us) -> (f32, f32) {
    let i = CORNERS
        .partition_point(|c| c.0 <= t)
        .clamp(1, CORNERS.len() - 1);
    let ((t0, x0, y0), (t1, x1, y1)) = (CORNERS[i - 1], CORNERS[i]);
    let f = ((t - t0) as f32 / (t1 - t0) as f32).clamp(0.0, 1.0);
    (x0 + (x1 - x0) * f, y0 + (y1 - y0) * f)
}

/// Every 4 ms, as the recorder samples (`dense`), or only the corners, as its thinning keeps a piecewise straight path.
fn track(dense: bool) -> CursorTrack {
    let times: Vec<Us> = if dense {
        (0..=8 * S).step_by(4_000).collect()
    } else {
        CORNERS.iter().map(|c| c.0).collect()
    };
    let mut lines = vec![head()];
    lines.extend(times.into_iter().map(|t| {
        let (x, y) = place(t);
        Line::Move { t, x, y }
    }));
    read(&file(&lines))
}

#[test]
fn still_runs_and_dwells_end_where_the_path_leaves_them_however_few_samples_were_kept() {
    let (dense, thin) = (track(true), track(false));
    let (a, b) = (StillRuns::of(&dense), StillRuns::of(&thin));
    let (ra, rb) = (a.runs(), b.runs());
    assert_eq!(ra.len(), 3, "{ra:?}");
    assert_eq!(rb.len(), ra.len(), "{rb:?}");
    // A run starts at its first sample, wherever the dense track's anchor last hopped while it arrived: up to the time
    // the path takes to cross the still zone (0.118 s on the drift). Its end is where the path leaves: the same line.
    for (x, y) in ra.iter().zip(rb) {
        assert!(
            (x.0 - y.0).abs() <= 120_000 && (x.1.min(9 * S) - y.1.min(9 * S)).abs() <= 2_000,
            "dense {ra:?}, thinned {rb:?}"
        );
    }
    // The drift leaves A once it has moved 0.2 % of the diagonal (1440 × 900 points): 2.36e-3 of the width, 0.118 s
    // into it. Not at 2 s, the last sample the thinned track kept inside.
    let leaves = 2 * S + 118_000;
    assert!((ra[0].1 - leaves).abs() < 2_000, "{ra:?}");
    assert!((rb[0].1 - leaves).abs() < 2_000, "{rb:?}");
    let (da, db) = (dwells(&dense, &a), dwells(&thin, &b));
    assert_eq!(da.len(), db.len());
    for (x, y) in da.iter().zip(&db) {
        assert!(
            (x.from - y.from).abs() <= 4_000 && (x.to - y.to).abs() <= 2_000,
            "{da:?} {db:?}"
        );
    }
}

#[test]
fn the_window_mean_is_by_time_so_a_thinned_glide_reads_as_the_dense_one() {
    let (dense, thin) = (track(true), track(false));
    // In the drift (no kept sample of the thinned track inside the window), across the quick move, and long after.
    for t in [2_500_000, 3_100_000, 5_150_000, 5_400_000, 20 * S] {
        let (a, b) = (dense.smoothed(t, 500_000), thin.smoothed(t, 500_000));
        let ((ax, ay), (bx, by)) = (a.unwrap(), b.unwrap());
        assert!(
            (ax - bx).abs() < 1e-4 && (ay - by).abs() < 1e-4,
            "{t}: {a:?} {b:?}"
        );
    }
    // The drift's mean over 2.0–2.5 s is its place at 2.25 s.
    let (x, _) = thin.smoothed(2_500_000, 500_000).unwrap();
    assert!((x - place(2_250_000).0).abs() < 1e-4, "{x}");
    assert_eq!(thin.smoothed(-1, 500_000), None, "before the first sample");
}
