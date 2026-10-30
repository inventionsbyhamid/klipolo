//! Parameters of contributions. A contribution declares a schema ([`ParamSpec`]); a
//! project stores values ([`ParamValue`]) by key. Values are plain JSON (`12`, `true`, `"#00E5FF"`,
//! `{"x": 0.5, "y": 0.5}`) and the schema gives them meaning, so chat and plugins can write them easily. Keys a
//! schema doesn't know are kept, because a newer version of the effect may use them. Keyframes will arrive as one
//! more value shape (`{"keys": […]}`) without breaking these.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::coded::CodedError;
use crate::font::{font_ok, FontRef};
use crate::messages::msg;
use crate::project::{AssetKind, Project};
use crate::validate::valid_color;

/// Longest text value, in characters.
pub const MAX_TEXT_PARAM: usize = 2000;
/// The most rows a table setting holds (a chart's bars, a list's lines), whatever its definition says.
pub const MAX_TABLE_ROWS: u32 = 50;
/// The most numbers per row.
pub const MAX_TABLE_COLUMNS: usize = 4;
/// Longest row label, in characters.
pub const MAX_ROW_LABEL: usize = 40;

/// A point in 0 to 1 coordinates of the layer (or the canvas), from the top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct PointN {
    pub x: f64,
    pub y: f64,
}

/// A stored parameter value; its [`ParamSpec`] says what it means.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(untagged)]
#[ts(export)]
pub enum ParamValue {
    Bool(bool),
    Number(f64),
    /// Colours (`#RRGGBB`, `#RRGGBBAA`), choices, texts and asset ids.
    Text(String),
    Point(PointN),
    /// A font setting's face: a `FontRef` (`{"family": …, "source": {"type": …}}`), never read as a point. A
    /// bare family name is the tools' shorthand, resolved before it reaches the model.
    Font(FontRef),
    /// A table setting's rows. Last, so strings, numbers and points read as before.
    Rows(Vec<TableRow>),
}

/// One row of a table setting: a label and one number per column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct TableRow {
    pub label: String,
    pub values: Vec<f64>,
}

impl ParamValue {
    /// Finite numbers and bounded text: the check for values no installed schema describes.
    pub fn is_sane(&self) -> bool {
        match self {
            Self::Bool(_) => true,
            Self::Number(n) => n.is_finite(),
            Self::Text(t) => t.chars().count() <= MAX_TEXT_PARAM,
            Self::Point(p) => p.x.is_finite() && p.y.is_finite(),
            Self::Font(f) => font_ok(f),
            Self::Rows(rows) => {
                rows.len() <= MAX_TABLE_ROWS as usize
                    && rows.iter().all(|r| {
                        r.values.len() <= MAX_TABLE_COLUMNS
                            && r.values.iter().all(|v| v.is_finite())
                            && r.label.chars().count() <= MAX_ROW_LABEL
                    })
            }
        }
    }
}

/// How a number reads in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ParamUnit {
    /// Pixels at a 1080-pixel shorter side; shaders multiply by `kl.px`.
    Px,
    /// 0–1, shown as 0–100 %.
    Percent,
    Seconds,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ParamKind {
    Float {
        min: f64,
        max: f64,
        default: f64,
        #[serde(default)]
        step: Option<f64>,
        #[serde(default)]
        unit: Option<ParamUnit>,
    },
    Int {
        min: i64,
        max: i64,
        default: i64,
    },
    Bool {
        default: bool,
    },
    /// `#RRGGBB`, or `#RRGGBBAA` when `alpha`.
    Color {
        default: String,
        #[serde(default)]
        alpha: bool,
    },
    Point {
        default: PointN,
    },
    /// Degrees from −360 to 360; shaders receive radians.
    Angle {
        default: f64,
    },
    Choice {
        options: Vec<String>,
        default: String,
    },
    Text {
        default: String,
        max_len: u32,
    },
    /// One of the project's assets, by id (no default).
    Asset {
        kind: AssetKind,
    },
    /// A `FontRef`, resolved through the one font resolver (bundled, then a downloaded Google family, then installed).
    Font {
        default: FontRef,
    },
    /// Rows of a label and `columns.len()` numbers (at most `MAX_TABLE_COLUMNS`), at most `max_rows` of them (and
    /// `MAX_TABLE_ROWS`): a chart's bars, a list's lines.
    Table {
        columns: Vec<String>,
        max_rows: u32,
        default: Vec<TableRow>,
    },
}

