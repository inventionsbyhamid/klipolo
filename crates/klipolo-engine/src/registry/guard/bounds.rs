//! Constant loop bounds. A loop's trips are read from the IR only when the loop provably ends after them: it opens
//! with naga's lowered `for` test (`if (i < N) {} else { break; }`, or the `break` arm first), evaluated afresh at the
//! top of every trip (a test or a load of `i` hoisted out of the loop, as `let first = i;`, never changes); `i` starts
//! at a constant (stored just before the loop, or its declaration's value when nothing else sets it); only its
//! `continuing` steps it, by a constant read there, towards `N`; and its last value fits its integer type (a `u32`
//! counting down to 0 never ends). Float counters are never read: a step of half a spacing can round back to where it
//! was. Anything else is `None`, counted as `UNKNOWN_LOOP_BOUND`.

use naga::{
    BinaryOperator as B, Block, Expression as E, Function, Handle, Literal, Module, ScalarKind,
    Statement as S, TypeInner, UnaryOperator,
};

pub(super) fn trips(m: &Module, f: &Function, block: &Block, at: usize) -> Option<u32> {
    let S::Loop {
        body, continuing, ..
    } = &block[at]
    else {
        return None;
    };
    let (lead, cond, exit_when_true) = opening(body)?;
    let E::Binary { op, left, right } = f.expressions[cond] else {
        return None;
    };
    let (v, load, op, bound) = match (counter(f, left), constant(m, f, right)) {
        (Some(v), Some(b)) => (v, left, op, b),
        _ => (
            counter(f, right)?,
            right,
            mirror(op)?,
            constant(m, f, left)?,
        ),
    };
    if !emitted(lead, cond) || !emitted(lead, load) {
        return None;
    }
    // The test as "go on while".
    let op = if exit_when_true { negate(op)? } else { op };
    let step = step(m, f, continuing, v)?;
    if writes(f, body, v) != 0 {
        return None;
    }
    let start = start(m, f, &block[..at], v)?;
    let TypeInner::Scalar(scalar) = m.types[f.local_variables[v].ty].inner else {
        return None;
    };
    count(start, bound, step, op, scalar.kind)
}

/// The loop's first statement after its `Emit`s (returned too): `if (c) {} else { break; }` gives (c, false);
/// `if (c) { break; }` gives (c, true).
pub(super) fn opening(body: &Block) -> Option<(&[S], Handle<naga::Expression>, bool)> {
    let at = body.iter().position(|s| !matches!(s, S::Emit(_)))?;
    let S::If {
        condition,
        accept,
        reject,
    } = &body[at]
    else {
        return None;
    };
    let lead = &body[..at];
    let only_break = |b: &Block| {
        let mut it = b.iter().filter(|s| !matches!(s, S::Emit(_)));
        matches!(it.next(), Some(S::Break)) && it.next().is_none()
    };
    let empty = |b: &Block| b.iter().all(|s| matches!(s, S::Emit(_)));
    match (only_break(accept), only_break(reject)) {
        (true, false) if empty(reject) => Some((lead, *condition, true)),
        (false, true) if empty(accept) => Some((lead, *condition, false)),
        _ => None,
    }
}

/// `h` is evaluated by one of these `Emit`s.
fn emitted(stmts: &[S], h: Handle<naga::Expression>) -> bool {
    stmts
        .iter()
        .any(|s| matches!(s, S::Emit(r) if r.clone().any(|x| x == h)))
}

/// `Load` of a local variable, itself (not a part of it).
fn counter(f: &Function, h: Handle<naga::Expression>) -> Option<Handle<naga::LocalVariable>> {
    let E::Load { pointer } = f.expressions[h] else {
        return None;
    };
    match f.expressions[pointer] {
        E::LocalVariable(v) => Some(v),
        _ => None,
    }
}

/// A constant's value: a literal, a module `const`, a splat or a negation of one.
pub(super) fn constant(m: &Module, f: &Function, h: Handle<naga::Expression>) -> Option<f64> {
    match f.expressions[h] {
        E::Literal(l) => literal(l),
        E::Constant(c) => match m.global_expressions[m.constants[c].init] {
            E::Literal(l) => literal(l),
            _ => None,
        },
        E::Splat { value, .. } => constant(m, f, value),
        E::Unary {
            op: UnaryOperator::Negate,
            expr,
        } => constant(m, f, expr).map(|v| -v),
        _ => None,
    }
}

fn literal(l: Literal) -> Option<f64> {
    Some(match l {
        Literal::F64(v) | Literal::AbstractFloat(v) => v,
        Literal::F32(v) => f64::from(v),
        Literal::U32(v) => f64::from(v),
        Literal::I32(v) => f64::from(v),
        Literal::U64(v) => v as f64,
        Literal::I64(v) | Literal::AbstractInt(v) => v as f64,
        Literal::Bool(b) => f64::from(u8::from(b)),
        Literal::F16(_) => return None,
    })
}

