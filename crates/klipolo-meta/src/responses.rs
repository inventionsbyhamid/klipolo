//! The Responses API (`POST /responses`): Muse Spark and SAM, as one JSON answer (`respond`) or read from its
//! server-sent events to the end (`respond_stream`, for a call that may take minutes).

use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::network_error;
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
    /// The `output_text` parts of each `message` item joined, with a newline
    /// between items; none when the model only called tools.
    pub output_text: Option<String>,
    /// The `function_call` items, in order.
    pub function_calls: Vec<FunctionCall>,
    pub usage: Usage,
    /// The answer's `status`: `completed`, or `incomplete` (cut off, e.g. at
    /// `max_output_tokens`, so structured output may be partial) or `failed`.
    pub status: Option<String>,
    /// The whole answer, for fields not parsed here (`incomplete_details`, SAM's text...).
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

    /// [`Self::respond`] for a call that may take minutes, such as a SAM video: sent with `stream: true` and read as
    /// server-sent events to its `response.completed` (or `response.incomplete`), whose answer is parsed as
    /// `respond`'s is. The text deltas stand in for an answer that carries no output of its own. Klipolo's relay
    /// forwards SAM video calls only streamed, so it can read them to their final usage.
    pub async fn respond_stream(&self, mut body: Value) -> Result<ResponseOut, MetaError> {
        if let Some(fields) = body.as_object_mut() {
            fields.insert("stream".into(), Value::Bool(true));
        }
        let request = self
            .http
            .post(self.meta_url("responses"))
            .header(ACCEPT, "text/event-stream")
            .json(&body);
        let mut response = self.send_meta(request).await?;
        let mut events = Events::default();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| network_error(&e, &self.secrets()))?
        {
            events.read(&chunk)?;
        }
        let out = events.finish()?;
        self.record_response(&body, &out);
        Ok(out)
    }

    /// A ledger row for a Spark, Wonder (`pricing::chat_usd`) or SAM answer that reports its usage.
    fn record_response(&self, body: &Value, out: &ResponseOut) {
        if !out.raw.get("usage").is_some_and(Value::is_object) {
            return;
        }
        let model = (out.raw.get("model").or_else(|| body.get("model")))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let usage = out.usage;
        let row = if model.starts_with("muse-spark-") || model.starts_with("muse-wonder-") {
            Row {
                model,
                purpose: "responses",
                input_tokens: Some(usage.input_tokens),
                output_tokens: Some(usage.output_tokens),
                frames_or_seconds: String::new(),
                cost_usd: pricing::chat_usd(model, usage),
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
        // One entry per message item that has text.
        let mut texts: Vec<String> = Vec::new();
        let mut function_calls = Vec::new();
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let parts = item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten();
                    let mut text_parts = parts
                        .filter(|p| p.get("type").and_then(Value::as_str) == Some("output_text"))
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .peekable();
                    if text_parts.peek().is_some() {
                        texts.push(text_parts.collect());
                    }
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
            Some(texts.join("\n"))
        };
        let usage = raw.get("usage").map(usage).unwrap_or_default();
        let status = raw.get("status").and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            output_text,
            function_calls,
            usage,
            status,
            raw,
        })
    }
}

/// A streamed answer as its events arrive: the text so far, and the final answer once it comes.
#[derive(Default)]
struct Events {
    /// Bytes of a line not ended yet.
    partial: Vec<u8>,
    /// The `data:` lines of the event not ended yet (a blank line ends one): an event may spread over several.
    data: Vec<String>,
    text: String,
    done: Option<Value>,
}

impl Events {
    /// The next bytes of the stream: every whole event in them read.
    fn read(&mut self, bytes: &[u8]) -> Result<(), MetaError> {
        self.partial.extend_from_slice(bytes);
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            self.line(&line)?;
        }
        Ok(())
    }

    /// One line: a `data:` line is kept for its event, and a blank line ends the event. Other fields (`event:`, `id:`)
    /// and comments are passed over.
    fn line(&mut self, line: &[u8]) -> Result<(), MetaError> {
        let line = String::from_utf8_lossy(line);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return self.event();
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
        }
        Ok(())
    }

    /// The event its `data:` lines make, joined by newlines.
    fn event(&mut self) -> Result<(), MetaError> {
        let data = std::mem::take(&mut self.data).join("\n");
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(());
        }
        let event: Value = serde_json::from_str(data)
            .map_err(|e| MetaError::Decode(format!("a streamed event: {e}")))?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.text.push_str(delta);
                }
            }
            Some("response.completed" | "response.incomplete") => {
                self.done = event.get("response").cloned();
            }
            Some("response.failed" | "error") => {
                let message = (event.pointer("/response/error/message"))
                    .or_else(|| event.pointer("/error/message"))
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                return Err(MetaError::Refused { message });
            }
            _ => {}
        }
        Ok(())
    }

    /// The final answer, its text from the deltas when it carries none.
    fn finish(mut self) -> Result<ResponseOut, MetaError> {
        let rest = std::mem::take(&mut self.partial);
        self.line(&rest)?;
        // A stream that ends without a blank line still ends its last event.
        self.event()?;
        let raw = self
            .done
            .take()
            .ok_or_else(|| MetaError::Decode("the stream ended before its answer".into()))?;
        let mut out = ResponseOut::parse(raw)?;
        if out.output_text.is_none() && !self.text.is_empty() {
            out.output_text = Some(self.text);
        }
        Ok(out)
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
