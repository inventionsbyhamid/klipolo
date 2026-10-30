//! Meta's Muse Spark behind chat's `klipolo_chat::LanguageModel`: the neutral request as a Responses API body, Meta's
//! answer and errors back in chat's terms. Whether the client calls Meta with the person's key or goes through the
//! relay is the client's business; Meta's types stop here.
//!
//! The body is what the relay takes and counts as text only: `model`, `instructions` (the system text), strict
//! function `tools`, `tool_choice: "auto"`, parallel calls, `max_output_tokens` within the relay's cap and `input`,
//! with nothing stored at Meta (no `previous_response_id` or `conversation`) and no reasoning items: a refusal round
//! sends the whole input again. Reasoning effort is Meta's default (the field is left out). The context is the first
//! input item, as the person's message, so the instructions, the tools and the context stay one prefix Meta's cache
//! reuses.
//!
//! The relay takes 4 Meta calls at once from a device; the analysis holds 3 (`meta::RELAY_META_CALLS`) and chat the
//! fourth: `MetaChat` keeps its own single slot, shared by its clones, and never takes one of `Meta::call`'s. A busy
//! answer (the relay's `busy`, or Traefik's plain-text 429) is asked again after a short wait, at most
//! `CHAT_BUSY_RETRIES` times; then it is `ModelError::Busy`. Each answer is priced from its `usage`, cached input at
//! the cached price (`pricing::spark_usd`); the client writes the spend ledger's row.

use std::sync::Arc;
use std::time::Duration;

use klipolo_chat::{
    BoxFuture, FunctionCall, Item, LanguageModel, ModelError, ModelRequest, ModelResponse,
};
use klipolo_meta::{MetaClient, MetaError, ResponseOut, pricing};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::meta::RELAY_BUSY;

/// The model chat runs on.
pub const CHAT_MODEL: &str = "muse-spark-1.3";
/// The most output tokens a chat answer may take, reasoning included: the planner spike's answers took at most 1,291,
/// and the relay takes at most 16,384 (its reservation counts all of it).
pub const CHAT_MAX_OUTPUT_TOKENS: u32 = 4096;
/// Chat calls at once: one of the device's four relay calls.
pub const CHAT_CALLS: usize = 1;
/// Times a busy answer is asked again.
pub const CHAT_BUSY_RETRIES: u32 = 2;
/// The longest wait before asking again, whatever `Retry-After` says.
pub const CHAT_BUSY_WAIT_MAX: Duration = Duration::from_secs(5);

/// Muse Spark as chat's language model, within chat's own slot. Its clones share the client and the slot.
#[derive(Clone, Debug)]
pub struct MetaChat {
    client: MetaClient,
    slot: Arc<Semaphore>,
    busy_wait_max: Duration,
}

impl MetaChat {
    /// One device's chat model. Make one per app and share it.
    pub fn new(client: MetaClient) -> Self {
        Self {
            client,
            slot: Arc::new(Semaphore::new(CHAT_CALLS)),
            busy_wait_max: CHAT_BUSY_WAIT_MAX,
        }
    }

    /// The longest wait before a busy answer is asked again (`CHAT_BUSY_WAIT_MAX`; tests shorten it).
    pub fn with_busy_wait_max(mut self, wait: Duration) -> Self {
        self.busy_wait_max = wait;
        self
    }

    /// How long to wait before asking again after `e`, or None when `e` is not a busy moment.
    fn busy_wait(&self, e: &MetaError) -> Option<Duration> {
        match e {
            // The relay's `busy`, or a 429 with no code at all (Traefik's plain-text rate limit).
            MetaError::RateLimited {
                retry_after, code, ..
            } if code.as_deref().is_none_or(|c| c == RELAY_BUSY) => {
                let wait = Duration::from_secs(retry_after.unwrap_or(1).max(1));
                Some(wait.min(self.busy_wait_max))
            }
            _ => None,
        }
    }
}

impl LanguageModel for MetaChat {
    fn respond<'a>(
        &'a self,
        request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            let body = wire_body(request);
            let _slot = (self.slot.acquire().await)
                .map_err(|_| ModelError::other("chat's slot is closed"))?;
            let mut retries = 0;
            loop {
                match self.client.respond(body.clone()).await {
                    Ok(out) => return answer(out),
                    Err(e) => match self.busy_wait(&e) {
                        Some(wait) if retries < CHAT_BUSY_RETRIES => {
                            tokio::time::sleep(wait).await;
                            retries += 1;
                        }
                        _ => return Err(model_error(e)),
                    },
                }
            }
        })
    }
}

/// `request` as a Responses API body.
pub fn wire_body(request: &ModelRequest) -> Value {
    let tools: Vec<Value> = (request.tools.as_array().into_iter().flatten())
        .map(|tool| {
            let mut tool = tool.clone();
            if let Some(t) = tool.as_object_mut() {
                t.insert("type".into(), json!("function"));
                t.insert("strict".into(), json!(true));
            }
            tool
        })
        .collect();
    let input: Vec<Value> = request.input.iter().map(item).collect();
    json!({
        "model": CHAT_MODEL,
        "instructions": request.system,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "max_output_tokens": CHAT_MAX_OUTPUT_TOKENS,
        "input": input,
    })
}

/// One input item in the Responses API's shape. A replayed call carries no `id`: it names no item stored at Meta.
fn item(item: &Item) -> Value {
    let said = |role: &str, part: &str, text: &str| json!({"type": "message", "role": role, "content": [{"type": part, "text": text}]});
    match item {
        Item::Context { text } | Item::User { text } => said("user", "input_text", text),
        Item::Assistant { text } => said("assistant", "output_text", text),
        Item::FunctionCall(call) => json!({
            "type": "function_call",
            "call_id": call.call_id,
            "name": call.name,
            "arguments": call.arguments,
        }),
        Item::FunctionCallOutput { call_id, output } => json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": output,
        }),
    }
}

/// Meta's answer in chat's terms, priced from its usage.
fn answer(out: ResponseOut) -> Result<ModelResponse, ModelError> {
    if out.status.as_deref() == Some("failed") {
        return Err(ModelError::other("Meta's answer failed"));
    }
    Ok(ModelResponse {
        text: out.output_text.unwrap_or_default(),
        calls: (out.function_calls.into_iter())
            .map(|c| FunctionCall {
                name: c.name,
                call_id: c.call_id,
                arguments: c.arguments,
            })
            .collect(),
        usage_usd: pricing::spark_usd(out.usage),
    })
}

/// A Meta client error in chat's terms. A 402 keeps the relay's or Meta's sentence (which credit, what to do); a
/// request refused as sent or an unreadable answer keeps its (redacted) text for logs only.
pub fn model_error(e: MetaError) -> ModelError {
    match e {
        MetaError::Budget { .. } => ModelError::Budget(e.user_message()),
        MetaError::RateLimited { .. } => ModelError::Busy,
        MetaError::Overloaded { .. } => ModelError::Unavailable,
        MetaError::Network(_) => ModelError::Network,
        MetaError::Unauthorized | MetaError::NotAllowed { .. } => ModelError::Refused,
        MetaError::BadRequest { .. }
        | MetaError::Decode(_)
        | MetaError::AlignUnavailable
        | MetaError::LocalFile { .. } => ModelError::other(e.to_string()),
    }
}
