//! A path's mesh, made once on the CPU: its commands flattened at the tolerance its drawn size needs
//! (Wang's bound, so every chord is within it), its fill tessellated by lyon's `FillTessellator` with an antialiased
//! fringe round its edge (`fringe.rs`), and its stroke from the centre line (`stroke.rs`). Vertices carry no width: the
//! shader moves them out by the draw's, so a trim, a dash, a colour or a width that grows doesn't make the mesh again.
//! At most [`MAX_MESH_VERTICES`] vertices: past it, nothing (the node isn't drawn).

use klipolo_model::scene::FillRule;
use klipolo_model::scene::path::PathCmd;
use lyon_tessellation::math::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule as LyonRule, FillTessellator, FillVertex, VertexBuffers,
};

use super::fringe;
use super::mesh_cache::{MAX_MESH_VERTICES, MeshData};
use super::path_pipeline::PathVertex;
use super::stroke::{self, StrokeStyle};

/// One flattened sub-path, in mesh units: its points (no two the same in a row), whether `Z` closed it, and whether
/// anything was drawn from its start (a lone `M` draws nothing; `M Z` is a dot).
pub(super) struct SubPath {
    pub points: Vec<[f64; 2]>,
    pub closed: bool,
    drawn: bool,
}

impl SubPath {
    fn at(p: [f64; 2]) -> Self {
        Self {
            points: vec![p],
            closed: false,
            drawn: false,
        }
    }

    fn to(&mut self, p: [f64; 2]) {
        self.drawn = true;
        if self.points.last() != Some(&p) {
            self.points.push(p);
        }
    }
}

/// The most chords one curve is cut into.
const MAX_CHORDS: usize = 1024;

/// `cmds` as sub-paths of points, every chord within `tolerance` of its curve; None past `budget` points.
pub(super) fn flatten(cmds: &[PathCmd], tolerance: f64, budget: usize) -> Option<Vec<SubPath>> {
    let tol = tolerance.max(1e-9);
    let mut out: Vec<SubPath> = Vec::new();
    let mut count = 0usize;
    let (mut at, mut start) = ([0.0, 0.0], [0.0, 0.0]);
    for &cmd in cmds {
        let mut chords_to = |out: &mut Vec<SubPath>, n: usize, f: &dyn Fn(f64) -> [f64; 2]| {
            to(out, &mut count, budget, n, f)
        };
        match cmd {
            PathCmd::Move { x, y } => {
                (at, start) = ([x, y], [x, y]);
                out.push(SubPath::at(at));
            }
            PathCmd::Line { x, y } => {
                let end = [x, y];
                chords_to(&mut out, 1, &|_| end)?;
                at = end;
            }
            PathCmd::Quad { x1, y1, x, y } => {
                let (p0, c, end) = (at, [x1, y1], [x, y]);
                let n = chords(0.25 * len(sub(add(p0, end), scale(c, 2.0))), tol);
                chords_to(&mut out, n, &|t| {
                    let u = 1.0 - t;
                    add(
                        add(scale(p0, u * u), scale(c, 2.0 * u * t)),
                        scale(end, t * t),
                    )
                })?;
                at = end;
            }
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (p0, c1, c2, end) = (at, [x1, y1], [x2, y2], [x, y]);
                let bend = len(sub(add(p0, c2), scale(c1, 2.0)))
                    .max(len(sub(add(c1, end), scale(c2, 2.0))));
                let n = chords(0.75 * bend, tol);
                chords_to(&mut out, n, &|t| {
                    let u = 1.0 - t;
                    add(
                        add(scale(p0, u * u * u), scale(c1, 3.0 * u * u * t)),
                        add(scale(c2, 3.0 * u * t * t), scale(end, t * t * t)),
                    )
                })?;
                at = end;
            }
            PathCmd::Close => {
                if let Some(last) = out.last_mut() {
                    // The closing chord back to the start is the stroke's and the fill's own.
                    if last.points.len() > 1 && last.points.last() == Some(&start) {
                        last.points.pop();
                    }
                    last.closed = true;
                    last.drawn = true;
                }
                // What follows a `Z` without an `M` starts where it closed.
                at = start;
                out.push(SubPath::at(start));
            }
        }
    }
    out.retain(|s| s.drawn);
    Some(out)
}

