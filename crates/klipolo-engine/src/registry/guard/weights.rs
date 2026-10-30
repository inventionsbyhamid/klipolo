//! What an operation costs: the scalars it reads or writes, so a `vec4` add counts 4, a copy
//! of an array of `mat4x4` counts every float, and a matrix product counts its multiply-adds (a 4×4 by 4×4 product is
//! 64).
//!
//! The calibration note: these count under their ALU cost on these GPUs, and
//! `STATIC_OPS_PER_MS`'s 5× margin (a fifth of the Intel Mac's peak) covers them today:
//! - integer `/` and `%`: tens of ALU operations each, counted 1;
//! - `determinant(mat4x4)`: counted 16 (its scalars), about 40 multiply-adds;
//! - `mix`, `smoothstep`, `normalize`, `cross`, `refract` and `pow`: 2–3× their count;
//! - the hardened text's own loop guard: about 4 operations a trip that the facts, read before
//!   hardening, don't count; at 4,096 trips about 16K a pixel, some 6.5 % of `MAX_OPS_PER_PIXEL`.
//!
//! Real numbers come from the [Mac] calibration (`tests/make_mac.rs`); the weights change only with a measurement that
//! shows the margin no longer holds.

use naga::valid::FunctionInfo;
use naga::{ArraySize, BinaryOperator as B, Expression as E, Function, Module, TypeInner};

use super::flow::Ex;
use super::work::HUGE;

/// Scalars each type holds, by type index, in one pass in arena order: a vector its components, a matrix its columns
/// × rows, an array its length × its element's, a struct the sum of its members'; saturating.
pub(super) fn type_scalars(m: &Module) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::with_capacity(m.types.len());
    for (_, ty) in m.types.iter() {
        let n = match ty.inner {
            TypeInner::Array {
                base,
                size: ArraySize::Constant(n),
                ..
            } => f64::from(n.get()) * out.get(base.index()).copied().unwrap_or(HUGE),
            TypeInner::Array { .. } | TypeInner::BindingArray { .. } => HUGE,
            TypeInner::Struct { ref members, .. } => members
                .iter()
                .map(|mem| out.get(mem.ty.index()).copied().unwrap_or(HUGE))
                .sum(),
            ref inner => shape(inner).count(),
        };
        out.push(n.clamp(1.0, HUGE));
    }
    out
}

#[derive(Clone, Copy)]
enum Shape {
    Scalar,
    Vector(f64),
    Matrix { columns: f64, rows: f64 },
}

impl Shape {
    fn count(self) -> f64 {
        match self {
            Shape::Scalar => 1.0,
            Shape::Vector(n) => n,
            Shape::Matrix { columns, rows } => columns * rows,
        }
    }
}

fn shape(inner: &TypeInner) -> Shape {
    match *inner {
        TypeInner::Vector { size, .. } => Shape::Vector(f64::from(size as u8)),
        TypeInner::Matrix { columns, rows, .. } => Shape::Matrix {
            columns: f64::from(columns as u8),
            rows: f64::from(rows as u8),
        },
        _ => Shape::Scalar,
    }
}

/// Weighs one function's expressions.
pub(super) struct Weights<'a> {
    pub m: &'a Module,
    pub f: &'a Function,
    pub fi: &'a FunctionInfo,
    /// `type_scalars`.
    pub scalars: &'a [f64],
}

impl Weights<'_> {
    /// Scalars a value of this expression's type holds (a pointer counts 1).
    pub(super) fn values(&self, h: Ex) -> f64 {
        let ty = &self.fi[h].ty;
        match ty.handle() {
            Some(t) => self.scalars.get(t.index()).copied().unwrap_or(HUGE),
            None => shape(ty.inner_with(&self.m.types)).count(),
        }
    }

    fn shape(&self, h: Ex) -> Shape {
        shape(self.fi[h].ty.inner_with(&self.m.types))
    }

    /// One evaluation of an emitted expression: its scalars and its operands', and a matrix product's multiply-adds.
    pub(super) fn of(&self, h: Ex) -> f64 {
        let v = |x: Ex| self.values(x);
        let n = match self.f.expressions[h] {
            E::Binary {
                op: B::Multiply,
                left,
                right,
            } => match (self.shape(left), self.shape(right)) {
                (
                    Shape::Matrix {
                        columns: k,
                        rows: n,
                    },
                    Shape::Matrix { columns: m, .. },
                ) => n * m * k,
                (
                    Shape::Matrix {
                        columns: k,
                        rows: n,
                    },
                    Shape::Vector(_),
                ) => n * k,
                (Shape::Vector(n), Shape::Matrix { columns: m, .. }) => n * m,
                _ => v(h).max(v(left)).max(v(right)),
            },
            E::Binary { left, right, .. } => v(h).max(v(left)).max(v(right)),
            E::Select {
                condition,
                accept,
                reject,
            } => v(h).max(v(condition)).max(v(accept)).max(v(reject)),
            E::Unary { expr: x, .. }
            | E::As { expr: x, .. }
            | E::Derivative { expr: x, .. }
            | E::Relational { argument: x, .. } => v(h).max(v(x)),
            E::Math {
                arg,
                arg1,
                arg2,
                arg3,
                ..
            } => [Some(arg), arg1, arg2, arg3]
                .into_iter()
                .flatten()
                .map(v)
                .fold(v(h), f64::max),
            _ => v(h),
        };
        n.clamp(1.0, HUGE)
    }
}
