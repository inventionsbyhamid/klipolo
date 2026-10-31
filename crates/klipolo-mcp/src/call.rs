//! An edit tool's call: chat's own tool, read, checked and compiled as chat's are, then applied, proposed or refused.

use klipolo_chat::FunctionCall;
use serde_json::Value;

use crate::rpc::{Mcp, tool_result};
use crate::session::Session;
use crate::words::NOT_BUILT;

/// What `call` did, as a tool result. Not built yet: every call answers so.
pub fn run(_mcp: &Mcp, _session: &mut Session, _call: &FunctionCall) -> Value {
    tool_result(NOT_BUILT, true)
}
