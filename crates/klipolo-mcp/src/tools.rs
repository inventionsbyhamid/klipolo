//! The tools an app is offered: chat's own edit tools, byte for byte (`klipolo_chat::tools`), minus the two that talk
//! to the person in Klipolo's chat (`askUser`, `cannotDo`: an app talks to its own person), then four reads. Each as
//! MCP lists a tool: `{name, description, inputSchema, annotations}`, the schema strict as chat's (closed, every
//! property required, a value that may be left unsaid nullable). The list never changes while an app is connected.
//!
//! Only a few words are an app's own (`apps_words`). Chat's model puts the person's words on screen, an app its own
//! (`klipolo_chat::compile_for_app`), so `addText`'s text and `makeTitleCard` say so. A sticker, chat's or an app's,
//! waits once drawn in Klipolo's chat panel for the person's Use; an app's person may not be looking there, so an
//! app's `makeSticker` says where it waits.

use std::sync::LazyLock;

use serde_json::{Map, Value, json};

use crate::words;

/// Chat's tools an app isn't offered.
pub const CONVERSATION: [&str; 2] = ["askUser", "cannotDo"];
/// The reads, in the order listed.
pub const READS: [&str; 4] = ["getSummary", "getContext", "getTranscript", "getProposal"];
/// The edit tool that changes nothing in the edit: it points the person at moments.
const SHOWS: &str = "findMoments";

/// The list, built once.
static LIST: LazyLock<Value> = LazyLock::new(build);

/// Every tool an app may call, in a fixed order: chat's edit tools, then the reads.
pub fn list() -> Value {
    LIST.clone()
}

/// Whether `name` is one of chat's edit tools an app may call.
pub fn is_edit(name: &str) -> bool {
    !is_read(name) && (LIST.as_array().into_iter().flatten()).any(|t| t["name"] == name)
}

/// Whether `name` is one of the reads.
pub fn is_read(name: &str) -> bool {
    READS.contains(&name)
}

fn build() -> Value {
    let chat = klipolo_chat::tools();
    let edits = (chat.as_array().into_iter().flatten())
        .filter(|t| !CONVERSATION.contains(&t["name"].as_str().unwrap_or_default()))
        .map(|t| {
            let name = t["name"].as_str().unwrap_or_default();
            let (description, schema) = apps_words(name, &t["description"], &t["parameters"]);
            listed(name, &description, schema, name == SHOWS)
        });
    let nullable_seconds =
        |description: &str| json!({"type": ["number", "null"], "description": description});
    let reads = [
        ("getSummary", words::SUMMARY_TOOL, closed(Vec::new())),
        ("getContext", words::CONTEXT_TOOL, closed(Vec::new())),
        (
            "getTranscript",
            words::TRANSCRIPT_TOOL,
            closed(vec![
                ("fromSec", nullable_seconds(words::FROM_SEC)),
                ("toSec", nullable_seconds(words::TO_SEC)),
            ]),
        ),
        ("getProposal", words::PROPOSAL_TOOL, closed(Vec::new())),
    ]
    .into_iter()
    .map(|(name, description, schema)| listed(name, &json!(description), schema, true));
    Value::Array(edits.chain(reads).collect())
}

/// Chat's tool `name` (its `description` and `schema`) as an app reads it: the words it puts on screen are its own,
/// and the sticker it starts waits for the person's Use.
fn apps_words(name: &str, description: &Value, schema: &Value) -> (Value, Value) {
    let (mut description, mut schema) = (description.clone(), schema.clone());
    let mut field = |field: &str, said: &str| {
        if let Some(property) = schema.pointer_mut(&format!("/properties/{field}")) {
            property["description"] = json!(said);
        }
    };
    match name {
        "addText" => field("text", words::TEXT_FIELD),
        "makeTitleCard" => {
            field("title", words::TITLE_FIELD);
            description = json!(words::TITLE_CARD_TOOL);
        }
        "makeSticker" => description = json!(words::STICKER_TOOL),
        _ => {}
    }
    (description, schema)
}

fn listed(name: &str, description: &Value, schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": read_only},
    })
}

/// A closed object schema with every property required, as chat writes them.
fn closed(properties: Vec<(&str, Value)>) -> Value {
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
