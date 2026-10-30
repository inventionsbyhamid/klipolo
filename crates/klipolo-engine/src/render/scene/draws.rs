//! A frame's path draws: a path-drawn node's fill and stroke as `PathDraw`s, in paint order, from its
//! place, its values this frame and its mesh. Nothing here tessellates: a trim, a dash offset, a colour or a motion only
//! changes the draw's numbers. A stroke's width is in its mesh's units (a `path`'s own unless `strokeScale` is false,
//! else 1080p pixels), drawn at least half a pixel wide (thinner is fainter instead).

use klipolo_model::scene::eval::NodeState;

use super::meshes::{NodeMesh, Units};
use super::pack::Run;
use super::path_pipeline::PathDraw;
use super::place::{World, centred, mul};
use super::statics::NodeStatic;

/// The longest dash pattern as drawn: four lengths written, an odd list repeated (SVG's rule) to six.
pub(super) const MAX_DASH: usize = 8;

/// A node's dashes as drawn: the lengths, an odd list written twice, and how many.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Dash {
    pub lengths: [f32; MAX_DASH],
    pub count: u8,
}

impl Dash {
    /// `written` (1 to 4 lengths, checked), doubled when odd; None for no dash or one of no length.
    pub fn of(written: &[f64]) -> Option<Dash> {
        let n = written.len().min(4);
        let reps = if n % 2 == 1 { 2 } else { 1 };
        let mut d = Dash::default();
        for (i, v) in written[..n].iter().cycle().take(n * reps).enumerate() {
            d.lengths[i] = v.max(0.0) as f32;
        }
        d.count = (n * reps) as u8;
        (d.lengths.iter().sum::<f32>() > 0.0).then_some(d)
    }
}

/// Mesh units per 1080p pixel of a stroke and its dashes; None when they are written in the mesh's own units (a `path`
/// whose stroke scales with it).
fn per_px(units: Units, k: f64) -> Option<f64> {
    match units {
        Units::Path { scale: true, .. } => None,
        Units::Path { fit, .. } | Units::Px { fit } => Some(k / fit.max(1e-12)),
    }
}

