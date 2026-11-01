//! Static facts from naga's IR. Each function is summarised once, callees first (WGSL has no recursion): its
//! cost by loop bounds and by the shared loop counter, whether it animates or reads its layer, and the settings that
//! decide how much work it does. The entry function's summary gives the facts, with a note for each colour or point
//! setting measured at probes (`settings`). Refused here: a bound over the cap, with advice
//! for the loop that can bring it under.

use klipolo_model::contrib::ContribDef;
use naga::valid::ModuleInfo;
use naga::{Block, Expression as E, Function, GlobalVariable, Handle, Module, Statement as S};

use super::cost::Walk;
use super::flow;
use super::globals::{Class, Globals, globals};
use super::rules::entry_name;
use super::taint::{Source, Taint};
use super::tally::{Tally, Where};
use super::weights::{Weights, type_scalars};
use super::work::Work;
use super::{Diagnostic, End, LOOP_TRIPS, LoopFact, Placed, StaticFacts, advice, settings};
use crate::registry::shader::ShaderKind;

pub(super) struct Ctx<'a> {
    pub m: &'a Module,
    pub g: &'a Globals,
    pub sums: &'a [Option<Summary>],
}

pub(super) struct LoopRec {
    /// In the body, when the body wrote it.
    pub offset: Option<usize>,
    pub trips: Option<u32>,
    /// One iteration by bounds, nested loops included.
    pub per_iter: Work,
    /// Its enclosing loops' trips in this function.
    pub mult: f64,
    pub bound_param: Option<u32>,
}

#[derive(Default)]
pub(super) struct Summary {
    pub bound: Work,
    pub straight: Work,
    pub max_own: Work,
    pub has_work: bool,
    pub animates: bool,
    pub reads_layer: bool,
    /// Reads `params` whole (copies it, or passes it to a function), itself or through a callee.
    pub whole_params: bool,
    /// It may `discard`, itself or through a call.
    pub may_discard: bool,
    /// The sources that reach a pointer handed to a helper, costly at both ends, each with one such helper (for
    /// the Probe note): one entry per source, sorted.
    pub via_pointer: Vec<(Source, Handle<Function>)>,
    pub ret: Taint,
    /// What it stores through each pointer argument.
    pub outs: Vec<Taint>,
    pub costly: Vec<(Source, End)>,
    /// The straight-line operations each source decides, per run of the function (the work tally).
    pub decides: Tally,
    /// Where in this function each source decides the most work: the note's place.
    pub at: Vec<(Source, Where)>,
    /// Call sites and their loops' trips around them.
    pub calls: Vec<(Handle<Function>, f64)>,
    pub loops: Vec<LoopRec>,
}

/// The facts, why the shader is refused (empty when it isn't), and the notes on settings measured at probes.
pub(super) fn facts(
    p: &Placed,
    def: &ContribDef,
    kind: ShaderKind,
    m: &Module,
    info: &ModuleInfo,
) -> (StaticFacts, Vec<Diagnostic>, Vec<Diagnostic>) {
    let g = globals(m, def);
    let scalars = type_scalars(m);
    let mut a = Analysis {
        p,
        m,
        info,
        g: &g,
        scalars: &scalars,
        sums: (0..m.functions.len()).map(|_| None).collect(),
        visiting: vec![false; m.functions.len()],
    };
    let body_fns: Vec<_> = m
        .functions
        .iter()
        .filter(|(h, _)| p.offset(m.functions.get_span(*h)).is_some())
        .map(|(h, _)| h)
        .collect();
    for &h in &body_fns {
        a.ensure(h);
    }
    let sums = a.sums;
    let mut loops: Vec<(usize, LoopFact)> = body_fns
        .iter()
        .filter_map(|h| sums[h.index()].as_ref())
        .flat_map(|s| &s.loops)
        .filter_map(|rec| {
            let offset = rec.offset?;
            let bound_param = rec
                .bound_param
                .and_then(|k| g.keys.get(k as usize))
                .filter(|(_, c)| *c != Class::Other);
            let line = p.body[..offset].matches('\n').count() as u32 + 1;
            let fact = LoopFact {
                line,
                max_trips: rec.trips,
                bound_param: bound_param.map(|(k, _)| k.clone()),
            };
            Some((offset, fact))
        })
        .collect();
    loops.sort_by_key(|(offset, _)| *offset);
    let mut facts = StaticFacts {
        loops: loops.into_iter().map(|(_, l)| l).collect(),
        ops_per_pixel: 0,
        samples_per_pixel: 0,
        ir_expressions: body_fns
            .iter()
            .map(|h| m.functions[*h].expressions.len())
            .sum(),
        functions: body_fns.len(),
        animates: false,
        reads_layer: false,
        costly_params: Vec::new(),
    };
    let name = entry_name(kind);
    let entry = body_fns
        .iter()
        .copied()
        .find(|h| m.functions[*h].name.as_deref() == Some(name));
    let Some((entry, e)) = entry.and_then(|h| Some((h, sums[h.index()].as_ref()?))) else {
        return (facts, Vec::new(), Vec::new());
    };
    let by_counter = e.straight + e.max_own * f64::from(LOOP_TRIPS);
    facts.ops_per_pixel = e.bound.ops.min(by_counter.ops) as u64;
    facts.samples_per_pixel = e
        .bound
        .samples
        .min(by_counter.samples)
        .min(f64::from(u32::MAX)) as u32;
    facts.animates = e.animates;
    facts.reads_layer = e.reads_layer;
    let (costly, notes) = settings::costly(p, def, &g, m, e);
    facts.costly_params = costly;
    let refused: Vec<Diagnostic> = advice::over_cap(p, &sums, entry, &facts)
        .into_iter()
        .collect();
    (facts, refused, notes)
}

