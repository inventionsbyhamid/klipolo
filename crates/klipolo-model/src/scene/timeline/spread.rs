//! How a tween's stagger spreads its members and how it repeats.

use super::super::{StaggerArg, StaggerFrom};

/// How a stagger spreads its members' starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stagger {
    pub each: Option<f64>,
    pub amount: Option<f64>,
    pub from: StaggerFrom,
    pub seed: u32,
}

impl Stagger {
    pub fn of(arg: &StaggerArg, seed: u32) -> Self {
        let finite = |v: Option<f64>| v.filter(|v| v.is_finite()).map(|v| v.max(0.0));
        match *arg {
            StaggerArg::Each(each) => Self {
                each: finite(Some(each)),
                amount: None,
                from: StaggerFrom::Start,
                seed,
            },
            StaggerArg::Spec {
                each,
                amount,
                from,
                seed: s,
            } => Self {
                each: finite(each),
                amount: finite(amount),
                from: from.unwrap_or(StaggerFrom::Start),
                seed: s.unwrap_or(seed),
            },
        }
    }

    /// How long it spreads `members`: each × (members − 1), or the amount (the check's `stagger_total`).
    pub fn total(&self, members: usize) -> f64 {
        let gaps = members.saturating_sub(1) as f64;
        let total = match (self.each, self.amount) {
            (Some(e), _) => e * gaps,
            (None, Some(a)) => a,
            (None, None) => 0.0,
        };
        if total.is_finite() {
            total.max(0.0)
        } else {
            0.0
        }
    }
}

/// Extra runs of a tween, per unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Repeat {
    /// −1 until the scene ends.
    pub count: i32,
    pub yoyo: bool,
    pub delay: f64,
}
