//! A call's arguments read field by field (`Args`), each taken out as it is read.

use std::ops::RangeInclusive;

use klipolo_model::time::{RangeUs, Us, secs_to_us};
use serde_json::{Map, Value};

use super::error::{LONGEST_SECS, Problem, ToolError};
use super::schema::Word;
use super::{MAX_PHRASE_WORDS, PAUSE_SECS};

/// What a value is, as an error says it was.
pub(super) fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(true) => "true",
        Value::Bool(false) => "false",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

const SECONDS: &str = "a number of seconds";

/// One object of arguments being read: each field is taken out as it is read, so what is left is what the tool
/// doesn't take.
pub(super) struct Args {
    pub(super) tool: &'static str,
    /// Where these arguments sit: empty for the call's own, `ranges[1].` for an item of a list.
    pub(super) path: String,
    pub(super) map: Map<String, Value>,
}

impl Args {
    pub(super) fn error(&self, key: &str, problem: Problem) -> ToolError {
        ToolError {
            tool: self.tool.to_owned(),
            field: Some(format!("{}{key}", self.path)),
            problem,
        }
    }

    pub(super) fn take(&mut self, key: &str) -> Result<Value, ToolError> {
        self.map
            .remove(key)
            .ok_or_else(|| self.error(key, Problem::Missing))
    }

