//! Which end of a setting's range makes a condition come out a given way: `i < params.steps` keeps a
//! loop going at the high end, `params.detail > 0.5` takes its branch at the high end, a bool compared with a
//! literal at `true` or `false`; anything else is costly at both ends. Each (condition, wanted value) is read once and
//! remembered, and `&&`, `||` and `!` are followed at most `MAX_DEPTH` deep (a `let` can make a condition a
//! DAG that doubles at every step).

use naga::{BinaryOperator as B, Expression as E, Function, ScalarKind, UnaryOperator as U};

use super::End;
use super::analyze::Ctx;
use super::bounds::constant;
use super::flow::{Ex, Flow};
use super::globals::Class;
use super::taint::{Sign, Source, through};

/// `&&`, `||` and `!` followed this deep at most; past it, a condition is costly at both ends.
const MAX_DEPTH: usize = 64;

impl Flow {
    /// The sources whose value makes the condition `h` come out `want`, each with the end of its range that does.
    pub(super) fn costly(
        &self,
        cx: &Ctx,
        f: &Function,
        h: Ex,
        want: bool,
        out: &mut Vec<(Source, End)>,
    ) {
        out.extend(self.read(cx, f, h, want, 0));
    }

    fn read(&self, cx: &Ctx, f: &Function, h: Ex, want: bool, depth: usize) -> Vec<(Source, End)> {
        if let Some(found) = self.memo.borrow().get(&(h, want)) {
            return found.clone();
        }
        let mut out = Vec::new();
        self.read_new(cx, f, h, want, depth, &mut out);
        let out = merged(out);
        self.memo.borrow_mut().insert((h, want), out.clone());
        out
    }

    fn read_new(
        &self,
        cx: &Ctx,
        f: &Function,
        h: Ex,
        want: bool,
        depth: usize,
        out: &mut Vec<(Source, End)>,
    ) {
        let t = |x: Ex| self.expr[x.index()].as_slice();
        let deeper = depth < MAX_DEPTH;
        match f.expressions[h] {
            E::Binary {
                op: B::Less | B::LessEqual,
                left,
                right,
            } => {
                ends(out, t(right), want);
                ends(out, t(left), !want);
            }
            E::Binary {
                op: B::Greater | B::GreaterEqual,
                left,
                right,
            } => {
                ends(out, t(left), want);
                ends(out, t(right), !want);
            }
            E::Binary {
                op: op @ (B::Equal | B::NotEqual),
                left,
                right,
            } => {
                let lone = |x: Ex| constant(cx.m, f, x).filter(|_| t(x).is_empty());
                let test = lone(right)
                    .map(|c| (c, left))
                    .or_else(|| lone(left).map(|c| (c, right)));
                match test.and_then(|(c, side)| self.bool_setting(cx, side).map(|k| (c, k))) {
                    Some((c, k)) => {
                        // The setting's `true` makes the test true.
                        let truthy = (c != 0.0) == (op == B::Equal);
                        out.push((
                            Source::Param(k),
                            if truthy == want { End::Max } else { End::Min },
                        ));
                    }
                    None => {
                        both(out, t(left));
                        both(out, t(right));
                    }
                }
            }
            E::Binary {
                op: B::LogicalAnd | B::LogicalOr,
                left,
                right,
            } if deeper => {
                out.extend(self.read(cx, f, left, want, depth + 1));
                out.extend(self.read(cx, f, right, want, depth + 1));
            }
            E::Unary {
                op: U::LogicalNot,
                expr,
            } if deeper => out.extend(self.read(cx, f, expr, !want, depth + 1)),
            E::As {
                expr,
                kind: ScalarKind::Bool,
                ..
            } => match self.bool_setting(cx, expr) {
                Some(k) => out.push((Source::Param(k), if want { End::Max } else { End::Min })),
                None => both(out, t(expr)),
            },
            _ => both(out, t(h)),
        }
    }

    /// `h` is a bool setting's own value: its member index.
    fn bool_setting(&self, cx: &Ctx, h: Ex) -> Option<u32> {
        match self.expr[h.index()].as_slice() {
            &[(Source::Param(k), Sign::Up)]
                if cx
                    .g
                    .keys
                    .get(k as usize)
                    .is_some_and(|(_, c)| *c == Class::Bool) =>
            {
                Some(k)
            }
            _ => None,
        }
    }
}

/// `up_wins`: a rising source makes the condition what's wanted.
fn ends(out: &mut Vec<(Source, End)>, taint: &[(Source, Sign)], up_wins: bool) {
    for &(s, g) in taint {
        out.push((s, through(if up_wins { End::Max } else { End::Min }, g)));
    }
}

pub(super) fn both(out: &mut Vec<(Source, End)>, taint: &[(Source, Sign)]) {
    out.extend(taint.iter().map(|&(s, _)| (s, End::Both)));
}

/// One entry per source, its ends joined: at most a function's settings and arguments, however many places reach them.
pub(super) fn merged(mut found: Vec<(Source, End)>) -> Vec<(Source, End)> {
    found.sort_by_key(|&(s, _)| s);
    let mut out: Vec<(Source, End)> = Vec::with_capacity(found.len());
    for (s, e) in found {
        match out.last_mut() {
            Some(last) if last.0 == s => last.1 = last.1.join(e),
            _ => out.push((s, e)),
        }
    }
    out
}
