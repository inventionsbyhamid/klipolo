//! The heal policy's budgets: how many restarts or encoder rebuilds fit in their window, and what a source past them
//! does instead.

use std::collections::VecDeque;

use super::notes::lost;
use super::{HEAL_WINDOW_NS, HEALS_PER_WINDOW, Heal, HealAction};

/// Drops the times that left the window; true (and the time kept) when another heal fits in it.
fn within_budget(times: &mut VecDeque<u64>, now_ns: u64) -> bool {
    fits(times, now_ns, HEAL_WINDOW_NS, HEALS_PER_WINDOW)
}

/// Drops the times that left `window`; true (and the time kept) when one more fits under `max`.
pub(super) fn fits(times: &mut VecDeque<u64>, now_ns: u64, window: u64, max: usize) -> bool {
    while times
        .front()
        .is_some_and(|&t| t.saturating_add(window) <= now_ns)
    {
        times.pop_front();
    }
    let fits = times.len() < max;
    if fits {
        times.push_back(now_ns);
    }
    fits
}

pub(super) fn restart(h: &mut Heal, source: &str, now_ns: u64, reason: &str) -> HealAction {
    if within_budget(&mut h.restarts, now_ns) {
        HealAction::Restart
    } else {
        HealAction::PauseSource(lost(source, reason, None))
    }
}

pub(super) fn encoder(h: &mut Heal, source: &str, status: i32, now_ns: u64) -> HealAction {
    if within_budget(&mut h.rebuilds, now_ns) {
        HealAction::RebuildEncoder
    } else if !h.lowered {
        h.lowered = true;
        HealAction::LowerPreset
    } else {
        HealAction::PauseSource(lost(source, "encoder", Some(status)))
    }
}
