//! Meta's Muse Spark behind chat's `klipolo_chat::LanguageModel`: the neutral request as a Responses API body, Meta's
//! answer and errors back in chat's terms. Whether the client calls Meta with the person's key or goes through the
//! relay is the client's business; Meta's types stop here.
//!
//! The body is what the relay takes and counts as text only: `model`, `instructions` (the system text), strict
//! function `tools`, `tool_choice: "auto"`, parallel calls, `max_output_tokens` within the relay's cap, `store: false`
//! (Meta keeps no copy of the answer) and `input`. No call refers to stored state (no `previous_response_id` or
//! `conversation`) and no reasoning item is sent back: a refusal round sends the whole input again. Reasoning effort is
//! Meta's default (the field is left out). The context is the first input item, as the person's message, so the
//! instructions, the tools and the context stay one prefix Meta's cache reuses.
//!
//! The relay takes 4 Meta calls at once from a device; the analysis holds 3 (`meta::RELAY_META_CALLS`) and chat the
//! fourth: `MetaChat` keeps its own single slot, shared by its clones, and never takes one of `Meta::call`'s. A busy
//! answer (the relay's `busy`, or Traefik's plain-text 429) is asked again after a short wait, at most
//! `CHAT_BUSY_RETRIES` times; then it is `ModelError::Busy`. A message's answer gets `CHAT_ANSWER_TIMEOUT` in all
//! (waits for the slot and busy retries included); past it the message fails saying who took too long, and chat is
//! free for the next one. Each answer is priced from its `usage`, cached input at the cached price
//! (`pricing::spark_usd`); the client writes the spend ledger's row.

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
/// The most output tokens a chat answer may take, reasoning included: the relay's own cap. Meta bills only what an
/// answer uses; a short's whole plan in one answer took 6,016 in one paid run and reached 8,192 (cut off) in the next.
/// The relay reserves the whole cap while the call runs: about $0.07 of output, with the input about $0.15 a call, far
/// under a device's $2 cap.
pub const CHAT_MAX_OUTPUT_TOKENS: u32 = 16_384;
/// Chat calls at once: one of the device's four relay calls.
pub const CHAT_CALLS: usize = 1;
/// Times a busy answer is asked again.
pub const CHAT_BUSY_RETRIES: u32 = 2;
/// The longest wait before asking again, whatever `Retry-After` says.
pub const CHAT_BUSY_WAIT_MAX: Duration = Duration::from_secs(5);
/// The longest one model call of a message may take, from asking for chat's slot to the answer. A short's whole plan
/// cut off at 8,192 output tokens took about 48 s in a paid run, so a full `CHAT_MAX_OUTPUT_TOKENS` answer takes about
/// 96 s: this leaves room for it under the relay's own 300 s read limit. Past it the call is dropped (its request with
/// it), billed all the same.
pub const CHAT_ANSWER_TIMEOUT: Duration = Duration::from_secs(180);

/// Muse Spark as chat's language model, within chat's own slot. Its clones share the client and the slot.
#[derive(Clone, Debug)]
pub struct MetaChat {
    client: MetaClient,
    slot: Arc<Semaphore>,
    busy_wait_max: Duration,
    answer_timeout: Duration,
}

impl MetaChat {
    /// One device's chat model. Make one per app and share it.
    pub fn new(client: MetaClient) -> Self {
        Self {
            client,
            slot: Arc::new(Semaphore::new(CHAT_CALLS)),
            busy_wait_max: CHAT_BUSY_WAIT_MAX,
            answer_timeout: CHAT_ANSWER_TIMEOUT,
        }
    }

    /// The longest wait before a busy answer is asked again (`CHAT_BUSY_WAIT_MAX`; tests shorten it).
    pub fn with_busy_wait_max(mut self, wait: Duration) -> Self {
        self.busy_wait_max = wait;
        self
    }

    /// The longest one call may take (`CHAT_ANSWER_TIMEOUT`; tests shorten it).
    pub fn with_answer_timeout(mut self, timeout: Duration) -> Self {
        self.answer_timeout = timeout;
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
            let asked = async {
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
                            _ => return Err(model_error(e, self.client.is_relay())),
                        },
                    }
                }
            };
            // Dropped past the limit: the request goes, and so does the slot.
            (tokio::time::timeout(self.answer_timeout, asked).await).unwrap_or_else(|_| {
                Err(ModelError::Unavailable(Some(format!(
                    "{} took too long to answer. Try again.",
                    who(self.client.is_relay())
                ))))
            })
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
        "store": false,
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

