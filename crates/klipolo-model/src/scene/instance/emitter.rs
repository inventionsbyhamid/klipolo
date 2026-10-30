//! An emitter node's particles, resolved with its settings and held to their ranges.

use arrayvec::ArrayVec;

use super::super::{Node, NodeType, NumRange, SceneLimits, Val, DEFAULT_PARTICLE_LIFE};
use super::Ctx;

/// An emitter's particles, resolved.
#[derive(Clone, Debug)]
pub(crate) struct Emitter {
    pub burst: u32,
    pub rate: f64,
    pub at: f64,
    /// Degrees (0 right, −90 up), and the spread around it.
    pub angle: f64,
    pub spread: f64,
    pub speed: [f64; 2],
    pub gravity: f64,
    pub drag: f64,
    pub spin: [f64; 2],
    pub life: [f64; 2],
    pub size_over_life: ArrayVec<f64, 8>,
    pub opacity_over_life: ArrayVec<f64, 8>,
    pub seed: u32,
    /// Alive at once at most: burst + rate × the longest life, rounded up.
    pub cap: usize,
    /// The particle shape's instance node.
    pub shape: Option<usize>,
}

/// An emitter node's particles.
pub(crate) fn emitter(node: &Node, ctx: &Ctx, seed: u32) -> Option<Emitter> {
    let p = &node.props;
    if node.kind != NodeType::Emitter {
        return None;
    }
    let count = |v: &Option<Val>| {
        v.as_ref()
            .and_then(|v| ctx.num(v))
            .unwrap_or(0.0)
            .clamp(0.0, 1e6)
    };
    let range = |r: Option<NumRange>, d: f64, lo: f64, hi: f64| {
        let [a, b] = r.map_or([d, d], |r| r.values());
        let held = |v: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        [held(a).min(held(b)), held(a).max(held(b))]
    };
    let finite = |v: Option<f64>, d: f64, lo: f64, hi: f64| {
        v.filter(|v| v.is_finite()).map_or(d, |v| v.clamp(lo, hi))
    };
    let list = |l: &Option<Vec<f64>>, hi: f64| -> ArrayVec<f64, 8> {
        l.iter()
            .flatten()
            .take(8)
            .map(|v| if v.is_finite() { v.clamp(0.0, hi) } else { 1.0 })
            .collect()
    };
    let burst = count(&p.burst).ceil();
    let rate = count(&p.rate);
    let life = range(p.life, DEFAULT_PARTICLE_LIFE, 0.01, 60.0);
    let alive = (burst + rate * life[1]).ceil();
    Some(Emitter {
        burst: burst as u32,
        rate,
        at: finite(p.at, 0.0, 0.0, 600.0),
        angle: finite(p.cone.and_then(|c| c.angle), -90.0, -3600.0, 3600.0),
        spread: finite(p.cone.and_then(|c| c.spread), 360.0, 0.0, 360.0),
        speed: range(p.speed, 200.0, 0.0, 1e5),
        gravity: finite(p.gravity, 0.0, -1e5, 1e5),
        drag: finite(p.drag, 0.0, 0.0, 100.0),
        spin: range(p.spin, 0.0, -1e5, 1e5),
        life,
        size_over_life: list(&p.size_over_life, 100.0),
        opacity_over_life: list(&p.opacity_over_life, 1.0),
        seed: p.seed.unwrap_or(seed),
        cap: if alive.is_finite() {
            (alive as usize).min(SceneLimits::MAX_PARTICLES)
        } else {
            0
        },
        shape: None,
    })
}
