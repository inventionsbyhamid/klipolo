//! Particles, analytic: each one's spawn time, direction, speed, life and spin come from hashing (seed, index), and its
//! place at age τ is p0 + v·τ + ½·g·τ² (with drag d, v·(1 − e^(−dτ))/d for the first term). A burst spawns all at
//! once at `at`; a rate spawns one every 1/rate seconds from `at`. At most `MAX_PARTICLES` are written a frame, newest
//! first for a rate; a frame looks at no more than four candidates per particle it may write, over every emitter
//! together, and counts the rest without evaluating them.

use super::super::modifiers::rand01;
use super::super::timeline::Prop;
use super::super::units::MAX_PX;
use super::super::SceneLimits;
use super::{FrameCounts, SceneFrame};
use crate::scene::instance::{Emitter, SceneInstance};

/// One live particle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleState {
    /// The emitter's instance node.
    pub emitter: u32,
    /// Where it spawned in the emitter's box, fractions.
    pub u: f64,
    pub v: f64,
    /// How far it has moved since, in 1080p pixels (down is positive).
    pub dx_px: f64,
    pub dy_px: f64,
    /// Degrees.
    pub rotate: f64,
    pub scale: f64,
    pub opacity: f64,
    /// Seconds since it spawned, and how long it lives.
    pub age: f64,
    pub life: f64,
}

/// `list` spread evenly over a life, read at `p` (0–1); 1 when empty.
fn over_life(list: &[f64], p: f64) -> f64 {
    match list {
        [] => 1.0,
        [one] => *one,
        _ => {
            let x = p.clamp(0.0, 1.0) * (list.len() - 1) as f64;
            let i = (x.floor() as usize).min(list.len() - 2);
            let f = x - i as f64;
            list[i] + (list[i + 1] - list[i]) * f
        }
    }
}

fn pick(range: [f64; 2], r: f64) -> f64 {
    range[0] + (range[1] - range[0]) * r
}

/// Particle `k` of emitter `e` at age `age`, if it is alive.
fn particle(em: &Emitter, e: usize, k: u32, age: f64) -> Option<ParticleState> {
    let r = |salt: u32| rand01(em.seed, k, salt);
    let life = pick(em.life, r(3));
    if !(0.0..life).contains(&age) {
        return None;
    }
    let dir = (em.angle + (r(1) - 0.5) * em.spread).to_radians();
    let speed = pick(em.speed, r(2));
    let travel = if em.drag > 0.0 {
        (1.0 - (-em.drag * age).exp()) / em.drag
    } else {
        age
    };
    let p = age / life;
    // Held within the pixel and rotation ranges however long and fast it flies.
    let px = |v: f64| {
        if v.is_finite() {
            v.clamp(-MAX_PX, MAX_PX)
        } else {
            0.0
        }
    };
    let rotate = r(7) * 360.0 + pick(em.spin, r(4)) * age;
    Some(ParticleState {
        emitter: e as u32,
        u: r(5),
        v: r(6),
        dx_px: px(speed * dir.cos() * travel),
        dy_px: px(speed * dir.sin() * travel + 0.5 * em.gravity * age * age),
        rotate: Prop::Rotate.clamp(rotate),
        scale: over_life(&em.size_over_life, p),
        opacity: over_life(&em.opacity_over_life, p),
        age,
        life,
    })
}

pub(super) fn write(inst: &SceneInstance, t: f64, out: &mut SceneFrame, counts: &mut FrameCounts) {
    out.particles.clear();
    let room = inst
        .particle_cap
        .min(SceneLimits::MAX_PARTICLES)
        .min(out.particles.capacity());
    // One budget for the frame over every emitter: four looks per particle it may write. Once the
    // room or the looks run out, the rest are counted, not evaluated.
    let budget = room.saturating_mul(4);
    let mut looks = 0usize;
    for &e in &inst.emitters {
        let Some(em) = inst.nodes.get(e).and_then(|n| n.emitter.as_deref()) else {
            continue;
        };
        if !out.nodes.get(e).is_some_and(|s| s.drawn) {
            continue;
        }
        let age0 = t - em.at;
        if age0 < 0.0 {
            continue;
        }
        // Whether one more candidate may be looked at; when not, `left` of them are counted as dropped.
        let mut go = |out: &SceneFrame, counts: &mut FrameCounts, left: usize| -> bool {
            if out.particles.len() >= room || looks >= budget {
                counts.dropped_particles = counts.dropped_particles.saturating_add(left);
                return false;
            }
            looks += 1;
            true
        };
        // A burst's particles all spawned at `at`: none lives past the longest life.
        let burst = if age0 < em.life[1] { em.burst } else { 0 };
        for k in 0..burst {
            if !go(out, counts, (burst - k) as usize) {
                break;
            }
            if let Some(p) = particle(em, e, k, age0) {
                out.particles.push(p);
            }
        }
        if em.rate > 0.0 {
            let newest = (age0 * em.rate).floor().min(f64::from(u32::MAX - em.burst));
            let oldest = ((age0 - em.life[1]) * em.rate).ceil().max(0.0);
            let mut k = newest;
            while k >= oldest {
                if !go(out, counts, (k - oldest + 1.0) as usize) {
                    break;
                }
                let spawned = em.at + k / em.rate;
                if let Some(p) = particle(em, e, em.burst + k as u32, t - spawned) {
                    out.particles.push(p);
                }
                k -= 1.0;
            }
        }
    }
    counts.particles = out.particles.len();
    counts.particle_looks = looks;
}
