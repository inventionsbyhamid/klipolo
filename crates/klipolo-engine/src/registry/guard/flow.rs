//! Where settings flow (for the costly settings). A forward pass over a function's expressions records, for
//! each, the settings (`params.<key>`) and arguments that reach it and whether it rises or falls with them
//! (`taint::one`). Values flow through `let`s and `var`s, and with them what decides a `var`'s value (`collect`):
//! the branches and early exits around its stores, the runtime index of an element stored, the exit conditions of the
//! loops around a store for reads after those loops (`n` counted up to where `params.k` breaks the loop depends on
//! `k`; the loop's own test doesn't see them, so a counter stays readable), and calls: a callee's result, and what it
//! stores through pointer arguments into the caller's variables.
//!
//! Each round walks the statements in order and applies each store where it happens, so straight-line
//! code settles in one round and only values carried around loops need more.
//!
//! The analysis is conservative: where tracking is uncertain a setting is
//! costly at both ends, so it is measured at both. `both` collects the settings that reach a runtime index or the
//! decision to `discard`, and `pointers` those that reach a pointer handed to a call. `costly` reads,
//! from a condition, which end of each setting's range makes it come out a given way.

use std::cell::RefCell;
use std::collections::HashMap;

use naga::valid::FunctionInfo;
use naga::{Expression as E, Function, Handle, TypeInner};

use super::End;
use super::analyze::Ctx;
use super::collect::{Collected, Lookups, Step, Store, Target, Value, collect, root};
use super::expr::{Site, one};
use super::taint::{Source, Taint, add, join, keep, unknown};

pub(super) type Ex = Handle<naga::Expression>;

/// Ends found for each source.
type Ends = Vec<(Source, End)>;

/// Rounds of the forward pass at most: values carried by `var`s around a loop need a second one, rarely a third.
const ROUNDS: usize = 16;

/// The local variable a pointer is (a part of).
pub(super) fn local_root(f: &Function, h: Ex) -> Option<Handle<naga::LocalVariable>> {
    match root(f, h, &mut Vec::new())? {
        Target::Local(v) => Some(v),
        Target::Arg(_) => None,
    }
}

pub(super) struct Flow {
    pub expr: Vec<Taint>,
    /// What reaches the function's result.
    pub ret: Taint,
    /// What it stores through each pointer argument, by argument index.
    pub outs: Vec<Taint>,
    /// What reaches a runtime index or a `discard`'s condition: costly at both ends.
    pub both: Taint,
    /// What reaches each pointer handed to a call, by callee: costly at both ends, and a colour or point there is
    /// measured at probes, its note naming the helper.
    pub pointers: Vec<(Handle<Function>, Taint)>,
    /// It may `discard`, itself or through a call.
    pub may_discard: bool,
    /// `costly`'s answers so far, by (condition, wanted value).
    pub(super) memo: RefCell<HashMap<(Ex, bool), Ends>>,
}

