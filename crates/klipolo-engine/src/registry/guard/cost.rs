//! The cost walk of one function: operations and texture samples by loop bounds (both arms of a branch, every
//! case of a switch: an upper bound however pixels diverge), outside loops, and in the largest own body of a loop
//! (for the shared counter's bound); with the settings that decide how much work it does.
//!
//! An operation is one of naga's emitted expressions, a statement or a call, weighted by the scalars it moves and a
//! matrix product by its multiply-adds (`weights`); zeroing the function's locals and passing values to a call count
//! too. A gather reads four texels.
//!
//! Costly settings, conservatively: a loop's own test is read for its costly
//! end, and so is a branch around loops or samples; a condition around any other early exit (`return`, `discard`,
//! `break`, `continue`, a `switch` case that leaves) in a function that holds work is costly at both ends, and so is
//! whatever `flow` marks (a runtime index, a `discard`'s condition). The straight-line work each condition decides goes
//! to the work tally (`tally`), and what reaches a pointer handed to a call to `via_pointer`. Each reason
//! also records where it happens (`Summary::at`), for the note on a setting measured at probes.
//! Calls are in `calls`.

use naga::{Block, Expression as E, Function, Span, Statement as S};

use super::analyze::{Ctx, LoopRec, Summary};
use super::costly::{both, merged};
use super::exits::{exits, leaves};
use super::flow::{Ex, Flow};
use super::taint::{Source, first};
use super::tally::decided;
use super::weights::Weights;
use super::work::{Cost, HUGE, ONE, Work};
use super::{Placed, UNKNOWN_LOOP_BOUND, bounds};

pub(super) struct Walk<'a> {
    pub cx: &'a Ctx<'a>,
    pub p: &'a Placed<'a>,
    pub f: &'a Function,
    pub weights: Weights<'a>,
    pub flow: &'a Flow,
    pub sum: Summary,
    /// Loops around the statement being walked.
    pub depth: usize,
    /// Conditions with an arm that leaves early, with their branch: costly at both ends when the function holds work
    /// and deciding the function's work in the tally.
    pub leaving: Vec<(Ex, Span)>,
    /// One of the prelude's helpers (`kl_*`): when it holds no loops or samples, its early exits count only their own
    /// arms, so `kl_unpremultiply`'s return on a clear texel
    /// doesn't make every tap of a blur decide work.
    pub trusted: bool,
}