/// Meta's answer in chat's terms, priced from its usage. An answer cut off at its output cap is `ModelError::CutOff`
/// with its words and the calls it finished (not marked `incomplete`, and arguments that read as a JSON object), for the
/// planner to keep and ask again. One that came back `failed`, or `incomplete` for another reason, is an error, none of
/// its calls used. Either is still billed, so the error carries its cost.
fn answer(out: ResponseOut) -> Result<ModelResponse, ModelError> {
    let usage_usd = pricing::spark_usd(out.usage);
    // A code from the answer, for logs: only lowercase letters, digits and underscores, never free text.
    let code = |pointer: &str| {
        (out.raw.pointer(pointer).and_then(Value::as_str))
            .filter(|c| {
                !c.is_empty()
                    && c.len() <= 64
                    && (c.bytes())
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            .unwrap_or("no reason given")
            .to_owned()
    };
    let cut_off = out.status.as_deref() == Some("incomplete")
        && code("/incomplete_details/reason") == "max_output_tokens";
    // In a cut-off answer, the calls Meta marks as cut part-way.
    let unfinished: Vec<String> = (out.raw.pointer("/output").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "function_call" && item["status"] == "incomplete")
        .filter_map(|item| item["call_id"].as_str().map(str::to_owned))
        .collect();
    let detail = match out.status.as_deref() {
        _ if cut_off => None,
        Some("failed") => Some(format!("answer failed: {}", code("/error/code"))),
        Some("incomplete") => Some(format!("cut off: {}", code("/incomplete_details/reason"))),
        _ => None,
    };
    if let Some(detail) = detail {
        return Err(ModelError::Other { detail, usage_usd });
    }
    let answer = ModelResponse {
        text: out.output_text.unwrap_or_default(),
        calls: (out.function_calls.into_iter())
            .filter(|c| {
                !cut_off
                    || (!unfinished.contains(&c.call_id)
                        && serde_json::from_str::<Value>(&c.arguments).is_ok_and(|v| v.is_object()))
            })
            .map(|c| FunctionCall {
                name: c.name,
                call_id: c.call_id,
                arguments: c.arguments,
            })
            .collect(),
        usage_usd,
    };
    if cut_off {
        return Err(ModelError::CutOff(answer));
    }
    Ok(answer)
}

/// What the person is told when the relay, or Traefik in front of it, gave up waiting and said nothing (a bare 504 or
/// 408): Klipolo's server, as Settings says it, not Meta.
pub const SERVER_BUSY: &str = "Klipolo's server is busy right now. Try again in a moment.";

/// The codes of a 429 that means the key's credit or quota is used up, not a busy moment: out of credit, never asked
/// again.
pub const QUOTA_CODES: [&str; 3] = [
    "insufficient_quota",
    "quota_exceeded",
    "billing_hard_limit_reached",
];

/// Who answers chat, as the person knows it: Klipolo's server through the relay (an access code), Meta with their own
/// key.
fn who(relay: bool) -> &'static str {
    if relay { "Klipolo's server" } else { "Meta" }
}

/// A Meta client error in chat's terms, `relay` saying whether the call went through the relay. A 402, or a 429 whose
/// code says the quota is used up (`QUOTA_CODES`, a direct key's), keeps the relay's or Meta's sentence (which credit,
/// what to do). A timeout keeps its sentence, or says who gave up when there is none (`SERVER_BUSY` through the relay,
/// Meta's otherwise); busy and not answering say who too; a relay with no Meta key and a content-policy refusal say so
/// in their own sentence. A request refused as sent or an unreadable answer keeps its (redacted) text for logs only.
/// Through the relay, a sentence the server didn't give names Klipolo's (`MetaError::relay_user_message`): a bare 402
/// is the device's Klipolo credit.
pub fn model_error(e: MetaError, relay: bool) -> ModelError {
    let said = |e: &MetaError| {
        if relay {
            e.relay_user_message()
        } else {
            e.user_message()
        }
    };
    match e {
        MetaError::Budget { .. } => ModelError::Budget(said(&e)),
        MetaError::RateLimited {
            code: Some(code),
            message,
            ..
        } if QUOTA_CODES.contains(&code.as_str()) => {
            // A direct key's quota: Meta's credit.
            let message = message.unwrap_or_default();
            ModelError::Budget(MetaError::Budget { message }.user_message())
        }
        MetaError::RateLimited { .. } => ModelError::Busy(Some(format!(
            "{} is busy right now. Try again in a moment.",
            who(relay)
        ))),
        MetaError::Overloaded { .. } => ModelError::Unavailable(Some(format!(
            "{} isn't answering right now. Try again in a moment.",
            who(relay)
        ))),
        MetaError::TimedOut { message: None, .. } if relay => {
            ModelError::Unavailable(Some(SERVER_BUSY.to_owned()))
        }
        MetaError::TimedOut { .. } | MetaError::NotConfigured => {
            ModelError::Unavailable(Some(said(&e)))
        }
        MetaError::Refused { .. } => ModelError::Declined(said(&e)),
        MetaError::Network(_) => ModelError::Network,
        MetaError::Unauthorized | MetaError::NotAllowed { .. } => ModelError::Refused,
        MetaError::BadRequest { .. }
        | MetaError::Decode(_)
        | MetaError::AlignUnavailable
        | MetaError::LocalFile { .. } => ModelError::other(e.to_string()),
    }
}