    /// A nullable field: None when it is null or left out.
    pub(super) fn take_nullable(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key).filter(|v| !v.is_null())
    }

    pub(super) fn number(
        &self,
        key: &str,
        value: &Value,
        expected: &'static str,
    ) -> Result<f64, ToolError> {
        value.as_f64().ok_or_else(|| {
            self.error(
                key,
                Problem::WrongType {
                    expected,
                    got: kind(value),
                },
            )
        })
    }

    /// Seconds within `bounds`, as given and in microseconds.
    pub(super) fn within(
        &self,
        key: &str,
        secs: f64,
        bounds: &RangeInclusive<f64>,
    ) -> Result<Us, ToolError> {
        if bounds.contains(&secs) {
            Ok(secs_to_us(secs))
        } else {
            Err(self.error(
                key,
                Problem::OutOfRange {
                    min: *bounds.start(),
                    max: *bounds.end(),
                    got: secs,
                },
            ))
        }
    }

    /// A timeline time: seconds, from 0 to the longest time the model stores.
    pub(super) fn time_secs(&mut self, key: &str) -> Result<(f64, Us), ToolError> {
        let value = self.take(key)?;
        let secs = self.number(key, &value, SECONDS)?;
        let bounds = 0.0..=LONGEST_SECS;
        Ok((secs, self.within(key, secs, &bounds)?))
    }

    pub(super) fn time(&mut self, key: &str) -> Result<Us, ToolError> {
        Ok(self.time_secs(key)?.1)
    }

    /// A range from `start` to `end`, the end after the start.
    pub(super) fn range(&mut self, start: &str, end: &str) -> Result<RangeUs, ToolError> {
        let (start_secs, start_us) = self.time_secs(start)?;
        let (end_secs, end_us) = self.time_secs(end)?;
        if end_us <= start_us {
            return Err(self.error(
                end,
                Problem::NotAfter {
                    start_field: format!("{}{start}", self.path),
                    start: start_secs,
                    end: end_secs,
                },
            ));
        }
        Ok(RangeUs {
            start: start_us,
            end: end_us,
        })
    }

    /// A timeline time, or None when it is null or left out.
    pub(super) fn nullable_time(&mut self, key: &str) -> Result<Option<Us>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => {
                self.map.remove(key);
                Ok(None)
            }
            Some(_) => self.time(key).map(Some),
        }
    }

    /// A length in seconds within `bounds`, or None when it is null or left out.
    pub(super) fn nullable_length(
        &mut self,
        key: &str,
        bounds: &RangeInclusive<f64>,
    ) -> Result<Option<Us>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(None);
        };
        let secs = self.number(key, &value, "a number of seconds or null")?;
        self.within(key, secs, bounds).map(Some)
    }

    /// A number within `bounds` that isn't seconds (a volume), or None when it is null or left out.
    pub(super) fn nullable_number(
        &mut self,
        key: &str,
        bounds: &RangeInclusive<f64>,
    ) -> Result<Option<f64>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(None);
        };
        let n = self.number(key, &value, "a number or null")?;
        if bounds.contains(&n) {
            Ok(Some(n))
        } else {
            Err(self.error(
                key,
                Problem::NotWithin {
                    min: *bounds.start(),
                    max: *bounds.end(),
                    got: n,
                },
            ))
        }
    }

    /// True or false, or None when it is null or left out.
    pub(super) fn nullable_flag(&mut self, key: &str) -> Result<Option<bool>, ToolError> {
        match self.take_nullable(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(b)),
            Some(other) => Err(self.error(
                key,
                Problem::WrongType {
                    expected: "true, false or null",
                    got: kind(&other),
                },
            )),
        }
    }

    /// A list of timeline times, at least one, each named by its place (`atSec[1]`) when it is wrong.
    pub(super) fn times(&mut self, key: &str) -> Result<Vec<Us>, ToolError> {
        let value = self.take(key)?;
        let Value::Array(items) = value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "a list",
                    got: kind(&value),
                },
            ));
        };
        if items.is_empty() {
            return Err(self.error(key, Problem::Empty));
        }
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let at = format!("{key}[{i}]");
            self.map.insert(at.clone(), item);
            out.push(self.time(&at)?);
        }
        Ok(out)
    }

    /// `removeSilences`' pause length: null, or seconds within `PAUSE_SECS`.
    pub(super) fn nullable_pause(&mut self, key: &str) -> Result<Option<Us>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(None);
        };
        let secs = self.number(key, &value, "a number of seconds or null")?;
        self.within(key, secs, &PAUSE_SECS).map(Some)
    }

    /// Text with its runs of white space read as one space, trimmed; possibly empty.
    pub(super) fn label(&mut self, key: &str) -> Result<String, ToolError> {
        let value = self.take(key)?;
        let Value::String(text) = &value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "text",
                    got: kind(&value),
                },
            ));
        };
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    /// Text as `label` reads it, not empty.
    pub(super) fn text(&mut self, key: &str) -> Result<String, ToolError> {
        let text = self.label(key)?;
        if text.is_empty() {
            return Err(self.error(key, Problem::Empty));
        }
        Ok(text)
    }

    /// One short noun phrase: no list, at most `MAX_PHRASE_WORDS` words.
    pub(super) fn phrase(&mut self, key: &str) -> Result<String, ToolError> {
        let text = self.text(key)?;
        let words: Vec<&str> = text.split(' ').collect();
        let joined = |w: &&str| w.eq_ignore_ascii_case("and") || w.eq_ignore_ascii_case("or");
        if words.len() > MAX_PHRASE_WORDS
            || text.contains([',', ';', '&', '+', '/'])
            || words.iter().any(joined)
        {
            return Err(self.error(key, Problem::NotOnePhrase));
        }
        Ok(text)
    }

    pub(super) fn flag(&mut self, key: &str) -> Result<bool, ToolError> {
        match self.take(key)? {
            Value::Bool(b) => Ok(b),
            other => Err(self.error(
                key,
                Problem::WrongType {
                    expected: "true or false",
                    got: kind(&other),
                },
            )),
        }
    }

    pub(super) fn word_of<T: Word>(
        &self,
        key: &str,
        value: &Value,
        all: &[T],
        nullable: bool,
    ) -> Result<T, ToolError> {
        (value.as_str())
            .and_then(|s| all.iter().copied().find(|w| w.word() == s))
            .ok_or_else(|| {
                self.error(
                    key,
                    Problem::NotOneOf {
                        allowed: all.iter().map(|w| w.word()).collect(),
                        nullable,
                        got: value.to_string(),
                    },
                )
            })
    }

    pub(super) fn word<T: Word>(&mut self, key: &str, all: &[T]) -> Result<T, ToolError> {
        let value = self.take(key)?;
        self.word_of(key, &value, all, false)
    }

    pub(super) fn nullable_word<T: Word>(
        &mut self,
        key: &str,
        all: &[T],
    ) -> Result<Option<T>, ToolError> {
        match self.take_nullable(key) {
            Some(value) => self.word_of(key, &value, all, true).map(Some),
            None => Ok(None),
        }
    }

    /// A list of objects, each read by `read` (and refused if it holds fields `read` doesn't take).
    pub(super) fn each<T>(
        &mut self,
        key: &str,
        read: fn(&mut Args) -> Result<T, ToolError>,
    ) -> Result<Vec<T>, ToolError> {
        let value = self.take(key)?;
        let Value::Array(items) = value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "a list",
                    got: kind(&value),
                },
            ));
        };
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let at = format!("{key}[{i}]");
            let Value::Object(map) = item else {
                return Err(self.error(
                    &at,
                    Problem::WrongType {
                        expected: "an object",
                        got: kind(&item),
                    },
                ));
            };
            let mut args = Args {
                tool: self.tool,
                path: format!("{}{at}.", self.path),
                map,
            };
            out.push(read(&mut args)?);
            args.finish()?;
        }
        Ok(out)
    }

    /// Refuses the first field left unread.
    pub(super) fn finish(self) -> Result<(), ToolError> {
        match self.map.keys().next() {
            Some(key) => Err(self.error(key, Problem::Unexpected)),
            None => Ok(()),
        }
    }
}
