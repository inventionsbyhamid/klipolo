//! The one CSS face matcher: the engine, the export and (over IPC) the webview all take this answer.

/// CSS font matching over `(weight, italic)` candidates (CSS Fonts 4 §5.2): the style first (italic, else upright), then
/// for a wanted weight in 400–500 the nearest heavier up to 500, then lighter, then heavier; below 400 lighter then
/// heavier; above 500 heavier then lighter. None only when there are no candidates.
pub fn nearest(candidates: &[(u16, bool)], weight: u16, italic: bool) -> Option<usize> {
    let styled: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].1 == italic)
        .collect();
    let pool: Vec<usize> = if styled.is_empty() {
        (0..candidates.len()).collect()
    } else {
        styled
    };
    let w = |i: usize| candidates[i].0;
    if let Some(&i) = pool.iter().find(|&&i| w(i) == weight) {
        return Some(i);
    }
    let lighter = |p: &[usize]| {
        p.iter()
            .copied()
            .filter(|&i| w(i) < weight)
            .max_by_key(|&i| w(i))
    };
    let heavier = |p: &[usize]| {
        p.iter()
            .copied()
            .filter(|&i| w(i) > weight)
            .min_by_key(|&i| w(i))
    };
    if (400..=500).contains(&weight) {
        if let Some(i) = pool
            .iter()
            .copied()
            .filter(|&i| w(i) > weight && w(i) <= 500)
            .min_by_key(|&i| w(i))
        {
            return Some(i);
        }
        return lighter(&pool).or_else(|| heavier(&pool));
    }
    if weight < 400 {
        lighter(&pool).or_else(|| heavier(&pool))
    } else {
        heavier(&pool).or_else(|| lighter(&pool))
    }
}
