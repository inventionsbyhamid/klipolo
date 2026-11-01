//! What the module rules read from each type, in one pass in arena order, where a type's parts come before it: how
//! many scalars it holds and how deep it nests. A deep fan-out of structs or a long `alias` chain costs one step a
//! type.

use naga::{ArraySize, Handle, Module, Type, TypeInner};

/// How many scalars each type holds, by type index: a vector its components, a matrix its columns ×
/// rows, an array its length times its element's, a struct the sum of its members', anything else 1; saturating, and
/// `u64::MAX` for an array whose length isn't a constant. One pass in arena order, where a type's parts come before
/// it, so a deep fan-out of structs costs nothing.
pub(super) fn type_sizes(m: &Module) -> Vec<u64> {
    let mut sizes: Vec<u64> = Vec::with_capacity(m.types.len());
    for (_, ty) in m.types.iter() {
        let of = |t: Handle<Type>| sizes.get(t.index()).copied().unwrap_or(u64::MAX);
        let n = match ty.inner {
            TypeInner::Array {
                base,
                size: ArraySize::Constant(n),
                ..
            } => u64::from(n.get()).saturating_mul(of(base)),
            TypeInner::Array { .. } | TypeInner::BindingArray { .. } => u64::MAX,
            TypeInner::Struct { ref members, .. } => members
                .iter()
                .map(|mem| of(mem.ty))
                .fold(0, u64::saturating_add),
            TypeInner::Vector { size, .. } => u64::from(size as u8),
            TypeInner::Matrix { columns, rows, .. } => {
                u64::from(columns as u8) * u64::from(rows as u8)
            }
            _ => 1,
        };
        sizes.push(n);
    }
    sizes
}

/// How deep each type nests, by type index: a scalar 0; a vector, matrix, atomic, texture or sampler 1;
/// an array or a pointer one more than what it holds; a struct one more than its deepest member. Saturating.
pub(super) fn type_depths(m: &Module) -> Vec<usize> {
    let mut depths: Vec<usize> = Vec::with_capacity(m.types.len());
    for (_, ty) in m.types.iter() {
        let of = |t: Handle<Type>| depths.get(t.index()).copied().unwrap_or(usize::MAX);
        let d = match ty.inner {
            TypeInner::Scalar(_) => 0,
            TypeInner::Array { base, .. }
            | TypeInner::BindingArray { base, .. }
            | TypeInner::Pointer { base, .. } => of(base).saturating_add(1),
            TypeInner::Struct { ref members, .. } => members
                .iter()
                .map(|mem| of(mem.ty))
                .max()
                .unwrap_or(0)
                .saturating_add(1),
            _ => 1,
        };
        depths.push(d);
    }
    depths
}
