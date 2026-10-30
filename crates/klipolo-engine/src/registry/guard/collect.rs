//! What `flow` reads from a function's statements, in one walk: the calls, the stores with the conditions that decide
//! whether they happen, the returns, each loop's exit conditions, where each expression is evaluated, the pointers
//! handed to calls and the conditions that decide a `discard`; and the order in which expressions are evaluated and
//! stores happen, so `flow` can apply each store where it happens.
//!
//! A store is decided by the branches around it, unless its variable is declared inside that branch, and by the early
//! exits before it: a `return` or `discard` for the rest of the function, a `continue` for the rest of its
//! loop. A `break` ends its loop, so it reaches only reads after that loop (`flow`'s exit taints).

use std::collections::HashMap;

use naga::{Block, Expression as E, Function, Handle, Span, Statement as S};

use super::exits::{discards, exits, leaves};
use super::flow::Ex;

/// What a pointer reaches: a local variable, or a pointer argument, by index.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Local(Handle<naga::LocalVariable>),
    Arg(u32),
}

/// What a pointer reaches, adding the runtime indices along the way to `indices`.
pub(super) fn root(f: &Function, mut h: Ex, indices: &mut Vec<Ex>) -> Option<Target> {
    loop {
        match f.expressions[h] {
            E::LocalVariable(v) => return Some(Target::Local(v)),
            E::FunctionArgument(i) => return Some(Target::Arg(i)),
            E::Access { base, index } => {
                indices.push(index);
                h = base;
            }
            E::AccessIndex { base, .. } => h = base,
            _ => return None,
        }
    }
}

pub(super) enum Value<'f> {
    Expr(Ex),
    /// What the callee stores through its argument `arg` of this call.
    Out {
        callee: Handle<Function>,
        arg: usize,
        args: &'f [Ex],
    },
}

/// One step of the function in statement order.
#[derive(Clone, Copy)]
pub(super) enum Step {
    /// An expression evaluated: emitted, or a call's result.
    Eval(Ex),
    /// The store `stores[i]`.
    Store(usize),
    /// The end of the loop `exits[l]`: what ends it is known.
    LoopEnd(usize),
}

pub(super) struct Store<'f> {
    pub target: Target,
    pub value: Value<'f>,
    /// The conditions that decide whether it happens.
    pub gates: Vec<Ex>,
    /// The runtime indices on its pointer: which element it writes.
    pub indices: Vec<Ex>,
    /// The innermost loop around it.
    pub within: Option<usize>,
}

#[derive(Default)]
pub(super) struct Collected<'f> {
    pub calls: HashMap<Ex, (Handle<Function>, &'f [Ex])>,
    pub stores: Vec<Store<'f>>,
    pub returns: Vec<(Ex, Vec<Ex>)>,
    /// Per loop: its exit conditions, and the loop around it.
    pub exits: Vec<Vec<Ex>>,
    pub parent: Vec<Option<usize>>,
    /// Per expression: the innermost loop whose body evaluates it.
    pub within: Vec<Option<usize>>,
    /// Pointers handed to calls, with the innermost loop around the call and the callee.
    pub pointers: Vec<(Ex, Option<usize>, Handle<Function>)>,
    /// The conditions that decide whether a `discard` runs, its own or a callee's.
    pub discards: Vec<Ex>,
    /// It may `discard`, itself or through a call.
    pub may_discard: bool,
    /// The evaluations and stores in statement order, with each loop's end.
    pub order: Vec<Step>,
}

impl Collected<'_> {
    /// `l` and the loops around it.
    pub(super) fn loops_from(&self, mut l: Option<usize>) -> impl Iterator<Item = usize> + '_ {
        std::iter::from_fn(move || {
            let here = l?;
            l = self.parent[here];
            Some(here)
        })
    }
}

/// The conditions around the statement being walked, each with its branch statement's span.
#[derive(Default)]
struct Scope {
    gates: Vec<(Ex, Span)>,
    /// Early exits so far that leave the function (`return`, `discard`).
    left: Vec<(Ex, Span)>,
    /// Early exits so far that skip the rest of a loop's step (`continue`); dropped at the loop's end.
    skipped: Vec<(Ex, Span)>,
    loops: Vec<usize>,
}

impl Scope {
    fn all(&self) -> Vec<Ex> {
        self.gates
            .iter()
            .chain(&self.left)
            .chain(&self.skipped)
            .map(|&(g, _)| g)
            .collect()
    }
}

/// What `walk` looks up beside the function's statements.
pub(super) struct Lookups<'a> {
    /// A callee may `discard` (its summary says so).
    pub may: &'a dyn Fn(Handle<Function>) -> bool,
    /// The expression is a pointer: only a pointer argument lets a callee store into the caller's variables.
    pub pointer: &'a dyn Fn(Ex) -> bool,
}

pub(super) fn collect<'f>(f: &'f Function, lookups: &Lookups) -> Collected<'f> {
    let mut c = Collected {
        within: vec![None; f.expressions.len()],
        ..Collected::default()
    };
    walk(f, &f.body, &mut Scope::default(), lookups, &mut c);
    c
}

