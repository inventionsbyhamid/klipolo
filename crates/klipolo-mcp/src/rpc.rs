//! MCP over JSON-RPC 2.0: `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list`,
//! `resources/templates/list` and `resources/read`, one message in and at most one reply out. `initialize` opens a
//! session; every other message needs one (`Handled::NoSession` otherwise). An edit tool's call goes to `call::run`; a
//! read is answered here. Refusals an agent can act on are tool results with `isError`; JSON-RPC errors are only for a
//! message that isn't one object or a request without a method (-32600), an unknown method (-32601), an unknown tool
//! (-32602) and an unknown resource (-32002). Bad JSON (-32700) is the server's, before a message reaches here.
//! Nothing here trusts the client.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use klipolo_chat::FunctionCall;
use klipolo_model::text_safety::clean;
use serde_json::{Value, json};

use crate::host::Host;
use crate::resources;
use crate::session::{NAME_CHARS, Session, Sessions, client_name, locked};
use crate::{call, tools, words};

/// The protocol versions Klipolo speaks: it echoes the client's when listed, else answers with the last.
pub const VERSIONS: [&str; 2] = ["2025-06-18", "2025-11-25"];

/// What became of one message.
#[derive(Clone, Debug, PartialEq)]
pub enum Handled {
    /// A JSON-RPC reply; `new_session` is the session an `initialize` opened (`Mcp-Session-Id`).
    Reply {
        body: Value,
        new_session: Option<String>,
    },
    /// A notification, or a client's reply: nothing to answer.
    Accepted,
    /// No session, or one Klipolo doesn't know (it restarted, or the session went idle): the client opens a new one.
    NoSession,
    /// Too many messages from this client: it waits and tries again.
    Paced,
}

/// The server's protocol state: the host, and the sessions.
pub struct Mcp {
    host: Arc<dyn Host>,
    sessions: Mutex<Sessions>,
    /// The apps last told to the host as connected; locked while telling, so the host hears every change in order.
    told: Mutex<Vec<String>>,
    /// The waiting proposal an app made here, which its next call joins.
    own: Mutex<Option<call::Own>>,
}

impl Mcp {
    pub fn new(host: Arc<dyn Host>) -> Mcp {
        Mcp {
            host,
            sessions: Mutex::new(Sessions::default()),
            told: Mutex::new(Vec::new()),
            own: Mutex::new(None),
        }
    }

    pub fn host(&self) -> &dyn Host {
        &*self.host
    }