pub(super) fn flow(cx: &Ctx, f: &Function, fi: &FunctionInfo) -> Flow {
    let may = |callee: Handle<Function>| {
        cx.sums[callee.index()]
            .as_ref()
            .is_some_and(|s| s.may_discard)
    };
    let is_pointer = |h: Ex| {
        matches!(
            fi[h].ty.inner_with(&cx.m.types),
            TypeInner::Pointer { .. } | TypeInner::ValuePointer { .. }
        )
    };
    let c = collect(
        f,
        &Lookups {
            may: &may,
            pointer: &is_pointer,
        },
    );
    let site = Site {
        cx,
        f,
        fi,
        calls: &c.calls,
    };
    // Per local: the loops whose bodies store to it; per load of a local, those it's read after (review m-b: once).
    let mut stored_in: Vec<Vec<usize>> = vec![Vec::new(); f.local_variables.len()];
    for s in &c.stores {
        if let Target::Local(v) = s.target {
            for l in c.loops_from(s.within) {
                if !stored_in[v.index()].contains(&l) {
                    stored_in[v.index()].push(l);
                }
            }
        }
    }
    let after = |v: usize, at: Option<usize>| -> Vec<usize> {
        let around: Vec<usize> = c.loops_from(at).collect();
        stored_in[v]
            .iter()
            .copied()
            .filter(|l| !around.contains(l))
            .collect()
    };
    let read_after: HashMap<usize, Vec<usize>> = f
        .expressions
        .iter()
        .filter_map(|(h, e)| match *e {
            E::Load { pointer } => match root(f, pointer, &mut Vec::new()) {
                Some(Target::Local(v)) => Some((h.index(), after(v.index(), c.within[h.index()]))),
                _ => None,
            },
            _ => None,
        })
        .filter(|(_, loops)| !loops.is_empty())
        .collect();
    let mut local = vec![Taint::new(); f.local_variables.len()];
    let mut expr = vec![Taint::new(); f.expressions.len()];
    let mut exit_taint = vec![Taint::new(); c.exits.len()];
    // Per local, its pointer expressions, which carry what it holds; and the expressions no statement evaluates
    // (arguments, variables, constants).
    let mut pointers_of: Vec<Vec<Ex>> = vec![Vec::new(); f.local_variables.len()];
    let mut stepped = vec![false; f.expressions.len()];
    for step in &c.order {
        if let Step::Eval(h) = *step {
            stepped[h.index()] = true;
        }
    }
    for (h, e) in f.expressions.iter() {
        if let E::LocalVariable(v) = *e {
            pointers_of[v.index()].push(h);
        }
    }
    let eval = |h: Ex, expr: &mut [Taint], local: &[Taint], exit_taint: &[Taint]| {
        let mut t = one(&site, h, &f.expressions[h], expr, local);
        // Read after a loop that stores the variable: what ends that loop decides the value.
        for &l in read_after.get(&h.index()).map_or(&[][..], |v| v.as_slice()) {
            join(&mut t, &exit_taint[l], keep);
        }
        let changed = t != expr[h.index()];
        expr[h.index()] = t;
        changed
    };
    let mut settled = false;
    for _ in 0..ROUNDS {
        let mut changed = false;
        for l in 0..c.exits.len() {
            exits_of(&c, l, &expr, &mut exit_taint);
        }
        for (h, _) in f.expressions.iter().filter(|(h, _)| !stepped[h.index()]) {
            changed |= eval(h, &mut expr, &local, &exit_taint);
        }
        for (v, var) in f.local_variables.iter() {
            if let Some(init) = var.init {
                changed |= join(&mut local[v.index()], &expr[init.index()], keep);
            }
        }
        for step in &c.order {
            match *step {
                Step::Eval(h) => changed |= eval(h, &mut expr, &local, &exit_taint),
                Step::Store(i) => {
                    let s = &c.stores[i];
                    if let Target::Local(v) = s.target
                        && join(&mut local[v.index()], &stored(cx, s, &expr), keep)
                    {
                        changed = true;
                        for &p in &pointers_of[v.index()] {
                            join(&mut expr[p.index()], &local[v.index()], keep);
                        }
                    }
                }
                Step::LoopEnd(l) => exits_of(&c, l, &expr, &mut exit_taint),
            }
        }
        if !changed {
            settled = true;
            break;
        }
    }
    let mut ret = Taint::new();
    for (value, gates) in &c.returns {
        join(&mut ret, &expr[value.index()], keep);
        for g in gates {
            join(&mut ret, &expr[g.index()], unknown);
        }
    }
    // A caller sees what's stored through a pointer after the loops around the store have ended.
    let mut outs = vec![Taint::new(); f.arguments.len()];
    for s in &c.stores {
        if let Target::Arg(j) = s.target
            && let Some(out) = outs.get_mut(j as usize)
        {
            join(out, &stored(cx, s, &expr), keep);
            for l in c.loops_from(s.within) {
                join(out, &exit_taint[l], keep);
            }
        }
    }
    let (mut both, pointers) = both_ends(f, &c, &expr, &exit_taint, &stored_in);
    if !settled {
        // Still changing at the round cap: every source the function reads may reach anything.
        let mut all = Taint::new();
        for t in &expr {
            join(&mut all, t, unknown);
        }
        join(&mut both, &all, unknown);
        join(&mut ret, &all, unknown);
        for out in &mut outs {
            join(out, &all, unknown);
        }
    }
    Flow {
        expr,
        ret,
        outs,
        both,
        pointers,
        may_discard: c.may_discard,
        memo: RefCell::default(),
    }
}

/// What ends the loop `exits[l]`, as unknown.
fn exits_of(c: &Collected, l: usize, expr: &[Taint], exit_taint: &mut [Taint]) {
    for cond in &c.exits[l] {
        join(&mut exit_taint[l], &expr[cond.index()], unknown);
    }
}

/// The settings that reach a runtime index of a load or store, or the decision to `discard`; and, per
/// call, what reaches its pointers, with the exits of the loops before the call that store their variables (costly at
/// both ends through `Summary::via_pointer`).
fn both_ends(
    f: &Function,
    c: &Collected,
    expr: &[Taint],
    exit_taint: &[Taint],
    stored_in: &[Vec<usize>],
) -> (Taint, Vec<(Handle<Function>, Taint)>) {
    let mut both = Taint::new();
    let mut pointers = Vec::new();
    for (_, e) in f.expressions.iter() {
        if let E::Access { index, .. } = *e {
            join(&mut both, &expr[index.index()], unknown);
        }
    }
    for &(a, at, callee) in &c.pointers {
        let mut reached = Taint::new();
        join(&mut reached, &expr[a.index()], unknown);
        if let Some(Target::Local(v)) = root(f, a, &mut Vec::new()) {
            let around: Vec<usize> = c.loops_from(at).collect();
            for &l in stored_in[v.index()].iter().filter(|l| !around.contains(l)) {
                join(&mut reached, &exit_taint[l], unknown);
            }
        }
        pointers.push((callee, reached));
    }
    for g in &c.discards {
        join(&mut both, &expr[g.index()], unknown);
    }
    (both, pointers)
}

/// What one store puts in its target: its value's taint (for a call, the callee's out-argument mapped through the
/// call's arguments), with the conditions that decide it and the index of the element it writes as unknown.
fn stored(cx: &Ctx, s: &Store, expr: &[Taint]) -> Taint {
    let mut t = Taint::new();
    match s.value {
        Value::Expr(x) => {
            join(&mut t, &expr[x.index()], keep);
        }
        Value::Out { callee, arg, args } => {
            let outs = cx.sums[callee.index()].as_ref().map(|sum| &sum.outs);
            for &(source, sign) in outs
                .and_then(|o| o.get(arg))
                .map_or(&[][..], |o| o.as_slice())
            {
                match source {
                    Source::Param(_) => {
                        add(&mut t, source, sign);
                    }
                    Source::Arg(k) => {
                        let from = args.get(k as usize).map_or(&[][..], |a| &expr[a.index()]);
                        for &(s2, g) in from {
                            add(&mut t, s2, sign.of(g));
                        }
                    }
                }
            }
        }
    }
    for g in s.gates.iter().chain(&s.indices) {
        join(&mut t, &expr[g.index()], unknown);
    }
    t
}
