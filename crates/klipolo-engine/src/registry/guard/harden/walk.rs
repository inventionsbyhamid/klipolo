//! The walk that reads a module's loops: it counts each loop and whether its body opens with the
//! host's counter check, on the IR, whatever names naga's writer chose.

use naga::{
    AddressSpace, BinaryOperator, Block, Expression, Function, GlobalVariable, Handle, Literal,
    Module, Scalar, Statement, TypeInner,
};

use super::TRIPS;
use crate::registry::guard::{LOOP_TRIPS, each_statement};

/// A module's loops, and how many of them open with the host's counter check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopGuards {
    pub loops: usize,
    pub guarded: usize,
}

/// Every loop of `module`'s functions and entry points (at any depth: loop bodies and `continuing`, blocks, both arms
/// of an `if`, every `switch` case), and those whose body opens with the host's check on the IR, whatever names
/// naga's writer chose: an `Emit` of a `Load` of the `kl_trips` global, an `If` on `GreaterEqual(that load, 4096u)`
/// whose only statement is `break`, then a `Store` of a load of `kl_trips` plus `1u` to `kl_trips` (naga's writer
/// bakes the first load and stores `_eN + 1u`), with only emits between.
pub fn loop_guards(module: &Module) -> LoopGuards {
    let trips = module
        .global_variables
        .iter()
        .find(|(_, g)| {
            g.name.as_deref() == Some(TRIPS)
                && g.space == AddressSpace::Private
                && g.binding.is_none()
                && module.types[g.ty].inner == TypeInner::Scalar(Scalar::U32)
        })
        .map(|(h, _)| h);
    let mut g = LoopGuards {
        loops: 0,
        guarded: 0,
    };
    let functions = module
        .functions
        .iter()
        .map(|(_, f)| f)
        .chain(module.entry_points.iter().map(|ep| &ep.function));
    for f in functions {
        each_statement(&f.body, &mut |s| {
            if let Statement::Loop { body, .. } = s {
                g.loops += 1;
                g.guarded += usize::from(trips.is_some_and(|t| opens_with_check(f, body, t)));
            }
        });
    }
    g
}

fn opens_with_check(f: &Function, body: &Block, trips: Handle<GlobalVariable>) -> bool {
    let e = &f.expressions;
    let is_trips =
        |p: Handle<Expression>| matches!(e[p], Expression::GlobalVariable(v) if v == trips);
    let is_u32 = |x: Handle<Expression>, n: u32| matches!(e[x], Expression::Literal(Literal::U32(v)) if v == n);
    let is_load =
        |x: Handle<Expression>| matches!(e[x], Expression::Load { pointer } if is_trips(pointer));
    let mut emitted = Vec::new();
    let mut first = Vec::with_capacity(2);
    for s in body.iter() {
        match s {
            Statement::Emit(range) => emitted.extend(range.clone()),
            s if first.len() < 2 => first.push(s),
            _ => break,
        }
    }
    let [
        Statement::If {
            condition,
            accept,
            reject,
        },
        Statement::Store { pointer, value },
    ] = first.as_slice()
    else {
        return false;
    };
    let Expression::Binary {
        op: BinaryOperator::GreaterEqual,
        left: load,
        right: limit,
    } = e[*condition]
    else {
        return false;
    };
    is_load(load)
        && emitted.contains(&load)
        && is_u32(limit, LOOP_TRIPS)
        && accept.len() == 1
        && matches!(accept.first(), Some(Statement::Break))
        && reject.is_empty()
        && is_trips(*pointer)
        && matches!(e[*value], Expression::Binary { op: BinaryOperator::Add, left, right }
            if is_load(left) && emitted.contains(&left) && is_u32(right, 1))
}

/// Each function's, then each entry point's, calls in statement order, by the callee's index: what naga's writer must
/// keep. The writer writes each call once and in place, and functions in their order, so an honest round trip
/// gives the same lists; a function whose name came back as a built-in's adds the built-in's calls to them.
pub(super) fn calls(m: &Module) -> Vec<Vec<usize>> {
    m.functions
        .iter()
        .map(|(_, f)| f)
        .chain(m.entry_points.iter().map(|ep| &ep.function))
        .map(|f| {
            let mut out = Vec::new();
            each_statement(&f.body, &mut |s| {
                if let Statement::Call { function, .. } = s {
                    out.push(function.index());
                }
            });
            out
        })
        .collect()
}