/// The same test with its sides swapped.
fn mirror(op: B) -> Option<B> {
    Some(match op {
        B::Less => B::Greater,
        B::LessEqual => B::GreaterEqual,
        B::Greater => B::Less,
        B::GreaterEqual => B::LessEqual,
        _ => return None,
    })
}

fn negate(op: B) -> Option<B> {
    Some(match op {
        B::Less => B::GreaterEqual,
        B::LessEqual => B::Greater,
        B::Greater => B::LessEqual,
        B::GreaterEqual => B::Less,
        _ => return None,
    })
}

/// `continuing` is one store `v = v ± c` (`i++`, `i += 2`, `i = i - 1`), its sum and its load of `v` evaluated there
/// (not `let s = i; … i = s + 1`): the step, signed.
fn step(
    m: &Module,
    f: &Function,
    continuing: &Block,
    v: Handle<naga::LocalVariable>,
) -> Option<f64> {
    let at = continuing.iter().position(|s| !matches!(s, S::Emit(_)))?;
    if continuing[at + 1..]
        .iter()
        .any(|s| !matches!(s, S::Emit(_)))
    {
        return None;
    }
    let S::Store { pointer, value } = &continuing[at] else {
        return None;
    };
    if !matches!(f.expressions[*pointer], E::LocalVariable(x) if x == v) {
        return None;
    }
    let E::Binary { op, left, right } = f.expressions[*value] else {
        return None;
    };
    let lead = &continuing[..at];
    let read_here = |load| emitted(lead, *value) && emitted(lead, load);
    match op {
        B::Add if counter(f, left) == Some(v) && read_here(left) => constant(m, f, right),
        B::Add if counter(f, right) == Some(v) && read_here(right) => constant(m, f, left),
        B::Subtract if counter(f, left) == Some(v) && read_here(left) => {
            constant(m, f, right).map(|c| -c)
        }
        _ => None,
    }
}

/// Where `v` starts: a constant stored just before the loop, else its declaration's value when the loop's step is the
/// only store to it in the whole function.
fn start(m: &Module, f: &Function, before: &[S], v: Handle<naga::LocalVariable>) -> Option<f64> {
    if let Some(S::Store { pointer, value }) =
        before.iter().rev().find(|s| !matches!(s, S::Emit(_)))
        && matches!(f.expressions[*pointer], E::LocalVariable(x) if x == v)
    {
        return constant(m, f, *value);
    }
    let init = f.local_variables[v].init?;
    (writes(f, &f.body, v) == 1)
        .then(|| constant(m, f, init))
        .flatten()
}

/// The stores to `v` (or a part of it) in `block`, nested blocks included, counting a pointer to it passed to a call.
pub(super) fn writes(f: &Function, block: &Block, v: Handle<naga::LocalVariable>) -> usize {
    let mine = |h: Handle<naga::Expression>| super::flow::local_root(f, h) == Some(v);
    block
        .iter()
        .map(|s| match s {
            S::Store { pointer, .. } => usize::from(mine(*pointer)),
            S::Call { arguments, .. } => arguments.iter().filter(|a| mine(**a)).count(),
            S::Block(b) => writes(f, b, v),
            S::If { accept, reject, .. } => writes(f, accept, v) + writes(f, reject, v),
            S::Switch { cases, .. } => cases.iter().map(|c| writes(f, &c.body, v)).sum(),
            S::Loop {
                body, continuing, ..
            } => writes(f, body, v) + writes(f, continuing, v),
            _ => 0,
        })
        .sum()
}

fn count(start: f64, bound: f64, step: f64, op: B, kind: ScalarKind) -> Option<u32> {
    let span = |from: f64, to: f64| (to - from) / step.abs();
    let trips = match op {
        B::Less if step > 0.0 => span(start, bound).ceil().max(0.0),
        B::LessEqual if step > 0.0 => (span(start, bound).floor() + 1.0).max(0.0),
        B::Greater if step < 0.0 => span(bound, start).ceil().max(0.0),
        B::GreaterEqual if step < 0.0 => (span(bound, start).floor() + 1.0).max(0.0),
        _ => return None,
    };
    if !trips.is_finite() {
        return None;
    }
    // The value after the last step must exist, or the counter wraps and the loop goes on. Floats aren't read.
    let last = start + trips * step;
    let fits = match kind {
        ScalarKind::Sint => (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&last),
        ScalarKind::Uint => (0.0..=f64::from(u32::MAX)).contains(&last),
        _ => false,
    };
    fits.then(|| trips.min(f64::from(u32::MAX)) as u32)
}
