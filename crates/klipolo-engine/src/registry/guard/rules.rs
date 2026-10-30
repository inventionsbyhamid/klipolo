//! The module rules on naga's IR, before validation: the body declares no globals but `const`s, no bindings,
//! entry points, overrides or atomics, no `kl` names; it has the kind's entry function; and it stays inside the size
//! caps, including no array or struct type of more than `MAX_CONST_ELEMENTS` values anywhere in the module
//! and none nested more than `MAX_NESTING` deep. Only items whose span starts inside the body are
//! the author's: the prelude, `Params` and the host's entry aren't. Every rule is linear in the module, and at most
//! `MAX_REPORTED` diagnostics are built.

use naga::{AddressSpace, Handle, Module, ScalarKind, Type, TypeInner, VectorSize};

use super::scan::tokens;
use super::types::{type_depths, type_sizes};
use super::{
    Diagnostic, MAX_CONST_ELEMENTS, MAX_EXPRESSIONS, MAX_FUNCTIONS, MAX_NESTING, MAX_REPORTED,
    Placed,
};
use crate::registry::shader::ShaderKind;

/// The host's entry points (`prelude.wgsl`, `ShaderEffect::assemble`).
const HOST_ENTRY_POINTS: [&str; 2] = ["kl_vs", "kl_fs"];