    /// The waiting proposal an app made here, if Klipolo still holds it: locked briefly, never across a host call.
    pub(crate) fn own(&self) -> MutexGuard<'_, Option<call::Own>> {
        locked(&self.own)
    }

    /// One message from a client, in session `session` (its `Mcp-Session-Id`). Synchronous: the server runs it on a
    /// blocking thread. A session's messages are handled one at a time.
    pub fn handle(&self, session: Option<&str>, msg: Value) -> Handled {
        let Value::Object(obj) = &msg else {
            return reply(error(&Value::Null, -32600, words::ONE_MESSAGE));
        };
        let id = obj.get("id");
        let method = obj.get("method").and_then(Value::as_str);
        let params = obj.get("params").unwrap_or(&Value::Null);
        if let (Some(id), Some("initialize")) = (id, method) {
            return self.initialize(id, params);
        }
        let Some(entry) = session.and_then(|s| locked(&self.sessions).get(s)) else {
            return Handled::NoSession;
        };
        let (Some(id), Some(method)) = (id, method) else {
            // A request with no method to answer: the client waits on its id, so it hears why.
            if let Some(id) = id
                && !obj.contains_key("result")
                && !obj.contains_key("error")
            {
                return reply(error(id, -32600, words::NO_METHOD));
            }
            // A notification, or a reply to a request: Klipolo sends no requests.
            return Handled::Accepted;
        };
        let mut session = locked(&entry);
        session.used = Instant::now();
        reply(match method {
            "tools/call" => self.call_tool(&mut session, id, params),
            "resources/read" => {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match resources::read(self.host(), &mut session, uri) {
                    Some((mime, text)) => ok(
                        id,
                        json!({"contents": [{"uri": uri, "mimeType": mime, "text": text}]}),
                    ),
                    None => error(id, -32002, words::UNKNOWN_RESOURCE),
                }
            }
            other => match listed(other) {
                Some(result) => ok(id, result),
                None => error(id, -32601, words::UNKNOWN_METHOD),
            },
        })
    }

    /// The apps with a session now, each once, sorted. A session goes once `expire` finds it unused for `IDLE`, so
    /// one may be listed a little longer than that, until the next `expire`.
    pub fn connected(&self) -> Vec<String> {
        locked(&self.sessions).connected()
    }

    /// Drops the sessions unused for `IDLE` at `now`; true when any went. The host hears if the apps connected changed.
    pub fn expire(&self, now: Instant) -> bool {
        let gone = locked(&self.sessions).expire(now);
        if gone {
            self.tell_connected();
        }
        gone
    }

    /// A copy of session `id`, if Klipolo has it.
    pub fn session(&self, id: &str) -> Option<Session> {
        let entry = locked(&self.sessions).get(id)?;
        let session = locked(&entry).clone();
        Some(session)
    }

    fn initialize(&self, id: &Value, params: &Value) -> Handled {
        let client = client_name(params);
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let session = locked(&self.sessions).open(client, Instant::now());
        self.tell_connected();
        Handled::Reply {
            body: ok(id, initialize_result(asked)),
            new_session: Some(session),
        }
    }

    /// Tells the host the apps connected, when they changed since it was last told.
    fn tell_connected(&self) {
        let mut told = locked(&self.told);
        let now = self.connected();
        if *told != now {
            self.host.connected(&now);
            *told = now;
        }
    }

    fn call_tool(&self, session: &mut Session, id: &Value, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(args) => args.clone(),
        };
        let host = self.host();
        let read = match name {
            "getSummary" => resources::summary(host, session),
            "getContext" => resources::context(host, session),
            "getTranscript" => match resources::transcript_times(&args) {
                Ok((from, to)) => resources::transcript(host, session, from, to),
                Err(refused) => refused,
            },
            "getProposal" => {
                let outcome = resources::proposal(host, session);
                return ok(
                    id,
                    json!({
                        "content": [{"type": "text", "text": outcome.to_string()}],
                        "isError": false,
                        "structuredContent": outcome,
                    }),
                );
            }
            _ if tools::is_edit(name) => {
                let call = FunctionCall {
                    name: name.to_owned(),
                    call_id: match id {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    },
                    arguments: args.to_string(),
                };
                return ok(id, call::run(self, session, &call));
            }
            _ => {
                return error(
                    id,
                    -32602,
                    &format!("Unknown tool: {}.", clean(name, NAME_CHARS)),
                );
            }
        };
        ok(id, tool_result(&read.text, read.refused))
    }
}

/// The answer to `initialize`: the protocol version `asked` when Klipolo speaks it, else its latest; what it serves;
/// and how to edit through it. The same for every client, so the stdio bridge answers it too.
pub fn initialize_result(asked: &str) -> Value {
    let version = VERSIONS
        .iter()
        .find(|v| **v == asked)
        .unwrap_or(&VERSIONS[VERSIONS.len() - 1]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}, "resources": {}},
        "serverInfo": {"name": "klipolo", "title": "Klipolo", "version": env!("CARGO_PKG_VERSION")},
        "instructions": words::INSTRUCTIONS,
    })
}

/// The answers that don't depend on the app (`ping`, the tools, the resources and their template), so the stdio
/// bridge answers them the same while the app is closed. None for any other method.
pub fn listed(method: &str) -> Option<Value> {
    Some(match method {
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools::list()}),
        "resources/list" => json!({"resources": resources::list()}),
        "resources/templates/list" => json!({"resourceTemplates": resources::templates()}),
        _ => return None,
    })
}

/// A tool's result: one text, an error the agent can act on or not.
pub fn tool_result(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn reply(body: Value) -> Handled {
    Handled::Reply {
        body,
        new_session: None,
    }
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
