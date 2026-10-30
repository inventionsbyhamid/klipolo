//! Span lists `[from, to)`, sorted and disjoint, combined: idle stretches are where the cursor is still, the screen is
//! quiet and the sound is quiet, and nobody is typing. Each is one merge pass.

use crate::Us;

/// Sorted, disjoint span lists intersected. O(a + b).
pub fn intersect(a: &[(Us, Us)], b: &[(Us, Us)]) -> Vec<(Us, Us)> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while let (Some(&x), Some(&y)) = (a.get(i), b.get(j)) {
        let (from, to) = (x.0.max(y.0), x.1.min(y.1));
        if from < to {
            out.push((from, to));
        }
        // The one ending first can meet nothing more.
        if x.1 < y.1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// `a` without the parts `b` covers. O(a + b).
pub fn subtract(a: &[(Us, Us)], b: &[(Us, Us)]) -> Vec<(Us, Us)> {
    let mut out = Vec::new();
    let mut j = 0;
    for &(mut from, to) in a {
        // The `b` spans ending before this one starts can't meet any later `a` span either.
        while b.get(j).is_some_and(|y| y.1 <= from) {
            j += 1;
        }
        for &(cut_from, cut_to) in b[j..].iter().take_while(|y| y.0 < to) {
            if cut_from > from {
                out.push((from, cut_from));
            }
            from = from.max(cut_to);
        }
        if from < to {
            out.push((from, to));
        }
    }
    out
}

/// The spans lasting at least `min_us`. O(n).
pub fn at_least(spans: &[(Us, Us)], min_us: Us) -> Vec<(Us, Us)> {
    let long = |s: &&(Us, Us)| s.1.saturating_sub(s.0) >= min_us;
    spans.iter().filter(long).copied().collect()
}

/// Idle: the cursor still, the screen quiet and the sound quiet, for at least `min_us`. Typing is never idle: a keystroke
/// changes a cell or two, so a quiet screen can hold silent typing.
pub fn idle(
    still: &[(Us, Us)],
    quiet_screen: &[(Us, Us)],
    quiet_sound: &[(Us, Us)],
    typing: &[(Us, Us)],
    min_us: Us,
) -> Vec<(Us, Us)> {
    let quiet = intersect(&intersect(still, quiet_screen), quiet_sound);
    at_least(&subtract(&quiet, typing), min_us)
}
