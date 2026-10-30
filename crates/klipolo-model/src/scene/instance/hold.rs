//! Where an instance's still is drawn (its hold point): the latest moment before any exit
//! starts, else the scene's end, so a still is never a blank exit. An exit is a property's last change to a value that
//! hides its node (opacity or `visible` 0, a scale of 0, a position a whole box or a 1080p frame's width away) after
//! which nothing brings it back; a `set` to 0 before an entrance or a blink that repeats forever isn't one.

use super::super::eval::tracks::secs;
use super::super::timeline::{Prop, Track};
use super::SceneInstance;

/// A position this far from rest has left its box: a whole box, or a 1080p frame's width in pixels.
const OFF_BOX: f64 = 1.0;
const OFF_FRAME_PX: f64 = 1920.0;

/// The latest moment before any exit of `inst` starts (a microsecond before it), else the scene's end.
pub(crate) fn hold_point(inst: &SceneInstance) -> f64 {
    let duration = inst.compiled.duration;
    let mut used = vec![false; inst.lists.len()];
    for n in &inst.nodes {
        if let Some(u) = used.get_mut(n.list) {
            *u = true;
        }
    }
    let mut first_exit: Option<i64> = None;
    for (list, _) in inst.lists.iter().zip(&used).filter(|(_, u)| **u) {
        // Each run of one property (node-level, then units) in timeline order: its last exit nothing undoes.
        let mut run: Option<(bool, Prop)> = None;
        let mut exit: Option<i64> = None;
        for tr in list.iter().filter_map(|&i| inst.pool.get(i as usize)) {
            if run != Some((tr.unit, tr.prop)) {
                first_exit = earlier(first_exit, exit.take());
                run = Some((tr.unit, tr.prop));
            }
            match ends_hidden(tr) {
                Some(true) => exit = exit.or_else(|| hides_from(tr)),
                Some(false) => exit = None,
                None => {}
            }
        }
        first_exit = earlier(first_exit, exit);
    }
    match first_exit {
        Some(at) => (secs(at) - 1e-6).clamp(0.0, duration),
        None => duration,
    }
}

fn earlier(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn hides(prop: Prop, v: f64) -> bool {
    match prop {
        Prop::Opacity | Prop::Visible => v <= 0.0,
        Prop::Scale | Prop::ScaleX | Prop::ScaleY => v.abs() < 1e-6,
        Prop::X | Prop::Y => v.abs() >= OFF_BOX,
        Prop::XPx | Prop::YPx => v.abs() >= OFF_FRAME_PX,
        _ => false,
    }
}

/// Whether `tr` leaves its node hidden once it has run: None when it ends on the value the property had (a chained
/// key, as a `from` does), and a repeat that runs forever or a yoyo that ends on its start isn't hidden.
fn ends_hidden(tr: &Track) -> Option<bool> {
    let last = tr.keys.len().checked_sub(1)?;
    if tr.modifier.is_some() {
        return None;
    }
    if let Some(r) = tr.repeat {
        if r.count < 0 || (r.yoyo && r.count % 2 == 1) {
            return Some(false);
        }
    }
    if tr.chain == Some(last) {
        return None;
    }
    Some(hides(tr.prop, tr.keys[last].v))
}

/// When `tr` starts on its last run of hiding keys: the key before them (a chained key, the value it had, never
/// hides); None when every key hides.
fn hides_from(tr: &Track) -> Option<i64> {
    let shown = |i: usize| tr.chain == Some(i) || !hides(tr.prop, tr.keys[i].v);
    (0..tr.keys.len())
        .rev()
        .find(|&i| shown(i))
        .map(|i| tr.keys[i].t_us)
}