pub(super) fn check(p: &Placed, kind: ShaderKind, m: &Module) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let room = |out: &Vec<Diagnostic>| out.len() < MAX_REPORTED;
    for (h, g) in m.global_variables.iter() {
        let Some(at) = p.offset(m.global_variables.get_span(h)) else {
            continue;
        };
        if !room(&out) {
            break;
        }
        let name = g.name.as_deref().unwrap_or("this variable");
        out.push(if g.binding.is_some() {
            Diagnostic::error(
                "rule.binding",
                format!("`{name}` has its own `@group`/`@binding`: the host owns every binding."),
            )
            .at(p.body, at)
            .hint(
                "Read the layer with `kl_sample_a(uv)`, settings from `params` and time from `kl`.",
            )
        } else {
            let space = match g.space {
                AddressSpace::Private => "`var<private>`",
                AddressSpace::Uniform => "`var<uniform>`",
                AddressSpace::Storage { .. } => "`var<storage>`",
                AddressSpace::WorkGroup => "`var<workgroup>`",
                AddressSpace::Handle => "texture or sampler",
                _ => "variable",
            };
            Diagnostic::error("rule.global", format!("`{name}` is a global {space}."))
                .at(p.body, at)
                .hint("Declare no globals: use `const`, a function's `var`, or `params`.")
        });
    }
    for ep in m
        .entry_points
        .iter()
        .filter(|ep| !HOST_ENTRY_POINTS.contains(&ep.name.as_str()))
        .take(MAX_REPORTED)
    {
        let at = ep
            .function
            .body
            .span_iter()
            .find_map(|(_, span)| p.offset(*span));
        let d = Diagnostic::error(
            "rule.entry",
            format!(
                "`{}` is an entry point (`@{}`): the host adds those.",
                ep.name,
                format!("{:?}", ep.stage).to_lowercase()
            ),
        )
        .hint(format!(
            "Remove the attribute and write `{}`.",
            signature(kind)
        ));
        out.push(match at {
            Some(at) => d.at(p.body, at),
            None => d,
        });
    }
    for (h, o) in m.overrides.iter().take(MAX_REPORTED) {
        if let Some(at) = p.offset(m.overrides.get_span(h)) {
            let name = o.name.as_deref().unwrap_or("this constant");
            out.push(
                Diagnostic::error("rule.override", format!("`{name}` is an `override`."))
                    .at(p.body, at)
                    .hint("Settings arrive in `params`, generated from your schema; use `const` for fixed values."),
            );
        }
    }
    let sizes = type_sizes(m);
    // Any type holding an atomic, wherever naga put it (an `atomic<u32>` written as a member has no span of its own);
    // the prelude has none. In arena order, a type's parts come first.
    let mut atomic = vec![false; m.types.len()];
    for (h, ty) in m.types.iter() {
        let of = |t: Handle<Type>| atomic.get(t.index()).copied().unwrap_or(false);
        let holds = match ty.inner {
            TypeInner::Atomic(_) => true,
            TypeInner::Array { base, .. } | TypeInner::BindingArray { base, .. } => of(base),
            TypeInner::Struct { ref members, .. } => members.iter().any(|mem| of(mem.ty)),
            _ => false,
        };
        atomic[h.index()] = holds;
    }
    if let Some((h, _)) = m.types.iter().find(|(h, _)| atomic[h.index()]) {
        let d = Diagnostic::error(
            "rule.atomic",
            "Atomics aren't allowed: a pixel shares nothing with another.",
        );
        out.push(match p.offset(m.types.get_span(h)) {
            Some(at) => d.at(p.body, at),
            None => d,
        });
    }
    if let Some((h, &n)) = m
        .types
        .iter()
        .zip(&sizes)
        .map(|((h, _), n)| (h, n))
        .find(|&(_, &n)| n > MAX_CONST_ELEMENTS)
    {
        let d = size(format!(
            "A type holds {} numbers; at most {MAX_CONST_ELEMENTS} are allowed in one array or struct.",
            if n == u64::MAX {
                "too many".to_string()
            } else {
                n.to_string()
            }
        ));
        out.push(match p.offset(m.types.get_span(h)) {
            Some(at) => d.at(p.body, at),
            None => d,
        });
    }
    // The writer and the lowerer recurse once per level of a type, and an `alias` chain nests without a bracket.
    if let Some((h, &depth)) = m
        .types
        .iter()
        .zip(&type_depths(m))
        .map(|((h, _), d)| (h, d))
        .find(|&(_, &d)| d > MAX_NESTING)
    {
        let d = Diagnostic::error(
            "rule.nesting",
            format!("A type nests {depth} deep; at most {MAX_NESTING} levels are allowed."),
        )
        .hint("Nest fewer arrays and structs inside one another, directly or through `alias`es.");
        out.push(match p.offset(m.types.get_span(h)) {
            Some(at) => d.at(p.body, at),
            None => d,
        });
    }
    reserved(p, &mut out);
    let entry_name = entry_name(kind);
    let body_fns: Vec<_> = m
        .functions
        .iter()
        .filter(|(h, _)| p.offset(m.functions.get_span(*h)).is_some())
        .collect();
    match body_fns
        .iter()
        .find(|(_, f)| f.name.as_deref() == Some(entry_name))
    {
        None => out.push(entry(p.body, kind, None)),
        Some((_, f)) => {
            let vec_of = |ty, size| matches!(m.types[ty].inner, TypeInner::Vector { size: s, scalar } if s == size && scalar.kind == ScalarKind::Float && scalar.width == 4);
            let fits = f.arguments.len() == 1
                && vec_of(f.arguments[0].ty, VectorSize::Bi)
                && f.arguments[0].binding.is_none()
                && f.result
                    .as_ref()
                    .is_some_and(|r| vec_of(r.ty, VectorSize::Quad) && r.binding.is_none());
            if !fits {
                out.push(entry(p.body, kind, None));
            }
        }
    }
    let expressions: usize = body_fns.iter().map(|(_, f)| f.expressions.len()).sum();
    if body_fns.len() > MAX_FUNCTIONS {
        out.push(size(format!(
            "The shader has {} functions; at most {MAX_FUNCTIONS} are allowed.",
            body_fns.len()
        )));
    }
    if expressions > MAX_EXPRESSIONS {
        out.push(size(format!("The shader compiles to {expressions} expressions; at most {MAX_EXPRESSIONS} are allowed.")));
    }
    let elements: u64 = m
        .constants
        .iter()
        .filter(|(h, c)| {
            p.offset(m.constants.get_span(*h)).is_some()
                && matches!(
                    m.types[c.ty].inner,
                    TypeInner::Array { .. } | TypeInner::Struct { .. }
                )
        })
        .map(|(_, c)| sizes[c.ty.index()])
        .fold(0, u64::saturating_add);
    if elements > MAX_CONST_ELEMENTS {
        out.push(size(format!(
            "The shader's constant arrays hold {elements} numbers; at most {MAX_CONST_ELEMENTS} are allowed."
        )));
    }
    out.sort_by_key(|d| (d.line, d.column));
    out.truncate(MAX_REPORTED);
    out
}

