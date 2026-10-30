//! The critically damped follower (ζ = 1), solved exactly over a step with its target held. So any step is stable,
//! and from rest it approaches a held target without passing it (its impulse response, ω²·t·e^(−ωt), is never
//! negative).

/// The follower after `dt` seconds toward `target` (held over the step), from position `x` and velocity `v`; ω in 1/s.
/// Exact: x(t) = T + (e + (v + ω e) t) e^{−ωt}, v(t) = (v − ω (v + ω e) t) e^{−ωt}, e = x − T. Stable for any dt.
pub fn step(x: f64, v: f64, target: f64, omega: f64, dt: f64) -> (f64, f64) {
    Filter::new(omega, dt).step(x, v, target)
}

/// `step` with ω and dt fixed, its decay computed once: a build takes hundreds of thousands of steps.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Filter {
    omega: f64,
    dt: f64,
    decay: f64,
}

impl Filter {
    pub(crate) fn new(omega: f64, dt: f64) -> Self {
        Self {
            omega,
            dt,
            decay: (-omega * dt).exp(),
        }
    }

    pub(crate) fn step(&self, x: f64, v: f64, target: f64) -> (f64, f64) {
        let e = x - target;
        let c = v + self.omega * e;
        (
            target + (e + c * self.dt) * self.decay,
            (v - self.omega * c * self.dt) * self.decay,
        )
    }
}
