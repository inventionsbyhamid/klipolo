//! The effects tool (`setEffects`): add, change, reorder or take off the effects on a clip's footage or a layer, any
//! effect in the catalogue, with settings from its schema. The catalogue stays out of the schema: the prompt lists the
//! effects and their settings in a few words, and the compiler checks each setting against the effect's own schema
//! (`contrib::Catalog::builtin`). An effect is named by the word after `effect.` in its id (`blur`, `drop-shadow`).

use serde_json::Value;

use super::args::{Args, kind};
use super::error::{Problem, ToolError};
use super::schema::string;
use super::sets;
use super::{Tool, ToolCall};

/// What one setting is set to, as the model wrote it: the compiler reads it as its schema's kind says.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    Number(f64),
    Text(String),
    Flag(bool),
}

impl SettingValue {
    /// As the model wrote it, for an error: `20`, `"blue"`, `true`.
    pub fn said(&self) -> String {
        match self {
            SettingValue::Number(n) => format!("{n}"),
            SettingValue::Text(t) => format!("{t:?}"),
            SettingValue::Flag(b) => format!("{b}"),
        }
    }
}

/// One effect `setEffects` adds, changes, moves or takes off.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectChange {
    /// As written (`blur`, `drop-shadow`), not yet found in the catalogue.
    pub name: String,
    /// Its place in the stack, from 0; None keeps it where it is, or adds it last.
    pub at: Option<usize>,
    pub remove: bool,
    pub settings: Vec<(String, SettingValue)>,
}

/// The most places a stack has.
pub const MAX_PLACE: f64 = 31.0;

fn setting(a: &mut Args) -> Result<(String, SettingValue), ToolError> {
    let key = a.text("key")?;
    let value = match a.take("value")? {
        Value::Bool(b) => SettingValue::Flag(b),
        Value::String(s) => SettingValue::Text(s.trim().to_owned()),
        Value::Number(n) => SettingValue::Number(n.as_f64().unwrap_or(f64::NAN)),
        other => {
            return Err(a.error(
                "value",
                Problem::WrongType {
                    expected: "a number, text or true or false",
                    got: kind(&other),
                },
            ));
        }
    };
    Ok((key, value))
}

pub(super) const SET_EFFECTS: Tool = Tool {
    name: "setEffects",
    description: "Add, change, move or remove one effect on a clip or layer. Keys: its settings, at (its place from 0), \
                  remove (true).",
    parameters: || {
        vec![
            ("target", string("A ref, or last.")),
            ("name", string("An effect, or a clip's look.")),
            ("set", sets::schema("Its settings, and at or remove.")),
        ]
    },
    read: |a| {
        let target = a.layer_ref("target")?;
        let name = a.text("name")?;
        let read = a.each("set", setting)?;
        let (mut at, mut remove, mut settings) = (None, false, Vec::new());
        for (i, (key, value)) in read.into_iter().enumerate() {
            let field = format!("set[{i}].value");
            match (key.as_str(), value) {
                ("at", SettingValue::Number(n))
                    if (0.0..=MAX_PLACE).contains(&n) && n.fract() == 0.0 =>
                {
                    at = Some(n as usize)
                }
                ("at", other) => {
                    return Err(a.error(
                        &field,
                        Problem::NotForm {
                            expected: "a place in the stack, a whole number from 0 to 31",
                            got: other.said(),
                        },
                    ));
                }
                ("remove", SettingValue::Flag(b)) => remove = b,
                ("remove", other) => {
                    return Err(a.error(
                        &field,
                        Problem::NotForm {
                            expected: "true or false",
                            got: other.said(),
                        },
                    ));
                }
                (_, value) => settings.push((key, value)),
            }
        }
        if remove && (!settings.is_empty() || at.is_some()) {
            return Err(a.error(
                "set",
                Problem::Unused {
                    why: "sets more than remove: an effect taken off takes no settings or place",
                },
            ));
        }
        Ok(ToolCall::SetEffects {
            target,
            effects: vec![EffectChange {
                name,
                at,
                remove,
                settings,
            }],
        })
    },
};