struct Analysis<'a> {
    p: &'a Placed<'a>,
    m: &'a Module,
    info: &'a ModuleInfo,
    g: &'a Globals,
    scalars: &'a [f64],
    sums: Vec<Option<Summary>>,
    visiting: Vec<bool>,
}

impl Analysis<'_> {
    /// Summarises `h` after its callees. A cycle (which validation refuses) leaves the callee without a summary.
    fn ensure(&mut self, h: Handle<Function>) {
        if self.sums[h.index()].is_some() || self.visiting[h.index()] {
            return;
        }
        self.visiting[h.index()] = true;
        let (m, g) = (self.m, self.g);
        let f = &m.functions[h];
        let mut callees = Vec::new();
        callees_of(&f.body, &mut callees);
        for c in callees {
            self.ensure(c);
        }
        let cx = Ctx {
            m,
            g,
            sums: &self.sums,
        };
        let fi = &self.info[h];
        let flow = flow::flow(&cx, f, fi);
        let mut walk = Walk {
            cx: &cx,
            p: self.p,
            f,
            weights: Weights {
                m,
                f,
                fi,
                scalars: self.scalars,
            },
            flow: &flow,
            sum: Summary::default(),
            depth: 0,
            leaving: Vec::new(),
            // The prelude's helpers: outside the body, and named `kl_*` (a body can't declare such a name).
            trusted: self.p.offset(m.functions.get_span(h)).is_none()
                && f.name.as_deref().is_some_and(|n| n.starts_with("kl_")),
        };
        let c = walk.function();
        let mut sum = walk.sum;
        (sum.bound, sum.straight, sum.max_own) = (c.bound, c.straight, c.max_own);
        sum.has_work |= c.work;
        let global = |x: Handle<naga::Expression>, v: Option<Handle<GlobalVariable>>| matches!(f.expressions[x], E::GlobalVariable(y) if Some(y) == v);
        for (_, e) in f.expressions.iter() {
            match *e {
                E::AccessIndex { base, index } if global(base, g.kl) && g.anim.contains(&index) => {
                    sum.animates = true
                }
                // The whole `kl`, copied or passed by value: everything in it is read.
                E::Load { pointer } if global(pointer, g.kl) => sum.animates = true,
                E::Load { pointer } if global(pointer, g.params) => sum.whole_params = true,
                E::GlobalVariable(v) if g.layers.contains(&v) => sum.reads_layer = true,
                _ => {}
            }
        }
        (sum.ret, sum.outs) = (flow.ret, flow.outs);
        sum.may_discard = flow.may_discard;
        self.sums[h.index()] = Some(sum);
    }
}

fn callees_of(b: &Block, out: &mut Vec<Handle<Function>>) {
    for s in b.iter() {
        match s {
            S::Call { function, .. } => out.push(*function),
            S::Block(x) => callees_of(x, out),
            S::If { accept, reject, .. } => {
                callees_of(accept, out);
                callees_of(reject, out);
            }
            S::Switch { cases, .. } => cases.iter().for_each(|c| callees_of(&c.body, out)),
            S::Loop {
                body, continuing, ..
            } => {
                callees_of(body, out);
                callees_of(continuing, out);
            }
            _ => {}
        }
    }
}
