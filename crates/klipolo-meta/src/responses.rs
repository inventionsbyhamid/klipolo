//! The Responses API (`POST /responses`), non-streaming: Muse Spark and SAM.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ledger::Row;
use crate::{pricing, MetaClient, MetaError};

/// What one call used, as Meta bills it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// All input tokens, cached ones included.
    pub input_tokens: u64,
    /// The part of `input_tokens` served from Meta's cache (cheaper).
    pub cached_tokens: u64,
    /// All output tokens, reasoning included.
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// SAM: frames billed for a video.
    pub video_frames_processed: u64,
}

/// One tool call the model made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Echo this back in the `function_call_output` that answers the call.
    pub call_id: String,
    /// The arguments as the JSON text the model wrote.
    pub arguments: String,
}

/// A parsed Responses API answer.
#[derive(Clone, Debug, PartialEq)]
pub struct ResponseOut {
    /// All `output_text` parts of the `message` items, joined; none when the
    /// model only called tools.
    pub output_text: Option<String>,
    /// The `function_call` items, in order.
    pub function_calls: Vec<FunctionCall>,
    pub usage: Usage,
    /// The whole answer, for fields not parsed here (`status`, SAM's text...).
    pub raw: Value,
}

impl MetaClient {
    /// Sends `body` to the Responses API exactly as given (the caller builds
    /// model, input, tools, `text.format`...) and parses the answer. Do not
    /// set `stream`: this reads one JSON answer.
    pub async fn respond(&self, body: Value) -> Result<ResponseOut, MetaError> {
        let request = self.http.post(self.meta_url("responses")).json(&body);
        let response = self.send_meta(request).await?;
        let raw: Value = self.read_json(response).await?;
        let out = ResponseOut::parse(raw)?;
        self.record_response(&body, &out);
        Ok(out)
    }

    /// A ledger row for a Spark or SAM answer that reports its usage.
    fn record_response(&self, body: &Value, out: &ResponseOut) {
        if !out.raw.get("usage").is_some_and(Value::is_object) {
            return;
        }
        let model = (out.raw.get("model").or_else(|| body.get("model")))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let usage = out.usage;
        let row = if model.starts_with("muse-spark-") {
            Row {
                model,
                purpose: "responses",
                input_tokens: Some(usage.input_tokens),
                output_tokens: Some(usage.output_tokens),
                frames_or_seconds: String::new(),
                cost_usd: pricing::spark_usd(usage),
            }
        } else if model.starts_with("sam-") {
            Row {
                model,
                purpose: "responses",
                input_tokens: None,
                output_tokens: None,
                frames_or_seconds: format!("{} frames", usage.video_frames_processed),
                cost_usd: pricing::sam_usd(usage),
            }
        } else {
            // No list price here for other models.
            return;
        };
        self.record(row);
    }
}

impl ResponseOut {
    /// Parses a Responses API answer (also for saved answers and fixtures).
    pub fn parse(raw: Value) -> Result<Self, MetaError> {
        if !raw.is_object() {
            return Err(MetaError::Decode("a response is a JSON object".into()));
        }
        let items = raw
            .get("output")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut texts = Vec::new();
        let mut function_calls = Vec::new();
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let parts = item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten();
                    texts.extend(
                        parts
                            .filter(|p| {
                                p.get("type").and_then(Value::as_str) == Some("output_text")
                            })
                            .filter_map(|p| p.get("text").and_then(Value::as_str)),
                    );
                }
                Some("function_call") => function_calls.push(function_call(item)?),
                _ => {}
            }
        }
        let output_text = if texts.is_empty() {
            raw.get("output_text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            Some(texts.concat())
        };
        let usage = raw.get("usage").map(usage).unwrap_or_default();
        Ok(Self {
            output_text,
            function_calls,
            usage,
            raw,
        })
    }
}

fn function_call(item: &Value) -> Result<FunctionCall, MetaError> {
    let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
    let (Some(name), Some(arguments)) = (text("name"), text("arguments")) else {
        return Err(MetaError::Decode(
            "a function call has no name or arguments".into(),
        ));
    };
    let call_id = text("call_id").or_else(|| text("id")).unwrap_or_default();
    Ok(FunctionCall {
        name,
        call_id,
        arguments,
    })
}

fn usage(usage: &Value) -> Usage {
    let count = |value: Option<&Value>| value.and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: count(usage.get("input_tokens")),
        cached_tokens: count(usage.pointer("/input_tokens_details/cached_tokens")),
        output_tokens: count(usage.get("output_tokens")),
        reasoning_tokens: count(usage.pointer("/output_tokens_details/reasoning_tokens")),
        video_frames_processed: count(usage.get("video_frames_processed")),
    }
}
