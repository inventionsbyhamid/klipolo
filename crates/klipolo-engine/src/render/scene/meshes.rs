//! An instance's path meshes: for each node drawn as a path, its mesh from the shared cache and how its
//! units fit its box. Made when the instance is built (the playback lookahead's `prepare`); after that, each frame
//! only compares what each mesh was made for with what the node needs now, and makes again just the nodes whose inputs
//! crossed a bucket: a path's tolerance (powers of two of its fit), a box-pixel mesh's box and an
//! outline's corners (each exact once it holds still, in steps of 10 % while it moves, the mesh fitted to the box
//! between them), a round end's width (powers of two).
//! A path's data is parsed, hashed and bounded once, when the instance is built. A trim, a dash, a colour, a fade, a
//! motion or a layer that scales makes nothing.

use std::sync::Arc;

use klipolo_model::scene::eval::{NodeState, SceneFrame};
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::path::PathCmd;
use klipolo_model::scene::timeline::Prop;
use klipolo_model::scene::{AspectRatio, FillRule, LineCap, LineJoin, Props};

use super::Drawn;
use super::geometry::Fit;
use super::mesh_cache::{GpuMesh, MeshCache, MeshKey};
use super::node_paths::{Parsed, bucket, cap_of, geometry_of, half_width, made_for, parse};
use super::paths;
use super::place::sizes;
use super::statics::{Look, NodeStatic};
use super::stroke::StrokeStyle;

/// What a stroke's width and dashes are in: a `path`'s own units (scaling with it unless not `scale`), or the box's
/// pixels; `fit` is the mesh's pixels per unit either way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Units {
    Path { scale: bool, fit: f64 },
    Px { fit: f64 },
}

/// What a mesh was made for: equal, it serves (fitted again to the box it is drawn in).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Made {
    /// The box a box-pixel mesh was built in, and an outline's corners and depth (each in steps of 10 %).
    pub size: (f64, f64),
    pub shape: [f64; 5],
    /// Powers of two: the flattening tolerance and the round ends' half width.
    pub tolerance: i8,
    pub width: i8,
}

/// One node's mesh and its fit.
pub(super) struct NodeMesh {
    pub mesh: Option<Arc<GpuMesh>>,
    pub fit: Fit,
    /// Mesh units → its paint's box (scale x, y, offset x, y), and that box in pixels.
    pub uv: [f32; 4],
    pub uv_px: [f32; 2],
    pub units: Units,
    /// The line's cap (0 butt, 1 round, 2 square): its ends' and every cut's.
    pub cap: u8,
    join: LineJoin,
    made: Option<Made>,
    /// A box-pixel mesh's box and corners last frame: unchanged, the mesh is made for them exactly; moving, for their
    /// 10 % steps (so a scaling layer reuses a few meshes and settles on the exact one).
    last: Option<((f64, f64), [f64; 5])>,
    fill: Option<FillRule>,
    strokes: bool,
}

/// An instance's meshes, by node.
#[derive(Default)]
pub(super) struct Meshes {
    nodes: Vec<Option<NodeMesh>>,
    /// Per compiled node (None: not a path, or one that doesn't parse).
    parsed: Vec<Option<Parsed>>,
    /// Every node's box this frame (its room kept).
    sizes: Vec<(f64, f64)>,
    /// A path past the vertex cap or the bound was left out (said once, `frame.sceneTooDetailed`).
    pub too_detailed: bool,
}

/// Where meshes come from: the GPU and the shared cache, and the frame number.
pub(super) struct Source<'a> {
    pub device: &'a wgpu::Device,
    pub cache: &'a mut MeshCache,
    pub now: u64,
}

pub(super) fn props(inst: &SceneInstance, i: usize) -> Option<&Props> {
    Some(&inst.compiled().nodes.get(inst.cnode(i)?)?.node.props)
}

