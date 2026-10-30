//! The taint rule of one expression (for the costly settings): from its operands' taints, the local variables', and a
//! callee's summary for a call's result. Conservative where direction isn't certain:
//! integer arithmetic and casts between signed and unsigned wrap, so they keep no direction.

use std::collections::HashMap;

use naga::valid::FunctionInfo;
use naga::{
    BinaryOperator as B, Expression as E, Function, Handle, ImageQuery, MathFunction as M,
    SampleLevel, ScalarKind, TypeInner, UnaryOperator as U,
};

use super::analyze::Ctx;
use super::bounds::constant;
use super::flow::Ex;
use super::taint::{Sign, Source, Taint, add, join, keep, unknown};

/// `h` is `params` itself, or its value.
fn is_params(cx: &Ctx, f: &Function, h: Ex) -> bool {
    let global = |h: Ex| matches!(f.expressions[h], E::GlobalVariable(g) if Some(g) == cx.g.params);
    global(h) || matches!(f.expressions[h], E::Load { pointer } if global(pointer))
}

/// Rising with each argument over its whole domain. `sqrt`, `log` and `pow` aren't here: below 0 they give NaN, and a
/// NaN test ends a loop at once.
fn monotone(fun: M) -> bool {
    matches!(
        fun,
        M::Min
            | M::Max
            | M::Clamp
            | M::Saturate
            | M::Floor
            | M::Ceil
            | M::Round
            | M::Trunc
            | M::Exp
            | M::Exp2
    )
}

/// The function an expression belongs to, with what the taint rules read beside the taints.
pub(super) struct Site<'a> {
    pub cx: &'a Ctx<'a>,
    pub f: &'a Function,
    pub fi: &'a FunctionInfo,
    /// A call's result → (callee, arguments).
    pub calls: &'a HashMap<Ex, (Handle<Function>, &'a [Ex])>,
}

impl Site<'_> {
    /// The scalar kind of an expression's value, when it's a scalar or a vector.
    pub(super) fn kind(&self, h: Ex) -> Option<ScalarKind> {
        match *self.fi[h].ty.inner_with(&self.cx.m.types) {
            TypeInner::Scalar(s) | TypeInner::Vector { scalar: s, .. } => Some(s.kind),
            _ => None,
        }
    }

    fn integer(&self, h: Ex) -> bool {
        matches!(self.kind(h), Some(ScalarKind::Sint | ScalarKind::Uint))
    }
}

