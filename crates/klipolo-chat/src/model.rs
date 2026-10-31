//! The language model behind chat, as an interface this crate owns: whichever provider runs it, the planner sends a
//! `ModelRequest` and reads a `ModelResponse` or a `ModelError`. A provider's adapter (Meta's lives with the other
//! providers, in klipolo-analysis) maps these into its own wire shape and maps its answers and errors back, so no
//! vendor type reaches the planner. Tests use a fake.
//!
//! A request is the system text, the tools (`tools::tools`, provider-neutral JSON) and an ordered list of input items:
//! the project context first, then the conversation, then the new message, and in a refusal round the first answer's
//! function calls with their outputs. The system text, the tools and the context come first and keep their bytes from
//! one message to the next on the same project revision, so a provider's prompt cache can reuse them.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::FunctionCall;

/// A boxed future that may move between threads, so `LanguageModel` works as a `dyn` object.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A language model that calls tools.
pub trait LanguageModel: Send + Sync {
    /// The model's answer to `request`: one call to the model (an adapter may send it again while the provider is
    /// busy for a moment, never more than a couple of times).
    fn respond<'a>(
        &'a self,
        request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>>;
}

/// What the model is sent: all of it, every time. Nothing is stored with the provider between calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// What the model is told before anything else (`prompt::system_prompt`).
    pub system: String,
    /// The tools it may call, `[{name, description, parameters}]` (`tools::tools`).
    pub tools: Value,
    /// In order: the context, the conversation, the new message, then any calls and their outputs.
    pub input: Vec<Item>,
}

/// One input item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Item {
    /// The project as the model reads it (`context::build_context`): sent as the person's, before the conversation.
    Context { text: String },
    /// What the person wrote.
    User { text: String },
    /// What the model said back, with a note of what came of it.
    Assistant { text: String },
    /// A call the model made in an earlier answer to this message.
    FunctionCall(FunctionCall),
    /// What came of that call: `call_id` is the call's own.
    FunctionCallOutput { call_id: String, output: String },
}

/// The model's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelResponse {
    /// What it wrote for the person; empty when it only called tools.
    pub text: String,
    /// Its tool calls, in order.
    pub calls: Vec<FunctionCall>,
    /// What the call cost, in US dollars.
    pub usage_usd: f64,
}

/// Why the model gave no answer, in terms that decide what the person is told.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelError {
    /// The credit for these calls is used up: the provider's own sentence for the person (which credit, what to do).
    Budget(String),
    /// Too many requests right now, still so after the adapter waited and asked again.
    Busy,
    /// The service is down, not set up, or gave no answer in time: the provider's sentence for the person when it has
    /// one (who gave up, what to do).
    Unavailable(Option<String>),
    /// The request never got an answer.
    Network,
    /// The key, token or access code was refused.
    Refused,
    /// The provider turned this request down under its content policy: its sentence for the person.
    Declined(String),
    /// Anything else: a request refused as sent, an answer that can't be read, or one that came back failed or cut
    /// off (at its output cap), which is billed (`usage_usd`). `detail` is for logs, never shown.
    Other { detail: String, usage_usd: f64 },
}

impl ModelError {
    /// `Other` for a call that cost nothing.
    pub fn other(detail: impl Into<String>) -> Self {
        ModelError::Other {
            detail: detail.into(),
            usage_usd: 0.0,
        }
    }

    /// What the failed call still cost, in US dollars: an answer that came back failed or cut off is billed.
    pub fn usage_usd(&self) -> f64 {
        match self {
            ModelError::Other { usage_usd, .. } => *usage_usd,
            _ => 0.0,
        }
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Budget(said) => write!(f, "out of credit: {said}"),
            ModelError::Busy => f.write_str("busy"),
            ModelError::Unavailable(None) => f.write_str("unavailable"),
            ModelError::Unavailable(Some(said)) => write!(f, "unavailable: {said}"),
            ModelError::Network => f.write_str("no answer"),
            ModelError::Refused => f.write_str("not allowed"),
            ModelError::Declined(said) => write!(f, "declined: {said}"),
            ModelError::Other { detail, .. } => write!(f, "failed: {detail}"),
        }
    }
}

impl std::error::Error for ModelError {}