impl Meshes {
    /// Every path-drawn node of `inst` (static parts `statics`) where it is `drawn`, at rest.
    pub(super) fn build(
        src: &mut Source,
        inst: &SceneInstance,
        statics: &[NodeStatic],
        drawn: Drawn,
    ) -> Meshes {
        let parsed = inst
            .compiled()
            .nodes
            .iter()
            .map(|c| parse(&c.node.props))
            .collect();
        let mut m = Meshes {
            nodes: (0..inst.len()).map(|_| None).collect(),
            parsed,
            ..Meshes::default()
        };
        sizes(inst, statics, drawn, &mut m.sizes);
        for i in 0..inst.len() {
            let (Some(st), Some(rest), Some(p)) = (statics.get(i), inst.rest(i), props(inst, i))
            else {
                continue;
            };
            let any = |props: &[Prop]| inst.animates(i, |q| props.contains(&q));
            let fills = rest.fill[3] > 0.0
                || st.fill_gradient >= 0
                || any(&[Prop::FillR, Prop::FillG, Prop::FillB, Prop::FillA]);
            let strokes = (rest.stroke[3] > 0.0
                || st.stroke_gradient >= 0
                || any(&[Prop::StrokeR, Prop::StrokeG, Prop::StrokeB, Prop::StrokeA]))
                && (rest.stroke_width > 0.0 || any(&[Prop::StrokeWidth]));
            let fill = (st.look == Look::Path && fills).then(|| p.fill_rule.unwrap_or_default());
            let wanted = matches!(st.look, Look::Path | Look::Polyline) || st.outline;
            if !wanted || (fill.is_none() && !strokes) {
                continue;
            }
            let cap = match st.look {
                Look::Line { cap } => cap as u8,
                _ => p.cap.unwrap_or(LineCap::Butt) as u8,
            };
            let mut nm = NodeMesh {
                mesh: None,
                fit: Fit::SAME,
                uv: [0.0; 4],
                uv_px: [0.0; 2],
                units: Units::Px { fit: 1.0 },
                cap,
                join: p.join.unwrap_or(LineJoin::Miter),
                made: None,
                last: None,
                fill,
                strokes,
            };
            if m.update(src, inst, i, st, rest, drawn.k, &mut nm) {
                m.nodes[i] = Some(nm);
            }
        }
        m
    }

    pub(super) fn get(&self, i: usize) -> Option<&NodeMesh> {
        self.nodes.get(i)?.as_ref()
    }

