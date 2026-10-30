//! Making a node's tracks: its own keys (`animate`), then each tween that targets it, every value resolved with the
//! layer's settings. Tracks are shared: a compiled node's tracks are made once, and a tween's
//! track for a property once whatever it targets, so memory grows with tweens × properties, not with instances. Only
//! a track whose values bind the repeat's index or row (`$i`, `$row.…`), or that holds texts (each instance's own), is
//! made per instance node. A key whose value doesn't resolve is left out; a key that takes the value the property had
//! (`chain`) is read when the frame is evaluated, with that member's stagger offsets.

use arrayvec::ArrayVec;

use super::super::check::{binding, Binding};
use super::super::eval::tracks::us;
use super::super::eval::ClipState;
use super::super::timeline::{lower_of, CompiledScene, Lower, Prop, Stagger, Track, Verb};
use super::super::units::{parse_clip, parse_origin};
use super::super::{TextCase, Val};
use super::own::own_keys;
use super::statics::{clip_state, length};
use super::Ctx;
use crate::motion::{Ease, Key};

/// Where values of one node go while its tracks are made.
pub(crate) struct Sink<'a> {
    pub ctx: &'a Ctx<'a>,
    pub case: Option<TextCase>,
    pub texts: &'a mut Vec<String>,
    /// The instance's clips (index 0 stands for a node's own).
    pub clips: &'a mut Vec<ClipState>,
}

impl Sink<'_> {
    /// The numbers one value writes, by track; None where it doesn't resolve.
    pub(super) fn values(
        &mut self,
        lower: Lower,
        v: &Val,
        px: bool,
    ) -> Option<ArrayVec<(Prop, f64), 4>> {
        let mut out = ArrayVec::new();
        match lower {
            Lower::Pos(f, p) => {
                let [a, b] = self.ctx.len(v, px)?;
                out.push((f, a));
                out.push((p, b));
            }
            Lower::Num(p) => out.push((p, self.ctx.num(v)?)),
            Lower::Color(props) => {
                let Val::Str(s) = v else { return None };
                let c = self.ctx.color(s)?;
                for (p, x) in props.into_iter().zip(c) {
                    out.push((p, f64::from(x)));
                }
            }
            Lower::Flag(p) => out.push((p, f64::from(u8::from(self.ctx.flag_val(v)?)))),
            Lower::Origin => {
                let Val::Str(s) = v else { return None };
                let (a, b) = parse_origin(s)?;
                let (a, b) = (length(a), length(b));
                out.push((Prop::OriginX, a[0]));
                out.push((Prop::OriginXPx, a[1]));
                out.push((Prop::OriginY, b[0]));
                out.push((Prop::OriginYPx, b[1]));
            }
            Lower::Clip => {
                let Val::Str(s) = v else { return None };
                let c = clip_state(parse_clip(s)?);
                if self.clips.len() >= usize::from(u16::MAX) {
                    return None;
                }
                self.clips.push(c);
                out.push((Prop::Clip, (self.clips.len() - 1) as f64));
            }
            Lower::Text => {
                if self.texts.len() >= usize::from(u16::MAX) {
                    return None;
                }
                let text = match v {
                    Val::Str(s) => self.ctx.text(s, self.case),
                    Val::Num(n) => self.ctx.number_text(*n, self.case),
                };
                self.texts.push(text);
                out.push((Prop::Text, (self.texts.len() - 1) as f64));
            }
        }
        Some(out)
    }
}

/// Whether a track's values switch at its keys instead of easing: on/off and texts.
pub(super) fn holds(p: Prop) -> bool {
    p.switches()
}

/// Whether a value differs from one repeat instance to the next: it binds `$i` or the row.
fn per_instance(v: &Val) -> bool {
    matches!(v, Val::Str(s) if matches!(
        binding(s),
        Some(Binding::Index | Binding::RowLabel | Binding::RowValue(_) | Binding::BadRow)
    ))
}

/// Whether tracks of a property name from these values are each instance's own: texts always (each instance has its
/// own texts), anything bound to the repeat's index or row.
fn own_per_instance<'v>(name: &str, mut values: impl Iterator<Item = &'v Val>) -> bool {
    lower_of(name) == Some(Lower::Text) || values.any(per_instance)
}