/// Node `i`'s fill and stroke drawn as a path, after the shapes before it: their draws and runs.
#[allow(clippy::too_many_arguments)]
pub(super) fn push(
    draws: &mut Vec<PathDraw>,
    runs: &mut Vec<Run>,
    i: usize,
    here: &World,
    s: &NodeState,
    st: &NodeStatic,
    nm: &NodeMesh,
    k: f64,
) {
    let Some(mesh) = nm.mesh.as_ref() else {
        return;
    };
    let f = nm.fit;
    let m = mul(&centred(here), &[f.sx, 0.0, 0.0, f.sy, f.tx, f.ty]);
    let px = (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(1e-9);
    let pixel = 1.0 / px;
    let [c1, c2] = here.clips;
    let base = PathDraw {
        m: [m[0], m[1], m[2], m[3]].map(|v| v as f32),
        t: [m[4] as f32, m[5] as f32, 0.0, 0.0],
        uv: nm.uv,
        box_hw: [nm.uv_px[0], nm.uv_px[1], 0.0, 1.0],
        trim: [0.0, 0.0, mesh.length, 4.0],
        clip_m: c1.m,
        clip_t: c1.t,
        clip: c1.c,
        clip2_m: c2.m,
        clip2_t: c2.t,
        clip2: c2.c,
        ..PathDraw::default()
    };
    let opacity = here.opacity as f32;
    let tinted = |solid: Option<[f32; 4]>, state: [f32; 4]| {
        solid.map_or(state, |c| [0, 1, 2, 3].map(|j| c[j] * state[j]))
    };
    let fill = tinted(st.fill_solid, s.fill);
    if mesh.fill.1 > mesh.fill.0 && fill[3] > 0.0 {
        draws.push(PathDraw {
            t: [base.t[0], base.t[1], (0.5 * pixel) as f32, 0.0],
            color: fill,
            paint: [
                st.fill_gradient as f32,
                s.fill_angle.to_radians() as f32,
                s.fill_offset as f32,
                opacity,
            ],
            ..base
        });
        runs.push(Run::Path {
            node: i as u32,
            draw: draws.len() as u32 - 1,
            stroke: false,
            once: false,
        });
    }
    let stroke = tinted(st.stroke_solid, s.stroke);
    let unit = per_px(nm.units, k).unwrap_or(1.0);
    let hw = s.stroke_width * unit / 2.0;
    if mesh.stroke.1 <= mesh.stroke.0 || stroke[3] <= 0.0 || hw <= 0.0 {
        return;
    }
    let Some(trim) = trim(s.trim, mesh.length, mesh.closed) else {
        return;
    };
    // At least half a pixel wide; thinner draws fainter.
    let thin = (hw * px / 0.5).min(1.0);
    let hw = hw.max(0.5 * pixel);
    // Every trim and dash cut takes the line's cap.
    let cap = f32::from(nm.cap);
    let (mut dash0, mut dash1, mut dash) = ([0.0; 4], [0.0; 4], [0.0, 0.0, 0.0, cap]);
    if let Some(d) = st.dash {
        let l = d.lengths.map(|v| (f64::from(v) * unit) as f32);
        dash0.copy_from_slice(&l[..4]);
        dash1.copy_from_slice(&l[4..]);
        let period: f32 = l[..usize::from(d.count)].iter().sum();
        dash = [
            f32::from(d.count),
            period,
            (s.dash_offset * unit) as f32,
            cap,
        ];
    }
    draws.push(PathDraw {
        t: [base.t[0], base.t[1], (hw + pixel) as f32, 1.0],
        box_hw: [nm.uv_px[0], nm.uv_px[1], hw as f32, thin as f32],
        color: stroke,
        paint: [
            st.stroke_gradient as f32,
            st.stroke_turn,
            st.stroke_slide,
            opacity,
        ],
        trim,
        dash0,
        dash1,
        dash,
        ..base
    });
    // A stroke that isn't opaque blends once a pixel where it overlaps itself (its joins, a crossing), through the
    // stencil; an opaque one costs nothing more.
    let alpha = f64::from(stroke[3]) * f64::from(opacity) * thin * f64::from(st.stroke_stop_alpha);
    runs.push(Run::Path {
        node: i as u32,
        draw: draws.len() as u32 - 1,
        stroke: true,
        once: alpha < 0.998,
    });
}

/// The shader's trim for `[start, end, offset]` (fractions) of a path `length` long: where the part drawn starts and
/// how long it is, and which of its ends are the path's own (an open path's ends are its caps, not cuts); None when
/// nothing of it is drawn.
pub(super) fn trim([start, end, offset]: [f64; 3], length: f32, closed: bool) -> Option<[f32; 4]> {
    let span = end - start;
    if span.is_nan() || span <= 1e-9 {
        return None;
    }
    if span >= 1.0 - 1e-9 {
        return Some([0.0, length, length, 4.0]);
    }
    let snap = |v: f64| {
        let f = v.rem_euclid(1.0);
        if (1e-6..=1.0 - 1e-6).contains(&f) {
            f
        } else {
            0.0
        }
    };
    let (a, b) = (snap(start + offset), snap(end + offset));
    let flags = if closed {
        0
    } else {
        u32::from(a == 0.0) | (u32::from(b == 0.0) << 1)
    };
    let l = f64::from(length);
    Some([(a * l) as f32, (span * l) as f32, length, flags as f32])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An odd dash list repeats (SVG's rule), so `[a, b, c]` dashes a, gaps b, dashes c, gaps a…; an
    /// even one is as written; all zeros is no dash.
    #[test]
    fn an_odd_dash_list_doubles() {
        let d = Dash::of(&[6.0, 2.0, 1.0]).unwrap();
        assert_eq!(d.count, 6);
        assert_eq!(&d.lengths[..6], &[6.0, 2.0, 1.0, 6.0, 2.0, 1.0]);
        let one = Dash::of(&[5.0]).unwrap();
        assert_eq!((one.count, &one.lengths[..2]), (2, &[5.0, 5.0][..]));
        let even = Dash::of(&[4.0, 2.0, 1.0, 3.0]).unwrap();
        assert_eq!(
            (even.count, &even.lengths[..4]),
            (4, &[4.0, 2.0, 1.0, 3.0][..])
        );
        assert!(Dash::of(&[0.0, 0.0]).is_none());
    }

    /// A trim's part drawn: from start to end moved by the offset, wrapping; an open path's ends aren't cuts.
    #[test]
    fn a_trim_cuts_where_it_should() {
        assert_eq!(
            trim([0.0, 1.0, 0.3], 100.0, false),
            Some([0.0, 100.0, 100.0, 4.0])
        );
        assert_eq!(trim([0.5, 0.5, 0.0], 100.0, false), None);
        // Draw-on to half: its start is the path's own (an end), its end a cut.
        assert_eq!(
            trim([0.0, 0.5, 0.0], 100.0, false),
            Some([0.0, 50.0, 100.0, 1.0])
        );
        // On a ring its start is a cut too.
        assert_eq!(
            trim([0.0, 0.5, 0.0], 100.0, true),
            Some([0.0, 50.0, 100.0, 0.0])
        );
        // Moved by 0.75: from 0.75 round to 0.25.
        let [a, l, _, flags] = trim([0.0, 0.5, 0.75], 100.0, false).unwrap();
        assert!((a - 75.0).abs() < 1e-3 && (l - 50.0).abs() < 1e-3 && flags == 0.0);
    }
}