    /// Nodes drawn as paths.
    pub(super) fn count(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_some()).count()
    }

    /// The CPU bytes they take (the meshes themselves are the cache's; a path's data once per compiled node).
    pub(super) fn bytes(&self) -> usize {
        let parsed: usize = self.parsed.iter().flatten().map(|p| p.cmds.len()).sum();
        self.nodes.capacity() * std::mem::size_of::<Option<NodeMesh>>()
            + self.sizes.capacity() * 16
            + parsed * std::mem::size_of::<PathCmd>()
    }

    /// This frame's boxes and values against what each mesh was made for: fitted again, or made again past a bucket.
    /// Allocates nothing when nothing crossed one.
    pub(super) fn refresh(
        &mut self,
        src: &mut Source,
        inst: &SceneInstance,
        frame: &SceneFrame,
        statics: &[NodeStatic],
        drawn: Drawn,
    ) {
        let mut now = std::mem::take(&mut self.sizes);
        sizes(inst, statics, drawn, &mut now);
        self.sizes = now;
        for i in 0..self.nodes.len() {
            let Some(mut nm) = self.nodes[i].take() else {
                continue;
            };
            if let (Some(st), Some(s)) = (statics.get(i), frame.nodes().get(i)) {
                self.update(src, inst, i, st, s, drawn.k, &mut nm);
            }
            self.nodes[i] = Some(nm);
        }
    }

    /// Node `i`'s fit for its box and values `s`, its mesh made again when they crossed a bucket. False when it draws
    /// no path.
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        src: &mut Source,
        inst: &SceneInstance,
        i: usize,
        st: &NodeStatic,
        s: &NodeState,
        k: f64,
        nm: &mut NodeMesh,
    ) -> bool {
        let (Some(&(w, h)), Some(p)) = (self.sizes.get(i), props(inst, i)) else {
            return false;
        };
        let parsed = inst
            .cnode(i)
            .and_then(|c| self.parsed.get(c))
            .and_then(Option::as_ref);
        let mut made = Made {
            size: (0.0, 0.0),
            shape: [0.0; 5],
            tolerance: 0,
            width: 0,
        };
        if st.look == Look::Path {
            let Some(parsed) = parsed else { return false };
            let meet = p.preserve_aspect_ratio != Some(AspectRatio::Stretch);
            nm.fit = Fit::of(parsed.view_box, meet, w, h);
            let [vx, vy, vw, vh] = parsed.view_box;
            let (vw, vh) = (vw.max(1e-9), vh.max(1e-9));
            nm.uv = [1.0 / vw, 1.0 / vh, -vx / vw, -vy / vh].map(|v| v as f32);
            nm.uv_px = [vw * nm.fit.sx.abs(), vh * nm.fit.sy.abs()].map(|v| v as f32);
            let scale = p.stroke_scale != Some(false);
            let fit = nm.fit.scale();
            nm.units = Units::Path { scale, fit };
        } else {
            let r = s.radius.map(|r| r * k);
            let shape = if st.outline {
                [r[0], r[1], r[2], r[3], s.inner_radius]
            } else {
                [0.0; 5]
            };
            let ((qw, qh), corners) = made_for(nm.last, (w, h), shape);
            nm.last = Some(((w, h), shape));
            let ratio = |v: f64, q: f64| if q > 0.0 { v / q } else { 1.0 };
            nm.fit = Fit {
                sx: ratio(w, qw),
                sy: ratio(h, qh),
                tx: 0.0,
                ty: 0.0,
            };
            nm.uv = [1.0 / qw.max(1e-9), 1.0 / qh.max(1e-9), 0.5, 0.5].map(|v| v as f32);
            nm.uv_px = [w as f32, h as f32];
            nm.units = Units::Px {
                fit: nm.fit.scale(),
            };
            made.size = (qw, qh);
            made.shape = corners;
        }
        // Flattened to a quarter pixel at its drawn size (its motion's scale aside), bucketed by powers of two.
        let px_per_unit = nm.fit.sx.abs().max(nm.fit.sy.abs()).max(1e-12);
        made.tolerance = bucket(0.25 / px_per_unit, f64::floor);
        let tolerance = f64::from(made.tolerance).exp2();
        let round = nm.cap == 1 || nm.join == LineJoin::Round;
        if round {
            made.width = bucket(half_width(s, nm.units, k).max(2.0 * tolerance), f64::ceil);
        }
        if nm.made == Some(made) {
            return true;
        }
        nm.made = Some(made);
        let Some((cmds, hash)) = geometry_of(inst, i, st, parsed, &made) else {
            return false;
        };
        let round_hw = if round {
            f64::from(made.width).exp2()
        } else {
            tolerance
        };
        let style = nm.strokes.then_some(StrokeStyle {
            cap: cap_of(nm.cap),
            join: nm.join,
            round_hw: round_hw.max(tolerance),
            tolerance,
        });
        let key = MeshKey {
            geometry: hash,
            fill: nm.fill.map(|r| r as u8),
            stroke: style.map(|s| (s.cap as u8, s.join as u8, made.width)),
            tolerance: made.tolerance,
        };
        let fill = nm.fill;
        let mesh = src.cache.get(src.device, key, src.now, || {
            paths::tessellate(&cmds, fill, style, tolerance)
        });
        self.too_detailed |= mesh.is_err();
        nm.mesh = mesh.ok();
        true
    }
}
