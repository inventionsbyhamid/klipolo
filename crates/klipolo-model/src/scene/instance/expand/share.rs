//! A compiled scene's shared tracks: each compiled node's made once, a tween's track for a property made once whatever
//! it targets (one made for another node is reused, not lowered again).

use std::collections::BTreeMap;

use super::super::super::timeline::{lower_of, Prop};
use super::super::lower::{lower, Sink};
use super::super::Ctx;
use super::{seed_of, Builder};

impl Builder<'_> {
    /// Every compiled node's shared tracks, a tween's track for a property made once whatever it targets.
    pub(super) fn share(&mut self) {
        let mut made: BTreeMap<(usize, Prop, bool), u32> = BTreeMap::new();
        let mut texts = Vec::new();
        let cs = self.cs;
        for c in 0..cs.nodes.len() {
            let split = cs.nodes[c].is_split();
            // A tween's track for a property is made once, whatever it targets: one made for another node is reused,
            // not lowered again.
            let key = |ti: usize, p: Prop| {
                let staggered = cs
                    .tweens
                    .get(ti)
                    .and_then(Option::as_ref)
                    .is_some_and(|t| t.stagger.is_some());
                (ti, p, staggered && split && p.per_unit())
            };
            let made_all = |ti: usize, name: &str| {
                lower_of(name)
                    .is_some_and(|l| l.props().iter().all(|&p| made.contains_key(&key(ti, p))))
            };
            let mut list: Vec<u32> = Vec::new();
            for &ti in &cs.nodes[c].tweens {
                let Some(Some(t)) = cs.tweens.get(ti) else {
                    continue;
                };
                for name in t
                    .tween
                    .vars
                    .keys()
                    .filter(|name| made_all(ti, name.as_str()))
                {
                    let props = lower_of(name).map(|l| l.props()).unwrap_or_default();
                    list.extend(props.iter().filter_map(|&p| made.get(&key(ti, p)).copied()));
                }
            }
            let ctx = Ctx {
                vals: self.vals,
                index: 0.0,
                row: None,
            };
            let tracks = {
                let mut sink = Sink {
                    ctx: &ctx,
                    case: cs.nodes[c].node.props.case,
                    texts: &mut texts,
                    clips: &mut self.clips,
                };
                lower(
                    cs,
                    c,
                    &mut sink,
                    self.placed,
                    seed_of(c, 0.0),
                    false,
                    &made_all,
                )
            };
            for tr in tracks {
                let k = tr.tween.map(|ti| key(ti, tr.prop));
                self.pool.push(tr);
                let index = (self.pool.len() - 1) as u32;
                if let Some(k) = k {
                    made.insert(k, index);
                }
                list.push(index);
            }
            self.lists.push(list);
            self.list_cnode.push(c);
        }
    }
}
