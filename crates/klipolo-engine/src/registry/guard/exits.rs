//! How control leaves early (for the costly settings): the conditions that end a loop, and whether a block leaves the
//! function, its loop or the rest of the loop's step.

use naga::{Block, Function, Handle, Statement as S};

use super::flow::Ex;

/// The conditions that end the loop whose body is `b` (its opening test; an `if` with a `break` or `return` in an
/// arm; a `switch` with a case that returns), each with the value that keeps the loop going (`None`: either value
/// may end it).
pub(super) fn exits(b: &Block, out: &mut Vec<(Ex, Option<bool>)>) {
    for s in b.iter() {
        match s {
            S::If {
                condition,
                accept,
                reject,
            } => {
                match (leaves(accept, true, false), leaves(reject, true, false)) {
                    (true, false) => out.push((*condition, Some(false))),
                    (false, true) => out.push((*condition, Some(true))),
                    (true, true) => out.push((*condition, None)),
                    (false, false) => {}
                }
                exits(accept, out);
                exits(reject, out);
            }
            S::Switch { selector, cases } => {
                if cases.iter().any(|c| leaves(&c.body, false, false)) {
                    out.push((*selector, None));
                }
            }
            S::Block(x) => exits(x, out),
            _ => {}
        }
    }
}

/// The block can leave: the function (`return`, `discard`) always; the loop it's in with `brk` (`break`); the rest of
/// the loop's step with `cont` (`continue`). Nested loops and switches keep their own `break`.
pub(super) fn leaves(b: &Block, brk: bool, cont: bool) -> bool {
    b.iter().any(|s| match s {
        S::Return { .. } | S::Kill => true,
        S::Break => brk,
        S::Continue => cont,
        S::Block(x) => leaves(x, brk, cont),
        S::If { accept, reject, .. } => leaves(accept, brk, cont) || leaves(reject, brk, cont),
        S::Switch { cases, .. } => cases.iter().any(|c| leaves(&c.body, false, cont)),
        S::Loop {
            body, continuing, ..
        } => leaves(body, false, false) || leaves(continuing, false, false),
        _ => false,
    })
}

/// The block calls a function that may `discard`.
pub(super) fn discards(b: &Block, may: &dyn Fn(Handle<Function>) -> bool) -> bool {
    b.iter().any(|s| match s {
        S::Call { function, .. } => may(*function),
        S::Block(x) => discards(x, may),
        S::If { accept, reject, .. } => discards(accept, may) || discards(reject, may),
        S::Switch { cases, .. } => cases.iter().any(|c| discards(&c.body, may)),
        S::Loop {
            body, continuing, ..
        } => discards(body, may) || discards(continuing, may),
        _ => false,
    })
}
