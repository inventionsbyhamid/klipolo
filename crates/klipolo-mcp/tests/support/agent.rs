//! An app connected to Klipolo's MCP server in a test: one session, and the tool calls it makes.

#![allow(dead_code)]

use klipolo_mcp::rpc::{Handled, Mcp};
use serde_json::{Value, json};

pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// One session of the app named `name`.
pub struct Agent<'a> {
    pub mcp: &'a Mcp,
    pub session: String,
}

impl<'a> Agent<'a> {
    /// Opens a session as the app `name`.
    pub fn open(mcp: &'a Mcp, name: &str) -> Self {
        let params = json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": name, "version": "1.0"},
        });
        match mcp.handle(None, request(1, "initialize", params)) {
            Handled::Reply {
                new_session: Some(session),
                ..
            } => Agent { mcp, session },
            other => panic!("initialize opens a session, got {other:?}"),
        }
    }

    /// Calls tool `name`: its result.
    pub fn call(&self, name: &str, arguments: Value) -> Value {
        let msg = request(
            7,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        );
        match self.mcp.handle(Some(&self.session), msg) {
            Handled::Reply { body, .. } => {
                assert_eq!(body["id"], 7, "{body}");
                body["result"].clone()
            }
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    /// Reads the context, as an app does before naming times.
    pub fn read(&self) {
        let result = self.call("getContext", json!({}));
        assert_eq!(result["isError"], false, "{result}");
    }

    /// `cutRange` from `a` to `b` timeline seconds.
    pub fn cut(&self, a: f64, b: f64) -> Value {
        self.call("cutRange", json!({"startSec": a, "endSec": b}))
    }
}

/// A tool result's one text.
pub fn text(result: &Value) -> &str {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a text result: {result}"))
}

/// What a call did, as its result says (`done`, `proposed`, `shown`, `started`, `refused`).
pub fn outcome(result: &Value) -> &str {
    result["structuredContent"]["outcome"]
        .as_str()
        .unwrap_or_else(|| panic!("an outcome: {result}"))
}

/// A refused call: an error, with this text.
pub fn assert_refused(result: &Value, said: &str) {
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(outcome(result), "refused", "{result}");
    assert_eq!(text(result), said, "{result}");
}
