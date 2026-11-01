//! What a definition read from a folder must already be. Its names were cleaned when it was saved (the loader checks
//! and refuses, it never cleans silently), and its settings' text holds no hidden characters. And it is within the
//! definition caps, refused plainly beside the padding's check: a table setting has 1 to 4 number columns and at most
//! 50 rows, and its default rows fit both; a padding is finite, 0 or more, at most `MAX_PADDING_PX`, and grows with a
//! number setting; every setting's condition names a real choice.

use klipolo_model::contrib::{ContribDef, MAX_PADDING_PX};
use klipolo_model::font::family_ok;
use klipolo_model::params::{MAX_ROW_LABEL, MAX_TABLE_COLUMNS, MAX_TABLE_ROWS, ParamKind};
use klipolo_model::text_safety::{
    DESCRIPTION_CHARS, MADE_BY_CHARS, NAME_CHARS, clean, clean_tags, hidden,
};

use super::quoted;

/// Whether the definition's name, description, tags and provenance are as cleaning leaves them; else which isn't.
pub fn names_clean(def: &ContribDef) -> Result<(), &'static str> {
    if def.name != clean(&def.name, NAME_CHARS) {
        return Err("name");
    }
    if def.description != clean(&def.description, DESCRIPTION_CHARS) {
        return Err("description");
    }
    if def.tags != clean_tags(&def.tags) {
        return Err("tags");
    }
    if let Some(p) = &def.provenance
        && (p.made_by != clean(&p.made_by, MADE_BY_CHARS)
            || p.license != clean(&p.license, MADE_BY_CHARS)
            || p.based_on.as_ref().is_some_and(|b| !b.is_well_formed()))
    {
        return Err("provenance");
    }
    Ok(())
}

/// A setting's key, and a condition's, at most (the worker tames its setting names to this many characters).
pub const MAX_KEY_CHARS: usize = 64;

/// Whether the text of its settings an agent wrote (each key and label, a condition's key and values, a choice's
/// options and default, a table's columns and default rows' labels, a font's default family) shows as written: no
/// control, invisible, direction or line-separator character, as `text_safety::hidden` reads them, and a font family
/// `family_ok`. They are refused, never cleaned: settings are in the key. A text setting's default is the person's
/// content and may hold direction marks. Else the setting's key, quoted. The one predicate for the loader and the Make
/// tools, so create and save agree.
pub fn setting_text_clean(def: &ContribDef) -> Result<(), String> {
    for spec in &def.params {
        let condition = spec.visible_when.iter().flat_map(|v| {
            std::iter::once(v.key.as_str()).chain(v.values.iter().map(String::as_str))
        });
        let own: Vec<&str> = match &spec.kind {
            ParamKind::Choice { options, default } => options
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(default.as_str()))
                .collect(),
            ParamKind::Table {
                columns, default, ..
            } => columns
                .iter()
                .map(String::as_str)
                .chain(default.iter().map(|row| row.label.as_str()))
                .collect(),
            ParamKind::Font { default } => vec![default.family.as_str()],
            _ => Vec::new(),
        };
        let font_ok = match &spec.kind {
            ParamKind::Font { default } => family_ok(&default.family),
            _ => true,
        };
        let mut texts = [spec.key.as_str(), spec.label.as_str()]
            .into_iter()
            .chain(condition)
            .chain(own);
        // Keys take the strict rule (a crafted project's definitions load here too):
        // nothing the worker's taming (`guard::shown`) would change, and at most 64 characters, so an envelope's keys
        // come back from the worker exactly as the definition names them.
        let mut keys = std::iter::once(spec.key.as_str())
            .chain(spec.visible_when.iter().map(|v| v.key.as_str()));
        let strict =
            |k: &str| k.chars().count() <= MAX_KEY_CHARS && crate::registry::guard::shown(k) == k;
        if !font_ok || texts.any(|t| hidden(t).is_some()) || !keys.all(strict) {
            return Err(quoted(&spec.key));
        }
    }
    Ok(())
}

/// The definition within the definition caps (see the module's doc), or why not, in plain English for the agent.
pub fn check_definition(def: &ContribDef) -> Result<(), String> {
    if let Some(pad) = &def.padding {
        if klipolo_model::validate::check_padding(def).is_err() || pad.max_px > MAX_PADDING_PX {
            return Err(format!(
                "`padding` takes finite numbers of 0 or more, `maxPx` at most {MAX_PADDING_PX}."
            ));
        }
        if let Some(key) = &pad.param
            && !matches!(
                def.param(key).map(|s| &s.kind),
                Some(ParamKind::Float { .. })
            )
        {
            return Err(format!(
                "`padding` grows with {}, which isn't a number setting.",
                quoted(key)
            ));
        }
    }
    for spec in &def.params {
        let ParamKind::Table {
            columns,
            max_rows,
            default,
        } = &spec.kind
        else {
            continue;
        };
        let key = quoted(&spec.key);
        if columns.is_empty() || columns.len() > MAX_TABLE_COLUMNS {
            return Err(format!(
                "The table setting {key} has {} columns: a table takes 1 to {MAX_TABLE_COLUMNS}.",
                columns.len()
            ));
        }
        if *max_rows > MAX_TABLE_ROWS {
            return Err(format!(
                "The table setting {key} allows {max_rows} rows: a table takes at most {MAX_TABLE_ROWS}."
            ));
        }
        let fits = default.len() <= *max_rows as usize
            && default.iter().all(|row| {
                row.values.len() == columns.len()
                    && row.values.iter().all(|v| v.is_finite())
                    && row.label.chars().count() <= MAX_ROW_LABEL
            });
        if !fits {
            return Err(format!(
                "The table setting {key}'s default rows don't fit its {} columns and {max_rows} rows.",
                columns.len()
            ));
        }
    }
    def.check_conditions().map_err(|e| {
        let shown = crate::registry::guard::shown(&e);
        shown.chars().take(240).collect()
    })
}
