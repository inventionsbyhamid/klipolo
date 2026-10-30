//! An instance's gradient table (text spans' own included): every
//! gradient a node or a span fills or strokes with, resolved with the node's settings and premultiplied, the same one
//! kept once, at most [`MAX_GRADIENTS`]. Built with the instance, never per frame.

use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::{Gradient, Paint};

use super::shapes::{GpuGradient, MAX_GRADIENTS, MAX_STOPS};

/// `g` (node `i`'s) for the GPU: its stops' colours resolved and premultiplied, at most [`MAX_STOPS`].
pub(super) fn gradient(inst: &SceneInstance, i: usize, g: &Gradient) -> Option<GpuGradient> {
    let (kind, geom) = match (g.linear, g.radial) {
        (Some(l), _) => (1.0, [l[0], l[1], l[2], l[3]]),
        (None, Some(r)) => (2.0, [r[0], r[1], r[2], 0.0]),
        (None, None) => return None,
    };
    let mut out = GpuGradient {
        head: [kind, 0.0, 0.0, 0.0],
        geom: geom.map(|v| if v.is_finite() { v as f32 } else { 0.0 }),
        pos: [[0.0; 4]; 4],
        color: [[0.0; 4]; MAX_STOPS],
    };
    let mut last = 0.0f32;
    for (k, (at, color)) in g.stops.iter().take(MAX_STOPS).enumerate() {
        let c = inst.color_of(i, color).unwrap_or([0.0; 4]);
        let at = if at.is_finite() { *at as f32 } else { 0.0 };
        // In order, as the format asks: a stop before the one ahead of it sits on it.
        last = at.clamp(last, 1.0);
        out.pos[k / 4][k % 4] = last;
        out.color[k] = [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
        out.head[1] = (k + 1) as f32;
    }
    (out.head[1] > 0.0).then_some(out)
}

/// A gradient's most transparent stop's alpha.
pub(super) fn least_alpha(g: &GpuGradient) -> f32 {
    let n = (g.head[1] as usize).min(MAX_STOPS);
    g.color[..n].iter().map(|c| c[3]).fold(1.0, f32::min)
}

/// A gradient's first stop, straight (its stops are premultiplied).
pub(super) fn first_stop(g: &GpuGradient) -> [f32; 4] {
    let [r, gr, b, a] = g.color[0];
    if a > 0.0 {
        [r / a, gr / a, b / a, a]
    } else {
        [0.0; 4]
    }
}

/// `g`'s index in `table`, added when it isn't there; −1 when the table is full (the shape then draws its first
/// stop's colour: the check refuses scenes past it).
pub(super) fn intern(table: &mut Vec<GpuGradient>, g: GpuGradient) -> i32 {
    if let Some(i) = table.iter().position(|t| *t == g) {
        return i as i32;
    }
    if table.len() >= MAX_GRADIENTS {
        return -1;
    }
    table.push(g);
    (table.len() - 1) as i32
}

/// Node `i`'s spans' gradients (a span filled with one draws it across its own letters), one entry per span
/// appended to `spans` (its index in `table`, −1 for a span without one or past the table): where node `i`'s start.
pub(super) fn span_gradients(
    inst: &SceneInstance,
    i: usize,
    list: Option<&[klipolo_model::scene::Span]>,
    table: &mut Vec<GpuGradient>,
    spans: &mut Vec<i32>,
) -> u32 {
    let from = spans.len() as u32;
    for s in list.into_iter().flatten() {
        let k = match &s.fill {
            Some(Paint::Gradient(g)) => gradient(inst, i, g).map_or(-1, |gpu| intern(table, gpu)),
            _ => -1,
        };
        spans.push(k);
    }
    from
}
