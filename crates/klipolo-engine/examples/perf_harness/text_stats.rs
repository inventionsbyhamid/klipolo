//! The `text` cases' figures: p50/p95/max spreads, the median of several runs (`--runs N`), and the
//! text budgets each case is held to, printed next to its drawer's own time.

use super::measure::pct;

/// The text budgets per layer, p95: GPU ms and CPU µs.
pub const NORMAL: (f64, f64) = (2.0, 300.0);
pub const LIGHT: (f64, f64) = (0.5, 100.0);

/// p50, p95 and max of some values.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spread {
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

impl Spread {
    pub fn of(values: &[f64]) -> Self {
        let mut v = values.to_vec();
        v.sort_by(f64::total_cmp);
        Self {
            p50: pct(&v, 0.5),
            p95: pct(&v, 0.95),
            max: v.last().copied().unwrap_or(0.0),
        }
    }

    /// Each figure's median over `runs`.
    pub fn median(runs: impl Iterator<Item = Self> + Clone) -> Self {
        Self {
            p50: median(runs.clone().map(|s| s.p50)),
            p95: median(runs.clone().map(|s| s.p95)),
            max: median(runs.map(|s| s.max)),
        }
    }

    /// As a column, `places` decimals each.
    pub fn show(&self, places: usize) -> String {
        format!(
            "{:6.p$} / {:6.p$} / {:6.p$}",
            self.p50,
            self.p95,
            self.max,
            p = places
        )
    }
}

/// The median of `values` (the mean of the middle two when even); 0 for none.
pub fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    }
}

/// The median of optional figures: None when any run had none (n/a: the device has no timestamps).
pub fn median_opt<T>(
    runs: impl Iterator<Item = Option<T>>,
    median_of: impl Fn(std::vec::IntoIter<T>) -> T,
) -> Option<T> {
    let all: Option<Vec<T>> = runs.collect();
    all.filter(|v| !v.is_empty())
        .map(|v| median_of(v.into_iter()))
}

/// The drawer's own time, with each budget's verdict on its p95s: "drawer CPU p50/p95/max µs … | drawer GPU … ms (or
/// n/a) | Normal ≤ 2 ms GPU, ≤ 300 µs CPU: ok/over | Light ≤ 0.5 ms, ≤ 100 µs: ok/over".
pub fn drawer(cpu_us: Spread, gpu_ms: Option<Spread>) -> String {
    let gpu = gpu_ms.map_or_else(
        || "   n/a (no timestamps)".to_owned(),
        |g| format!("{} ms", g.show(3)),
    );
    let verdict = |(gpu_budget, cpu_budget): (f64, f64)| {
        let cpu = if cpu_us.p95 <= cpu_budget {
            "CPU ok"
        } else {
            "CPU over"
        };
        let gpu = match gpu_ms {
            Some(g) if g.p95 <= gpu_budget => "GPU ok",
            Some(_) => "GPU over",
            None => "GPU n/a",
        };
        format!("{cpu}, {gpu}")
    };
    format!(
        "drawer CPU p50/p95/max {} µs, GPU {} | budget Normal (p95 ≤ {} ms GPU, ≤ {} µs CPU): {} | Light (≤ {} ms, ≤ {} \
         µs): {}",
        cpu_us.show(0),
        gpu,
        NORMAL.0,
        NORMAL.1,
        verdict(NORMAL),
        LIGHT.0,
        LIGHT.1,
        verdict(LIGHT),
    )
}
