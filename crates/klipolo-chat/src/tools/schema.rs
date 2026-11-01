//! The pieces of a tool's strict JSON Schema, and the words a tool takes for the model's own values.

use klipolo_model::captions::styles::BuiltinCaptionStyle;
use klipolo_model::ops::Edge;
use klipolo_model::project::FormatKind;
use serde_json::{Map, Value, json};

/// A word a tool takes for one of the model's own values: the model's name for it.
pub(super) trait Word: Copy {
    fn word(self) -> &'static str;
}

impl Word for FormatKind {
    fn word(self) -> &'static str {
        match self {
            FormatKind::Landscape => "landscape",
            FormatKind::Portrait => "portrait",
            FormatKind::Square => "square",
        }
    }
}

impl Word for Edge {
    fn word(self) -> &'static str {
        match self {
            Edge::Start => "start",
            Edge::End => "end",
        }
    }
}

impl Word for BuiltinCaptionStyle {
    fn word(self) -> &'static str {
        self.key()
    }
}

pub(super) fn seconds(description: &str) -> Value {
    json!({"type": "number", "description": description})
}

/// A number (seconds, a volume), or null.
pub(super) fn nullable_number(description: &str) -> Value {
    json!({"type": ["number", "null"], "description": description})
}

pub(super) fn nullable_boolean(description: &str) -> Value {
    json!({"type": ["boolean", "null"], "description": description})
}

pub(super) fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

pub(super) fn boolean(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

pub(super) fn words<T: Word>(all: &[T], description: &str) -> Value {
    let all: Vec<&str> = all.iter().map(|w| w.word()).collect();
    json!({"type": "string", "enum": all, "description": description})
}

/// One of `all`, or null: null is listed in the enum too, or a strict schema would refuse it.
pub(super) fn nullable_words<T: Word>(all: &[T], description: &str) -> Value {
    let mut listed: Vec<Value> = all.iter().map(|w| json!(w.word())).collect();
    listed.push(Value::Null);
    json!({"type": ["string", "null"], "enum": listed, "description": description})
}

pub(super) fn list(items: Value, description: &str) -> Value {
    json!({"type": "array", "items": items, "description": description})
}

/// A closed object: every property required, no others.
pub(super) fn object(properties: Vec<(&'static str, Value)>) -> Value {
    let required: Vec<&str> = properties.iter().map(|(name, _)| *name).collect();
    let properties: Map<String, Value> = (properties.into_iter())
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Text, or null.
pub(super) fn nullable_string(description: &str) -> Value {
    json!({"type": ["string", "null"], "description": description})
}
