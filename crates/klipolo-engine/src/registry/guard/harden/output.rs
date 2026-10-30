//! The host's output check (`SANITIZE`) as the re-emitted module reads it: `kl_fs` returns `kl_sanitize`
//! of the entry function's result, and `kl_sanitize` and `kl_finite` call nothing but `kl_finite`. A body's function
//! that would replace a WGSL built-in they call is refused before this (`guard::host`, `rule.shadow`); this is the
//! round trip's own check that the text a device compiles still sanitises (fail closed).

use naga::{Expression, Module, Statement};

use crate::registry::guard::each_statement;

pub(super) fn intact(m: &Module) -> Result<(), String> {
    let find = |name: &str| {
        m.functions
            .iter()
            .find(|(_, f)| f.name.as_deref() == Some(name))
            .map(|(h, _)| h)
    };
    let (Some(sanitize), Some(finite)) = (find("kl_sanitize"), find("kl_finite")) else {
        return Err("its output check is missing".into());
    };
    let mut foreign = None;
    for h in [sanitize, finite] {
        each_statement(&m.functions[h].body, &mut |s| {
            if let Statement::Call { function, .. } = *s
                && !(h == sanitize && function == finite)
            {
                foreign.get_or_insert(function);
            }
        });
    }
    if let Some(h) = foreign {
        let name = m.functions[h].name.as_deref().unwrap_or("a function");
        return Err(format!("its output check calls `{name}`"));
    }
    let fs = m
        .entry_points
        .iter()
        .find(|ep| ep.name == "kl_fs")
        .ok_or("it has no `kl_fs`")?;
    let f = &fs.function;
    let (mut returns, mut through) = (0, 0);
    each_statement(&f.body, &mut |s| {
        if let Statement::Return { value } = *s {
            returns += 1;
            through += usize::from(value.is_some_and(
                |v| matches!(f.expressions[v], Expression::CallResult(c) if c == sanitize),
            ));
        }
    });
    if returns == 0 || through != returns {
        return Err("`kl_fs` no longer returns through the output check".into());
    }
    Ok(())
}