/// `n` chords of a curve `f` (0–1) from the last sub-path's end, counted against `budget`.
fn to(
    out: &mut [SubPath],
    count: &mut usize,
    budget: usize,
    n: usize,
    f: &dyn Fn(f64) -> [f64; 2],
) -> Option<()> {
    *count += n;
    if *count > budget {
        return None;
    }
    let last = out.last_mut()?;
    for i in 1..=n {
        last.to(f(i as f64 / n as f64));
    }
    Some(())
}

/// Wang's bound: chords for a curve whose second differences reach `m`, each within `tol`.
fn chords(m: f64, tol: f64) -> usize {
    let n = (m / tol).sqrt().ceil();
    if n.is_finite() {
        (n as usize).clamp(1, MAX_CHORDS)
    } else {
        1
    }
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(a: [f64; 2], s: f64) -> [f64; 2] {
    [a[0] * s, a[1] * s]
}

fn len(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

/// A mesh for `cmds`: its fill (by `rule`) with its fringe when asked, then its stroke in `style` when asked. None when
/// it would be past [`MAX_MESH_VERTICES`] or draws nothing.
pub(super) fn tessellate(
    cmds: &[PathCmd],
    rule: Option<FillRule>,
    style: Option<StrokeStyle>,
    tolerance: f64,
) -> Option<MeshData> {
    let subpaths = flatten(cmds, tolerance, MAX_MESH_VERTICES)?;
    let mut mesh = MeshData {
        closed: !subpaths.is_empty() && subpaths.iter().all(|s| s.closed),
        ..MeshData::default()
    };
    if let Some(rule) = rule {
        fill(&subpaths, rule, tolerance, &mut mesh)?;
    }
    mesh.fill = (0, mesh.indices.len() as u32);
    if let Some(style) = style {
        mesh.length = stroke::stroke(&subpaths, style, &mut mesh)?;
    }
    mesh.stroke = (mesh.fill.1, mesh.indices.len() as u32);
    (mesh.vertices.len() <= MAX_MESH_VERTICES && !mesh.indices.is_empty()).then_some(mesh)
}

/// The fill's triangles (lyon), each edge vertex moved half a pixel in by the shader along its outward normal, and the
/// fringe outside them.
fn fill(subpaths: &[SubPath], rule: FillRule, tolerance: f64, mesh: &mut MeshData) -> Option<()> {
    let contours: Vec<&[[f64; 2]]> = subpaths
        .iter()
        .map(|s| s.points.as_slice())
        .filter(|p| p.len() >= 3)
        .collect();
    if contours.is_empty() {
        return Some(());
    }
    let normals = fringe::normals(&contours, rule);
    let mut b = Path::builder();
    for c in &contours {
        b.begin(point(c[0][0] as f32, c[0][1] as f32));
        for p in &c[1..] {
            b.line_to(point(p[0] as f32, p[1] as f32));
        }
        b.end(true);
    }
    let path = b.build();
    let mut buffers: VertexBuffers<PathVertex, u16> = VertexBuffers::new();
    let options = FillOptions::tolerance(tolerance.max(1e-6) as f32).with_fill_rule(match rule {
        FillRule::NonZero => LyonRule::NonZero,
        FillRule::EvenOdd => LyonRule::EvenOdd,
    });
    let made = FillTessellator::new().tessellate_path(
        &path,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
            let p = v.position();
            let dir = v
                .as_endpoint_id()
                .and_then(|e| normals.get(e.0 as usize))
                .copied()
                .unwrap_or([0.0; 2]);
            PathVertex {
                pos: [p.x, p.y],
                dir,
                side: -1.0,
                along: 1.0,
                sub: 0.0,
            }
        }),
    );
    // A fill lyon can't make (or makes past the cap) leaves the node out, said, rather than drawing its stroke alone
    //.
    if made.is_err() || buffers.vertices.len() > MAX_MESH_VERTICES {
        return None;
    }
    mesh.vertices = buffers.vertices;
    mesh.indices = buffers.indices;
    fringe::push(&contours, &normals, mesh)
}
