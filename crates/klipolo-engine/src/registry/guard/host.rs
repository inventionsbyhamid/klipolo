//! The host's code calls only the host's functions. naga resolves a call to the module's own
//! function before WGSL's, so a body's `fn max(a: vec3<f32>, b: vec3<f32>) -> vec3<f32>` would replace the one the
//! prelude's `kl_to_linear` calls, and a `fn min` the one the output check calls. Only `kl_fs` calls into the
//! body, and only the kind's entry function. Runs after `rules`, so every entry point is the host's.

use naga::{Function, Handle, Module, Statement};

use super::rules::entry_name;
use super::{Diagnostic, Placed, each_statement};
use crate::registry::shader::ShaderKind;

pub(super) fn check(p: &Placed, kind: ShaderKind, m: &Module) -> Vec<Diagnostic> {
    let in_body = |h: Handle<Function>| p.offset(m.functions.get_span(h));
    let callers = m
        .functions
        .iter()
        .filter(|(h, _)| in_body(*h).is_none())
        .map(|(_, f)| (f, false))
        .chain(
            m.entry_points
                .iter()
                .map(|ep| (&ep.function, ep.name == "kl_fs")),
        );
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for (f, is_fs) in callers {
        each_statement(&f.body, &mut |s| {
            let Statement::Call { function, .. } = *s else {
                return;
            };
            let Some(at) = in_body(function) else {
                return;
            };
            let name = m.functions[function]
                .name
                .as_deref()
                .unwrap_or("this function");
            if (is_fs && name == entry_name(kind)) || seen.contains(&function) {
                return;
            }
            seen.push(function);
            out.push(
                Diagnostic::error(
                    "rule.shadow",
                    format!("`{name}` replaces WGSL's own `{name}`, which the host's code calls."),
                )
                .at(p.body, at)
                .hint(format!("Rename the function, for example to `my_{name}`.")),
            );
        });
    }
    out
}
