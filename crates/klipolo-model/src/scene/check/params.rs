//! A scene's own settings: at most 16, each a name bindings can use (`$title`), of a kind scenes take, with
//! sane bounds and a default inside them.

use std::collections::BTreeMap;

use super::super::problems::{at, join, Problems};
use super::super::{Scene, SceneLimits};
use super::is_name;
use crate::contrib::{ContribDef, ContribKind};
use crate::font::font_ok;
use crate::params::{ParamKind, ParamSpec, MAX_ROW_LABEL, MAX_TABLE_COLUMNS, MAX_TABLE_ROWS};
use crate::project::AssetKind;
use crate::validate::valid_color;

/// The params by key (the first of a repeated key), with a problem for each one that can't be used.
pub(super) fn check<'a>(scene: &'a Scene, out: &mut Problems) -> BTreeMap<&'a str, &'a ParamSpec> {
    let mut by_key = BTreeMap::new();
    if scene.params.len() > SceneLimits::MAX_PARAMS {
        out.push(
            "/params",
            "scene.params",
            format!(
                "A scene has at most {} settings; this one has {}.",
                SceneLimits::MAX_PARAMS,
                scene.params.len()
            ),
        );
    }
    for (i, spec) in scene.params.iter().enumerate() {
        let path = at("/params", i);
        let key = spec.key.as_str();
        if !is_name(key) || key.len() > 40 || key == "i" || key == "row" {
            out.push(
                join(&path, "key"),
                "scene.params",
                "A setting's key is a letter or _, then up to 39 letters, digits, _ or - (not i or row, which repeats \
                 use), so \"$key\" can bind it.",
            );
        } else if by_key.insert(key, spec).is_some() {
            out.push(
                join(&path, "key"),
                "scene.params",
                format!("Two settings have the key `{key}`."),
            );
        }
        let label = spec.label.chars().count();
        if label == 0 || label > 80 || spec.label.chars().any(char::is_control) {
            out.push(
                join(&path, "label"),
                "scene.params",
                "A setting's label is 1 to 80 characters on one line.",
            );
        }
        if let Err(why) = kind(&spec.kind) {
            out.push(path, "scene.params", why);
        }
    }
    let def = ContribDef {
        params: scene.params.clone(),
        ..ContribDef::new("klipolo.scene/scene.check", ContribKind::Scene, "scene")
    };
    if let Err(why) = def.check_conditions() {
        let why = why
            .strip_prefix("klipolo.scene/scene.check: ")
            .unwrap_or(&why);
        out.push("/params", "scene.params", why);
    }
    by_key
}

fn kind(kind: &ParamKind) -> Result<(), String> {
    let ok = |cond: bool, why: &str| if cond { Ok(()) } else { Err(why.to_owned()) };
    match kind {
        ParamKind::Float {
            min,
            max,
            default,
            step,
            ..
        } => {
            ok(
                [*min, *max, *default].iter().all(|v| v.is_finite()) && (*min..=*max).contains(default),
                "A number setting has finite min ≤ default ≤ max.",
            )?;
            ok(
                step.is_none_or(|s| s.is_finite() && s > 0.0),
                "A number setting's step is above 0.",
            )
        }
        ParamKind::Int { min, max, default } => ok(
            (*min..=*max).contains(default),
            "A whole-number setting has min ≤ default ≤ max.",
        ),
        ParamKind::Angle { default } => ok(
            default.is_finite() && (-360.0..=360.0).contains(default),
            "An angle setting's default is from -360 to 360 degrees.",
        ),
        ParamKind::Bool { .. } => Ok(()),
        ParamKind::Color { default, .. } => ok(
            valid_color(default),
            "A colour setting's default is #RRGGBB or #RRGGBBAA.",
        ),
        ParamKind::Choice { options, default } => ok(
            (1..=32).contains(&options.len())
                && options.iter().all(|o| (1..=80).contains(&o.chars().count()))
                && options.contains(default),
            "A choice setting has 1 to 32 options of 1 to 80 characters, its default one of them.",
        ),
        ParamKind::Text { default, max_len } => ok(
            (1..=SceneLimits::MAX_TEXT_NODE as u32).contains(max_len)
                && default.chars().count() <= *max_len as usize,
            "A text setting's maxLen is 1 to 500, and its default fits it.",
        ),
        ParamKind::Asset { kind } => ok(
            *kind == AssetKind::Image,
            "A scene's asset settings are images (\"kind\": \"image\").",
        ),
        ParamKind::Font { default } => ok(
            font_ok(default),
            "A font setting's default is a family of 1 to 100 characters on one line (a Google one with its \
             version, v31).",
        ),
        ParamKind::Table {
            columns,
            max_rows,
            default,
        } => ok(
            (1..=MAX_TABLE_COLUMNS).contains(&columns.len())
                && columns.iter().all(|c| (1..=20).contains(&c.chars().count()))
                && (1..=MAX_TABLE_ROWS).contains(max_rows)
                && default.len() <= *max_rows as usize
                && default.iter().all(|r| {
                    r.values.len() == columns.len()
                        && r.values.iter().all(|v| v.is_finite())
                        && r.label.chars().count() <= MAX_ROW_LABEL
                }),
            "A table setting has 1 to 4 columns, maxRows from 1 to 50, and default rows of a label (up to 40 \
             characters) and one finite number per column.",
        ),
        ParamKind::Point { .. } => Err(
            "Scenes don't take point settings: use two number settings for x and y.".to_owned(),
        ),
    }
}