/// One expression's taint from its operands' (`expr`), the local variables' (`local`) and, for a call's result, the
/// callee's summary. A pointer carries what its variable holds, so a `Load` of it, or a call handed it, sees that.
pub(super) fn one(site: &Site, h: Ex, e: &E, expr: &[Taint], local: &[Taint]) -> Taint {
    let (cx, f, calls) = (site.cx, site.f, site.calls);
    let t = |x: Ex| expr[x.index()].as_slice();
    let mut out = Taint::new();
    match *e {
        E::FunctionArgument(i) => out.push((Source::Arg(i), Sign::Up)),
        E::LocalVariable(v) => {
            join(&mut out, &local[v.index()], keep);
        }
        E::AccessIndex { base, index } if is_params(cx, f, base) => {
            out.push((Source::Param(index), Sign::Up))
        }
        E::AccessIndex { base, .. }
        | E::Splat { value: base, .. }
        | E::Swizzle { vector: base, .. } => {
            join(&mut out, t(base), keep);
        }
        E::Access { base, index } => {
            join(&mut out, t(base), keep);
            join(&mut out, t(index), unknown);
        }
        E::Compose { ref components, .. } => {
            for c in components {
                join(&mut out, t(*c), keep);
            }
        }
        E::Load { pointer } => {
            join(&mut out, t(pointer), keep);
            if is_params(cx, f, pointer) {
                // The whole struct: any setting, read any way.
                for k in 0..cx.g.keys.len() {
                    add(&mut out, Source::Param(k as u32), Sign::Unknown);
                }
            }
        }
        E::ImageSample {
            coordinate,
            array_index,
            offset,
            level,
            depth_ref,
            ..
        } => {
            let level = match level {
                SampleLevel::Exact(x) | SampleLevel::Bias(x) => vec![x],
                SampleLevel::Gradient { x, y } => vec![x, y],
                SampleLevel::Auto | SampleLevel::Zero => vec![],
            };
            let operands = [Some(coordinate), array_index, offset, depth_ref];
            for x in operands.into_iter().flatten().chain(level) {
                join(&mut out, t(x), unknown);
            }
        }
        E::ImageLoad {
            coordinate,
            array_index,
            sample,
            level,
            ..
        } => {
            for x in [Some(coordinate), array_index, sample, level]
                .into_iter()
                .flatten()
            {
                join(&mut out, t(x), unknown);
            }
        }
        E::ImageQuery { image, query } => {
            join(&mut out, t(image), unknown);
            if let ImageQuery::Size { level: Some(x) } = query {
                join(&mut out, t(x), unknown);
            }
        }
        E::Unary { op, expr: x } => {
            // `-i32::MIN` wraps.
            let map = if op == U::Negate && site.integer(h) {
                unknown
            } else {
                Sign::flip
            };
            join(&mut out, t(x), map);
        }
        E::Binary { left, right, .. } if site.integer(h) => {
            // Integer arithmetic wraps: no direction survives it.
            join(&mut out, t(left), unknown);
            join(&mut out, t(right), unknown);
        }
        E::Binary { op, left, right } => {
            let lone = |x: Ex| constant(cx.m, f, x).filter(|_| t(x).is_empty());
            let scaled = |c: f64| {
                if c >= 0.0 {
                    keep as fn(Sign) -> Sign
                } else {
                    Sign::flip
                }
            };
            match op {
                B::Add | B::LogicalAnd | B::LogicalOr => {
                    join(&mut out, t(left), keep);
                    join(&mut out, t(right), keep);
                }
                B::Subtract => {
                    join(&mut out, t(left), keep);
                    join(&mut out, t(right), Sign::flip);
                }
                B::Multiply | B::Divide if lone(right).is_some_and(|c| c != 0.0) => {
                    join(&mut out, t(left), scaled(lone(right).unwrap_or(1.0)));
                }
                B::Multiply if lone(left).is_some_and(|c| c != 0.0) => {
                    join(&mut out, t(right), scaled(lone(left).unwrap_or(1.0)));
                }
                _ => {
                    join(&mut out, t(left), unknown);
                    join(&mut out, t(right), unknown);
                }
            }
        }
        E::Select {
            condition,
            accept,
            reject,
        } => {
            join(&mut out, t(accept), keep);
            join(&mut out, t(reject), keep);
            join(&mut out, t(condition), unknown);
        }
        E::Relational { argument, .. } | E::Derivative { expr: argument, .. } => {
            join(&mut out, t(argument), unknown);
        }
        E::Math {
            fun,
            arg,
            arg1,
            arg2,
            arg3,
        } => {
            let map = if monotone(fun) { keep } else { unknown };
            for a in [Some(arg), arg1, arg2, arg3].into_iter().flatten() {
                join(&mut out, t(a), map);
            }
        }
        E::As {
            expr: x,
            kind,
            convert,
        } => {
            // A bitcast, or `u32(i32)` / `i32(u32)`, which reinterpret the bits, keeps no direction; nor does `bool(x)`,
            // which is `x != 0`, from anything but a `u32` (a bool setting).
            let from = site.kind(x);
            let reinterprets = matches!(
                (from, kind),
                (Some(ScalarKind::Sint), ScalarKind::Uint)
                    | (Some(ScalarKind::Uint), ScalarKind::Sint)
            ) || (kind == ScalarKind::Bool && from != Some(ScalarKind::Uint));
            let map = if convert.is_some() && !reinterprets {
                keep
            } else {
                unknown
            };
            join(&mut out, t(x), map);
        }
        E::CallResult(_) => {
            let Some(&(callee, args)) = calls.get(&h) else {
                return out;
            };
            let Some(sum) = cx.sums[callee.index()].as_ref() else {
                return out;
            };
            for &(source, sign) in &sum.ret {
                match source {
                    Source::Param(_) => {
                        add(&mut out, source, sign);
                    }
                    Source::Arg(j) => {
                        for &(s, g) in args.get(j as usize).map_or(&[][..], |a| t(*a)) {
                            add(&mut out, s, sign.of(g));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}
