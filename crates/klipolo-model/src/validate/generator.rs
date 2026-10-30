//! Generator layers: a source of a kind [`GENERATOR_SOURCES`] allows, settings checked against it, and a
//! piece inside its span (`timeline::piece_time`). And a definition's padding, which the loaders of definitions that
//! don't ship with Klipolo check.

use std::collections::BTreeMap;

use super::{check, check_contrib, check_params, GENERATOR_PARAMS};
use crate::contrib::{Catalog, ContribDef, ContribId, ContribKind, GENERATOR_SOURCES};
use crate::error::Result;
use crate::messages::code;
use crate::params::ParamValue;
use crate::project::Project;
use crate::time::{Us, MAX_TIME_US};

/// What a generator layer stores beside its kind.
pub(super) struct Generator<'a> {
    pub source: &'a ContribId,
    pub params: &'a BTreeMap<String, ParamValue>,
    pub offset_us: Us,
    pub span_us: Option<Us>,
}

/// A layer `duration` long. Its source is refused only when this build knows it as a kind a generator layer can't
/// show; with a span, the piece lies inside it.
pub(super) fn check_generator(p: &Project, g: Generator<'_>, duration: Us) -> Result<()> {
    let def =
        check_contrib(g.source, ContribKind::Generator).or_else(
            |refused| match Catalog::builtin().get(g.source) {
                Some(def) if GENERATOR_SOURCES.contains(&def.kind) => Ok(Some(def)),
                _ => Err(refused),
            },
        )?;
    check_params(p, def, g.params, &GENERATOR_PARAMS)?;
    // With no span the layer is the whole animation, so it starts at its beginning.
    let inside = g.offset_us.saturating_add(duration) <= g.span_us.unwrap_or(duration)
        && g.span_us.is_none_or(|span| span <= MAX_TIME_US);
    check(
        (0..=MAX_TIME_US).contains(&g.offset_us) && inside,
        code!("model.layer.generatorTime"),
    )
}

/// A definition's padding has `px`, `per_unit` and `max_px` finite and never negative. The library and
/// embedded-folder loaders call it before `Catalog::with`; `PaddingSpec::px_for` caps what it gives anyway.
pub fn check_padding(def: &ContribDef) -> Result<()> {
    let ok = def.padding.as_ref().is_none_or(|pad| {
        [pad.px, pad.per_unit, pad.max_px]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
    });
    check(ok, code!("model.contrib.padding"))
}