/// Whether compiled node `c` has tracks made per instance.
pub(crate) fn has_own(cs: &CompiledScene, c: usize) -> bool {
    let Some(cn) = cs.nodes.get(c) else {
        return false;
    };
    let keys = cn
        .node
        .animate
        .iter()
        .any(|(name, keys)| own_per_instance(name, keys.iter().map(|k| &k.v)));
    let tweens = cn.tweens.iter().any(|&ti| {
        let Some(Some(t)) = cs.tweens.get(ti) else {
            return false;
        };
        t.tween.vars.iter().any(|(name, v)| {
            let start = t.tween.start.as_ref().and_then(|s| s.get(name));
            own_per_instance(name, std::iter::once(v).chain(start))
        })
    });
    keys || tweens
}

/// Compiled node `c`'s tracks: the shared ones (`own` false), or the ones each instance node has of its own, leaving
/// out a tween's property `skip` says is made already. Tweens' keys are placed at `placed`.
pub(crate) fn lower(
    cs: &CompiledScene,
    c: usize,
    sink: &mut Sink,
    placed: &[(f64, f64)],
    seed: u32,
    own: bool,
    skip: &dyn Fn(usize, &str) -> bool,
) -> Vec<Track> {
    let mut tracks = Vec::new();
    let Some(cn) = cs.nodes.get(c) else {
        return tracks;
    };
    let split = cn.is_split();
    let own_stagger = cn
        .node
        .props
        .stagger
        .as_ref()
        .map(|st| Stagger::of(st, seed));
    for (name, keys) in &cn.node.animate {
        let Some(lower) = lower_of(name) else {
            continue;
        };
        if own_per_instance(name, keys.iter().map(|k| &k.v)) == own {
            own_keys(sink, lower, keys, split, own_stagger, &mut tracks);
        }
    }
    for &ti in &cn.tweens {
        let Some(Some(t)) = cs.tweens.get(ti) else {
            continue;
        };
        let start = placed.get(ti).map_or(0.0, |p| p.0);
        // A change at one instant (a 0 s tween) keys the value before it a microsecond earlier, so the new value shows
        // from the instant itself (a time's first key is the old value).
        let end_us = us(start + t.timing.dur);
        let start_us = us(start).min(end_us - 1);
        for (name, v) in &t.tween.vars {
            let Some(lower) = lower_of(name) else {
                continue;
            };
            let start_v = t.tween.start.as_ref().and_then(|s| s.get(name));
            if own_per_instance(name, std::iter::once(v).chain(start_v)) != own || skip(ti, name) {
                continue;
            }
            let Some(to) = sink.values(lower, v, false) else {
                continue;
            };
            let from = match (t.verb, start_v) {
                (Verb::FromTo, Some(sv)) => sink.values(lower, sv, false),
                _ => None,
            };
            for (k, &(prop, value)) in to.iter().enumerate() {
                let keyed = |v: f64, ease: Option<Ease>, t_us: i64| Key { t_us, v, ease };
                let into = Some(if holds(prop) { Ease::Hold } else { t.ease });
                let (keys, chain) = match t.verb {
                    Verb::To => (
                        vec![keyed(0.0, None, start_us), keyed(value, into, end_us)],
                        Some(0),
                    ),
                    Verb::From => (
                        vec![keyed(value, None, start_us), keyed(0.0, into, end_us)],
                        Some(1),
                    ),
                    Verb::FromTo => match from.as_ref().and_then(|f| f.get(k)) {
                        Some(&(_, sv)) => (
                            vec![keyed(sv, None, start_us), keyed(value, into, end_us)],
                            None,
                        ),
                        None => (
                            vec![keyed(0.0, None, start_us), keyed(value, into, end_us)],
                            Some(0),
                        ),
                    },
                    // A set switches at its time, whatever `dur` says.
                    Verb::Set => (
                        vec![
                            keyed(0.0, None, us(start) - 1),
                            keyed(value, Some(Ease::Hold), us(start)),
                        ],
                        Some(0),
                    ),
                };
                tracks.push(Track {
                    prop,
                    keys,
                    stagger: t.stagger,
                    repeat: t.repeat,
                    modifier: None,
                    unit: t.stagger.is_some() && split && prop.per_unit(),
                    tween: Some(ti),
                    chain,
                    placed_us: us(start),
                });
            }
        }
    }
    tracks
}
