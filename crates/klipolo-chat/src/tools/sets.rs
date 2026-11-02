//! A list of settings a tool takes as `{key, value}` pairs (`setLayer`, `styleText`, `animateText`'s values), so a long
//! list of fields stays out of the schema: the prompt lists the keys and what each takes, and each value is read here
//! by its key, refused with what it takes when it doesn't fit.

use serde_json::{Value, json};

use super::args::{Args, kind};
use super::error::{Problem, ToolError};
use super::schema::{list, object, string};

/// One pair as written: its key (any case), its value, and where it sits (`set[2]`).
pub(super) struct Pair {
    pub key: &'static str,
    pub value: Value,
    pub at: String,
}

/// The pairs at `field`, each key one of `keys` and each at most once.
pub(super) fn pairs(
    a: &mut Args,
    field: &str,
    keys: &[&'static str],
) -> Result<Vec<Pair>, ToolError> {
    let read = a.each(field, |item| {
        let key = item.text("key")?;
        let value = item.take("value")?;
        Ok((key, value))
    })?;
    let mut out: Vec<Pair> = Vec::with_capacity(read.len());
    for (i, (key, value)) in read.into_iter().enumerate() {
        let at = format!("{field}[{i}]");
        let Some(key) = keys.iter().copied().find(|k| k.eq_ignore_ascii_case(&key)) else {
            return Err(a.error(
                &format!("{at}.key"),
                Problem::NotOneOf {
                    allowed: keys.to_vec(),
                    nullable: false,
                    got: Value::String(key).to_string(),
                },
            ));
        };
        if out.iter().any(|p| p.key == key) {
            return Err(a.error(
                &format!("{at}.key"),
                Problem::Unused {
                    why: "is set twice: give each key once",
                },
            ));
        }
        out.push(Pair { key, value, at });
    }
    Ok(out)
}

impl Pair {
    fn error(&self, a: &Args, problem: Problem) -> ToolError {
        a.error(&format!("{}.value", self.at), problem)
    }

    /// A number within `min..=max`.
    pub fn number(&self, a: &Args, (min, max): (f64, f64)) -> Result<f64, ToolError> {
        let n = self.value.as_f64().ok_or_else(|| {
            self.error(
                a,
                Problem::WrongType {
                    expected: "a number",
                    got: kind(&self.value),
                },
            )
        })?;
        if (min..=max).contains(&n) {
            Ok(n)
        } else {
            Err(self.error(a, Problem::NotWithin { min, max, got: n }))
        }
    }

    pub fn flag(&self, a: &Args) -> Result<bool, ToolError> {
        self.value.as_bool().ok_or_else(|| {
            self.error(
                a,
                Problem::WrongType {
                    expected: "true or false",
                    got: kind(&self.value),
                },
            )
        })
    }

    /// Text, trimmed.
    pub fn text(&self, a: &Args) -> Result<String, ToolError> {
        match &self.value {
            Value::String(s) if !s.trim().is_empty() => Ok(s.trim().to_owned()),
            Value::String(_) => Err(self.error(a, Problem::Empty)),
            other => Err(self.error(
                a,
                Problem::WrongType {
                    expected: "text",
                    got: kind(other),
                },
            )),
        }
    }

    /// One of `words` (any case).
    pub fn word(&self, a: &Args, words: &[&'static str]) -> Result<&'static str, ToolError> {
        let said = self.value.as_str().map(str::trim).unwrap_or_default();
        words
            .iter()
            .copied()
            .find(|w| w.eq_ignore_ascii_case(said))
            .ok_or_else(|| {
                self.error(
                    a,
                    Problem::NotOneOf {
                        allowed: words.to_vec(),
                        nullable: false,
                        got: self.value.to_string(),
                    },
                )
            })
    }

    /// Four numbers in text (`"0.1 0.2 0.5 0.3"`, commas allowed), or a list of four.
    pub fn four(&self, a: &Args, form: &'static str) -> Result<[f64; 4], ToolError> {
        let numbers: Option<Vec<f64>> = match &self.value {
            Value::String(s) => s
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|p| !p.is_empty())
                .map(|p| p.parse::<f64>().ok().filter(|v| v.is_finite()))
                .collect(),
            Value::Array(items) => items.iter().map(Value::as_f64).collect(),
            _ => None,
        };
        match numbers.as_deref() {
            Some(&[w, x, y, z]) => Ok([w, x, y, z]),
            _ => Err(self.error(
                a,
                Problem::NotForm {
                    expected: form,
                    got: self.value.to_string(),
                },
            )),
        }
    }
}

/// The schema of a list of `{key, value}` pairs.
pub(super) fn schema(description: &str) -> Value {
    list(
        object(vec![
            ("key", string("A key.")),
            (
                "value",
                json!({"type": ["number", "string", "boolean"], "description": "Its value."}),
            ),
        ]),
        description,
    )
}

/// [`schema`], or null.
pub(super) fn nullable_schema(description: &str) -> Value {
    let mut out = schema(description);
    out["type"] = json!(["array", "null"]);
    out
}