/// One declared parameter. JSON: `{"key":"radius","label":"Radius","type":"float","min":0,"max":64,"default":12,…}`,
/// the same shape the plugin manifest uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct ParamSpec {
    /// The stored key; for shader contributions also the WGSL field name.
    pub key: String,
    pub label: String,
    /// When the setting applies; always when absent. Purely presentational: the UI hides the setting otherwise,
    /// and its stored value stays (and stays valid) for when it applies again.
    #[serde(
        rename = "visibleWhen",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub visible_when: Option<VisibleWhen>,
    #[serde(flatten)]
    pub kind: ParamKind,
}

/// A setting applies only while the choice setting `key` of the same contribution has one of the values `in`.
/// JSON: `{"key":"shape","in":["rounded","frame"]}`. `ContribDef::check_conditions` checks it when the definition
/// loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct VisibleWhen {
    pub key: String,
    #[serde(rename = "in")]
    pub values: Vec<String>,
}

impl ParamSpec {
    /// Checks a stored value against this parameter; the refusal names the setting by its label.
    pub fn check(&self, value: &ParamValue, p: &Project) -> Result<(), CodedError> {
        let label = self.label.as_str();
        let range = |n: f64, min: f64, max: f64| {
            if n.is_finite() && (min..=max).contains(&n) {
                Ok(())
            } else {
                Err(msg!(
                    "model.param.range",
                    label = label,
                    min = min,
                    max = max
                ))
            }
        };
        match (&self.kind, value) {
            (ParamKind::Float { min, max, .. }, ParamValue::Number(n)) => range(*n, *min, *max),
            (ParamKind::Int { min, max, .. }, ParamValue::Number(n)) if n.fract() == 0.0 => {
                range(*n, *min as f64, *max as f64)
            }
            (ParamKind::Angle { .. }, ParamValue::Number(n)) => range(*n, -360.0, 360.0),
            (ParamKind::Point { .. }, ParamValue::Point(pt)) => {
                range(pt.x, -1.0, 2.0)?;
                range(pt.y, -1.0, 2.0)
            }
            (ParamKind::Bool { .. }, ParamValue::Bool(_)) => Ok(()),
            (ParamKind::Color { .. }, ParamValue::Text(c)) if valid_color(c) => Ok(()),
            (ParamKind::Choice { options, .. }, ParamValue::Text(t)) if options.contains(t) => {
                Ok(())
            }
            (ParamKind::Text { max_len, .. }, ParamValue::Text(t))
                if t.chars().count() <= *max_len as usize =>
            {
                Ok(())
            }
            (ParamKind::Font { .. }, ParamValue::Font(f)) if font_ok(f) => Ok(()),
            (ParamKind::Asset { kind }, ParamValue::Text(id))
                if p.asset(id).is_some_and(|a| a.kind == *kind) =>
            {
                Ok(())
            }
            (
                ParamKind::Table {
                    columns, max_rows, ..
                },
                ParamValue::Rows(rows),
            ) if columns.len() <= MAX_TABLE_COLUMNS
                && rows.len() <= (*max_rows).min(MAX_TABLE_ROWS) as usize
                && rows.iter().all(|r| {
                    r.values.len() == columns.len()
                        && r.values.iter().all(|v| v.is_finite())
                        && r.label.chars().count() <= MAX_ROW_LABEL
                }) =>
            {
                Ok(())
            }
            (ParamKind::Table { .. }, _) => Err(msg!("model.param.table", label = label)),
            _ => Err(msg!("model.param.invalid", label = label)),
        }
    }
}
