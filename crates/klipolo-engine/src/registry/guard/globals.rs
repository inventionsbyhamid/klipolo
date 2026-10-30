//! The prelude's globals the facts are about (`kl`, `params`, the layers), found by name, and each setting's class.

use klipolo_model::contrib::ContribDef;
use klipolo_model::params::ParamKind;
use naga::{GlobalVariable, Handle, Module, StructMember, TypeInner};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Number,
    Bool,
    Choice,
    /// Colours and points: they have no ends, so one that decides a pixel's work is measured at probes (`End::Probe`).
    Other,
}

/// `KlFrame`'s members that change over time: whatever reads one of them (or the whole `kl`) animates. Stepped and piece
/// time (ABI v1.1) reach a shader only through these; every member v1.1 added holds over a layer's frames
/// (`render::effects::frame_info`'s layout test keeps every member in one list or the other).
pub(crate) const TIME_MEMBERS: [&str; 3] = ["time", "progress", "frame"];

/// The prelude's globals the facts are about, found by name.
pub(super) struct Globals {
    pub kl: Option<Handle<GlobalVariable>>,
    pub params: Option<Handle<GlobalVariable>>,
    pub layers: Vec<Handle<GlobalVariable>>,
    /// `KlFrame`'s members that change over time (`time`, `progress`, `frame`), by name, never by position.
    pub anim: Vec<u32>,
    /// `Params`' members in order: the setting's key and its class.
    pub keys: Vec<(String, Class)>,
}

pub(super) fn class(kind: &ParamKind) -> Class {
    match kind {
        ParamKind::Float { .. } | ParamKind::Angle { .. } | ParamKind::Int { .. } => Class::Number,
        ParamKind::Bool { .. } => Class::Bool,
        ParamKind::Choice { .. } => Class::Choice,
        _ => Class::Other,
    }
}

fn members(m: &Module, h: Option<Handle<GlobalVariable>>) -> &[StructMember] {
    match h.map(|h| &m.types[m.global_variables[h].ty].inner) {
        Some(TypeInner::Struct { members, .. }) => members,
        _ => &[],
    }
}

pub(super) fn globals(m: &Module, def: &ContribDef) -> Globals {
    let find = |name: &str| {
        m.global_variables
            .iter()
            .find(|(_, g)| g.name.as_deref() == Some(name))
            .map(|(h, _)| h)
    };
    let (kl, params) = (find("kl"), find("params"));
    Globals {
        kl,
        params,
        layers: ["kl_a", "kl_b"].into_iter().filter_map(find).collect(),
        anim: members(m, kl)
            .iter()
            .enumerate()
            .filter(|(_, mem)| {
                mem.name
                    .as_deref()
                    .is_some_and(|n| TIME_MEMBERS.contains(&n))
            })
            .map(|(i, _)| i as u32)
            .collect(),
        keys: members(m, params)
            .iter()
            .map(|mem| {
                let key = mem.name.clone().unwrap_or_default();
                let class = def.param(&key).map_or(Class::Other, |s| class(&s.kind));
                (key, class)
            })
            .collect(),
    }
}