pub(super) fn entry_name(kind: ShaderKind) -> &'static str {
    match kind {
        ShaderKind::Filter | ShaderKind::Generator => "effect",
        ShaderKind::Transition => "transition",
    }
}

fn signature(kind: ShaderKind) -> String {
    format!("fn {}(uv: vec2<f32>) -> vec4<f32>", entry_name(kind))
}

/// The kind's entry function is missing or has another signature; `detail` is naga's word for it. Points at the
/// body's `fn effect` when there is one.
pub(super) fn entry(body: &str, kind: ShaderKind, detail: Option<&str>) -> Diagnostic {
    let sig = signature(kind);
    let message = match detail {
        Some(detail) => format!("The entry function must be `{sig}` ({detail})."),
        None => format!("The entry function must be `{sig}`."),
    };
    let d = Diagnostic::error("rule.entry", message)
        .hint(format!("Define it once, with one `vec2<f32>` argument (uv, 0–1 from the top left), returning premultiplied RGBA: `{sig} {{ … }}`."));
    let name = entry_name(kind);
    let toks = tokens(body);
    let at = toks
        .windows(2)
        .find(|w| w[0].1 == "fn" && w[1].1 == name)
        .map(|w| w[0].0);
    match at {
        Some(at) => d.at(body, at),
        None => d,
    }
}

fn size(message: String) -> Diagnostic {
    Diagnostic::error("rule.size", message)
        .hint("Make it smaller: fewer functions, shorter arrays and constant tables.")
}

/// Names the body declares that start with `kl` in any case (the host's): after `fn`, `let`, `var` (and its
/// `<…>`), `const`, `override`, `struct` or `alias`, or before a `:` (arguments, members, typed bindings). Read from
/// the text, since naga keeps no trace of a function's `const`s or of aliases.
fn reserved(p: &Placed, out: &mut Vec<Diagnostic>) {
    const DECLARE: [&str; 7] = ["fn", "let", "var", "const", "override", "struct", "alias"];
    let toks = tokens(p.body);
    for (i, &(at, word)) in toks.iter().enumerate() {
        if !word.get(..2).is_some_and(|s| s.eq_ignore_ascii_case("kl")) {
            continue;
        }
        if out.len() >= MAX_REPORTED {
            break;
        }
        let prev = |k: usize| i.checked_sub(k).map(|j| toks[j].1);
        let after_keyword = prev(1).is_some_and(|w| DECLARE.contains(&w))
            || (prev(1) == Some(">") && var_template(&toks[..i]));
        let before_colon = toks.get(i + 1).is_some_and(|t| t.1 == ":");
        if after_keyword || before_colon {
            out.push(
                Diagnostic::error(
                    "rule.reserved",
                    format!("`{word}` starts with `kl`: those names are the host's."),
                )
                .at(p.body, at)
                .hint("Rename it, for example without the `kl` prefix."),
            );
        }
    }
}

/// `toks` ends with `var<…>` (a template of a few words: only the last 8 tokens are looked at).
fn var_template(toks: &[(usize, &str)]) -> bool {
    let toks = &toks[toks.len().saturating_sub(8)..];
    let Some(open) = toks.iter().rposition(|t| t.1 == "<") else {
        return false;
    };
    open > 0
        && toks[open - 1].1 == "var"
        && toks[open + 1..toks.len() - 1]
            .iter()
            .all(|t| t.1 != ">" && t.1 != ";")
}
