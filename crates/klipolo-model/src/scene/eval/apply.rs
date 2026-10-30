//! Applying one node's or one unit's tracks, property by property, through a [`Reader`].

use super::super::instance::INode;
use super::super::timeline::Prop;
use super::state::{NodeState, TextNow, UnitState};
use super::tracks::Reader;

/// One node's node-level tracks, property by property.
pub(super) fn apply(r: &Reader, list: &[u32], t: f64, s: &mut NodeState, n: &INode) {
    let prop_of = |i: u32| r.inst.pool.get(i as usize).map(|tr| tr.prop);
    let mut i = 0;
    while i < list.len() {
        let prop = prop_of(list[i]);
        let mut j = i + 1;
        while j < list.len() && prop_of(list[j]) == prop {
            j += 1;
        }
        let run = &list[i..j];
        match prop {
            Some(Prop::Clip) => s.clip = r.clip(run, t, &n.base.clip, 0),
            Some(Prop::Text) => {
                s.text = match u16::try_from(r.text(run, t, 0) as usize) {
                    Ok(0) | Err(_) => TextNow::Base,
                    Ok(i) => TextNow::Text(i),
                };
            }
            Some(p) => s.set(p, r.number(run, t, s.get(p), 0)),
            None => {}
        }
        i = j;
    }
}

/// One unit's tracks, property by property.
pub(super) fn apply_units(r: &Reader, list: &[u32], t: f64, u: &mut UnitState) {
    let prop_of = |i: u32| r.inst.pool.get(i as usize).map(|tr| tr.prop);
    let mut i = 0;
    while i < list.len() {
        let prop = prop_of(list[i]);
        let mut j = i + 1;
        while j < list.len() && prop_of(list[j]) == prop {
            j += 1;
        }
        if let Some(p) = prop {
            let rest = u.get(p).unwrap_or(0.0);
            u.set(p, r.number(&list[i..j], t, rest, 0));
        }
        i = j;
    }
}