impl Walk<'_> {
    /// The whole function: its body, and zeroing its local variables at the start.
    pub(super) fn function(&mut self) -> Cost {
        let mut c = self.block(&self.f.body, 1.0, None);
        let zeroing: f64 = self
            .f
            .local_variables
            .iter()
            .map(|(_, v)| {
                self.weights
                    .scalars
                    .get(v.ty.index())
                    .copied()
                    .unwrap_or(HUGE)
            })
            .sum();
        c.add_straight(Work {
            ops: zeroing.min(HUGE),
            samples: 0.0,
        });
        let flow = self.flow;
        both(&mut self.sum.costly, &flow.both);
        for (callee, reached) in &flow.pointers {
            for &(s, _) in reached {
                first(&mut self.sum.via_pointer, s, *callee);
            }
        }
        // An early exit skips at most the whole function; in a function that holds loops or samples its condition is
        // costly at both ends as well. Not in a prelude helper without work: its branches'
        // arms are tallied like any other, and that is all its exits decide.
        let skip = self.trusted && !c.work;
        for (cond, span) in std::mem::take(&mut self.leaving)
            .into_iter()
            .filter(|_| !skip)
        {
            let t = &flow.expr[cond.index()];
            if c.work {
                both(&mut self.sum.costly, t);
                self.mark(t.iter().map(|&(s, _)| s), HUGE, span);
            }
            self.decide(t, c.bound.ops, span);
        }
        // One entry per setting or argument, whatever the number of places or call sites.
        self.sum.costly = merged(std::mem::take(&mut self.sum.costly));
        c
    }

    /// `opening`: the index in `b` of its loop's own test, when `b` is a loop's body.
    fn block(&mut self, b: &Block, mult: f64, opening: Option<usize>) -> Cost {
        let mut c = Cost::default();
        for (i, (stmt, span)) in b.span_iter().enumerate() {
            match stmt {
                S::Emit(range) => {
                    let mut w = Work::default();
                    for h in range.clone() {
                        w.ops += self.weights.of(h);
                        if let E::ImageSample { gather, .. } = self.f.expressions[h] {
                            w.samples += if gather.is_some() { 4.0 } else { 1.0 };
                            c.work = true;
                        } else if matches!(self.f.expressions[h], E::ImageLoad { .. }) {
                            w.samples += 1.0;
                            c.work = true;
                        }
                    }
                    c.add_straight(Work {
                        ops: w.ops.min(HUGE),
                        samples: w.samples,
                    });
                }
                S::Store { value, .. } => c.add_straight(Work {
                    ops: self.weights.values(*value),
                    samples: 0.0,
                }),
                S::Call {
                    function,
                    arguments,
                    ..
                } => self.call(&mut c, *function, arguments, mult, *span),
                S::If {
                    condition,
                    accept,
                    reject,
                } => {
                    let (a, r) = (
                        self.block(accept, mult, None),
                        self.block(reject, mult, None),
                    );
                    c.add_straight(ONE);
                    c.merge(a);
                    c.merge(r);
                    let flow = self.flow;
                    let t = &flow.expr[condition.index()];
                    // A branch around loops or samples: the setting that takes it is costly.
                    match (a.work, r.work) {
                        (true, true) => both(&mut self.sum.costly, t),
                        (true, false) => self.wants(*condition, true),
                        (false, true) => self.wants(*condition, false),
                        (false, false) => {}
                    }
                    if a.work || r.work {
                        self.mark(t.iter().map(|&(s, _)| s), HUGE, *span);
                    }
                    // The straight-line work it decides, each time the loops around it run.
                    self.decide(t, decided(&[a.straight.ops, r.straight.ops]) * mult, *span);
                    let early = self.depth > 0;
                    let leaves_early = leaves(accept, early, early) || leaves(reject, early, early);
                    if leaves_early && opening != Some(i) {
                        self.leaving.push((*condition, *span));
                    }
                }
                S::Switch { selector, cases } => {
                    c.add_straight(ONE);
                    let (mut work, mut arms) = (false, Vec::with_capacity(cases.len()));
                    for case in cases {
                        let k = self.block(&case.body, mult, None);
                        work |= k.work;
                        arms.push(k.straight.ops);
                        c.merge(k);
                    }
                    let flow = self.flow;
                    let t = &flow.expr[selector.index()];
                    if work {
                        both(&mut self.sum.costly, t);
                        self.mark(t.iter().map(|&(s, _)| s), HUGE, *span);
                    }
                    self.decide(t, decided(&arms) * mult, *span);
                    let early = self.depth > 0;
                    if cases.iter().any(|k| leaves(&k.body, false, early)) {
                        self.leaving.push((*selector, *span));
                    }
                }
                S::Loop {
                    body,
                    continuing,
                    break_if,
                } => {
                    let trips = bounds::trips(self.cx.m, self.f, b, i);
                    let t = f64::from(trips.unwrap_or(UNKNOWN_LOOP_BOUND));
                    let inner = (mult * t).min(HUGE);
                    let open = bounds::opening(body);
                    self.depth += 1;
                    let k = self.block(body, inner, open.map(|(lead, ..)| lead.len()));
                    let k2 = self.block(continuing, inner, None);
                    self.depth -= 1;
                    let per_iter = k.bound + k2.bound + ONE;
                    c.bound = c.bound + per_iter * t;
                    let own = k.straight + k2.straight + ONE;
                    c.max_own = c.max_own.larger(own).larger(k.max_own).larger(k2.max_own);
                    c.work = true;
                    // The loop's own test is read for its end; any other exit is costly at both ends.
                    let mut ends = Vec::new();
                    if let Some((_, cond, exit_when_true)) = open {
                        self.flow
                            .costly(self.cx, self.f, cond, !exit_when_true, &mut ends);
                    }
                    let mut found = Vec::new();
                    exits(body, &mut found);
                    let others = found.into_iter().map(|(cond, _)| cond).chain(*break_if);
                    for cond in others.filter(|&cond| open.is_none_or(|(_, o, _)| o != cond)) {
                        both(&mut ends, &self.flow.expr[cond.index()]);
                    }
                    let bound_param = ends
                        .iter()
                        .filter_map(|&(s, _)| match s {
                            Source::Param(k) => Some(k),
                            Source::Arg(_) => None,
                        })
                        .min();
                    self.mark(ends.iter().map(|&(s, _)| s), HUGE, *span);
                    self.sum.costly.extend(ends);
                    let offset = self.p.offset(*span);
                    self.sum.loops.push(LoopRec {
                        offset,
                        trips,
                        per_iter,
                        mult,
                        bound_param,
                    });
                }
                S::Block(inner) => {
                    let k = self.block(inner, mult, None);
                    c.merge(k);
                }
                // Returning a value copies it into the result, an argument or a constant included.
                S::Return { value: Some(v) } => c.add_straight(Work {
                    ops: self.weights.values(*v),
                    samples: 0.0,
                }),
                S::Break | S::Continue | S::Return { .. } | S::Kill => {}
                _ => c.add_straight(ONE),
            }
        }
        c
    }

    /// The sources that make `cond` come out `want`, at their ends.
    fn wants(&mut self, cond: Ex, want: bool) {
        self.flow
            .costly(self.cx, self.f, cond, want, &mut self.sum.costly);
    }
}