fn walk<'f>(f: &'f Function, b: &'f Block, sc: &mut Scope, fx: &Lookups, out: &mut Collected<'f>) {
    let (may, within) = (fx.may, sc.loops.last().copied());
    for (s, span) in b.span_iter() {
        match s {
            S::Emit(range) => {
                for h in range.clone() {
                    out.within[h.index()] = within;
                    out.order.push(Step::Eval(h));
                }
            }
            S::Store { pointer, value } => store(f, sc, out, *pointer, Value::Expr(*value)),
            S::Call {
                function,
                arguments,
                result,
            } => {
                if let Some(r) = result {
                    out.calls.insert(*r, (*function, arguments.as_slice()));
                    out.within[r.index()] = within;
                    out.order.push(Step::Eval(*r));
                }
                // A call that may `discard` is a `discard` here.
                if may(*function) {
                    out.may_discard = true;
                    let all = sc.all();
                    out.discards.extend(all);
                }
                // An argument passed by value is no pointer, even an argument of this function's own.
                for (arg, a) in arguments
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| (fx.pointer)(**a))
                {
                    if root(f, *a, &mut Vec::new()).is_some() {
                        out.pointers.push((*a, within, *function));
                    }
                    let value = Value::Out {
                        callee: *function,
                        arg,
                        args: arguments.as_slice(),
                    };
                    store(f, sc, out, *a, value);
                }
            }
            S::Return { value: Some(v) } => out.returns.push((*v, sc.all())),
            S::Kill => {
                out.may_discard = true;
                let all = sc.all();
                out.discards.extend(all);
            }
            S::If {
                condition,
                accept,
                reject,
            } => {
                sc.gates.push((*condition, *span));
                walk(f, accept, sc, fx, out);
                walk(f, reject, sc, fx, out);
                sc.gates.pop();
                exited(sc, may, *condition, *span, [accept, reject].into_iter());
            }
            S::Switch { selector, cases } => {
                sc.gates.push((*selector, *span));
                for case in cases {
                    walk(f, &case.body, sc, fx, out);
                }
                sc.gates.pop();
                exited(sc, may, *selector, *span, cases.iter().map(|c| &c.body));
            }
            S::Loop {
                body,
                continuing,
                break_if,
            } => {
                let id = out.exits.len();
                let mut found = Vec::new();
                exits(body, &mut found);
                let mut conds: Vec<Ex> = found.into_iter().map(|(c, _)| c).collect();
                conds.extend(*break_if);
                out.exits.push(conds);
                out.parent.push(within);
                sc.loops.push(id);
                let skipped = sc.skipped.len();
                walk(f, body, sc, fx, out);
                // `continuing` runs after a `continue` too.
                sc.skipped.truncate(skipped);
                walk(f, continuing, sc, fx, out);
                sc.loops.pop();
                out.order.push(Step::LoopEnd(id));
            }
            S::Block(inner) => walk(f, inner, sc, fx, out),
            _ => {}
        }
    }
}

/// After a branch whose arms leave early, its condition decides everything later: the rest of the function for a
/// `return`, a `discard` or a call that may discard, the rest of the loop's step for a `continue`.
fn exited<'a>(
    sc: &mut Scope,
    may: &dyn Fn(Handle<Function>) -> bool,
    cond: Ex,
    span: Span,
    arms: impl Iterator<Item = &'a Block>,
) {
    let in_loop = !sc.loops.is_empty();
    let (mut function, mut step) = (false, false);
    for arm in arms {
        function |= leaves(arm, false, false) || discards(arm, may);
        step |= in_loop && leaves(arm, false, true);
    }
    if function {
        sc.left.push((cond, span));
    } else if step {
        sc.skipped.push((cond, span));
    }
}

fn store<'f>(f: &Function, sc: &Scope, out: &mut Collected<'f>, pointer: Ex, value: Value<'f>) {
    let mut indices = Vec::new();
    let Some(target) = root(f, pointer, &mut indices) else {
        return;
    };
    let declared = match target {
        Target::Local(v) => f.local_variables.get_span(v).to_range().map(|r| r.start),
        Target::Arg(_) => None,
    };
    // A variable declared inside a branch is only seen there, and one declared after an early exit didn't exist
    // before it: neither decides its value.
    let inside = |g: &Span| declared.is_some_and(|d| g.to_range().is_some_and(|r| r.contains(&d)));
    let after = |g: &Span| declared.is_some_and(|d| g.to_range().is_some_and(|r| d >= r.end));
    let gates = sc
        .gates
        .iter()
        .filter(|(_, g)| !inside(g))
        .chain(sc.left.iter().chain(&sc.skipped).filter(|(_, g)| !after(g)))
        .map(|&(g, _)| g)
        .collect();
    out.stores.push(Store {
        target,
        value,
        gates,
        indices,
        within: sc.loops.last().copied(),
    });
    out.order.push(Step::Store(out.stores.len() - 1));
}
