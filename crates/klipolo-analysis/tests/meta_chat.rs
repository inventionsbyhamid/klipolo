//! Meta's chat adapter end to end: the planner through `MetaChat` to a mock relay (wiremock), checking the exact body
//! sent (only the fields the relay takes, nothing stored, no reasoning replayed), how Meta's answers and errors come
//! back in chat's terms, and that chat holds its own slot, never one of the analysis's.
//!
//! The project is "talk.mov", 30 s, played whole: "Hello there friends." is said from 0.5 to 2.0 s and nothing after,
//! so a cut from 12.3 to 19.7 s stands on nothing and goes back to the model.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::meta_chat::{CHAT_MODEL, MetaChat, wire_body};
use klipolo_chat::{
    AnalysisState, ChatContext, ChatErrorKind, Inputs, Item, LanguageModel, ModelError,
    ModelRequest, Refs, View, build_context, plan, system_prompt, tools,
};
use klipolo_meta::pricing::{SPARK_CACHED_INPUT_USD, SPARK_INPUT_USD, SPARK_OUTPUT_USD};
use klipolo_meta::{MetaClient, MetaError, SecretString};
use klipolo_model::Project;
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "relay-chat-token-0123456789";
const RESPONSES: &str = "/meta/v1/responses";

/// The top-level fields the relay forwards (`services/relay`, its client contract).
const RELAY_FIELDS: [&str; 18] = [
    "model",
    "input",
    "instructions",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "text",
    "reasoning",
    "max_output_tokens",
    "temperature",
    "top_p",
    "stream",
    "store",
    "metadata",
    "user",
    "safety_identifier",
    "background",
    "service_tier",
];
/// The relay's most output tokens a call may ask for.
const RELAY_MAX_OUTPUT_TOKENS: u64 = 16_384;

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn word(text: &str, a: f64, b: f64) -> Word {
    Word {
        text: text.into(),
        start_us: secs(a),
        end_us: secs(b),
        confidence: 0.9,
        kind: WordKind::Word,
    }
}

/// "talk.mov", its speech, and what one message compiles against.
fn talk() -> (Project, Inputs, ChatContext) {
    let mut p = Project::new("Talk");
    p.assets = vec![Asset {
        id: "talk".into(),
        kind: AssetKind::Video,
        path: "/media/talk.mov".into(),
        name: "talk.mov".into(),
        duration_us: secs(30.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
    }];
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(30.0));
    clip.id = "t".into();
    p.timeline.main = vec![clip];
    let mut speech = Speech::default();
    speech.transcripts.insert(
        "talk".into(),
        Arc::new(Transcript {
            key: "key".into(),
            language: "en".into(),
            model: "test".into(),
            covered_us: secs(30.0),
            complete: true,
            words: vec![
                word("Hello", 0.5, 0.9),
                word("there", 1.0, 1.4),
                word("friends.", 1.5, 2.0),
            ],
        }),
    );
    let inputs = Inputs {
        message: String::new(),
        view: View::default(),
        refs: Refs::of(&p),
        speech,
        scenes: HashMap::new(),
        analysis: HashMap::from([("talk".to_owned(), AnalysisState::Done)]),
    };
    let ctx = build_context(
        &p,
        &inputs.speech,
        &HashMap::new(),
        &inputs.scenes,
        &inputs.view,
    );
    (p, inputs, ctx)
}

/// The adapter on a relay at `server`, in relay mode (as with an access code), waiting at most 1 ms when busy.
fn chat(server: &MockServer) -> MetaChat {
    MetaChat::new(client(server)).with_busy_wait_max(Duration::from_millis(1))
}

fn client(server: &MockServer) -> MetaClient {
    MetaClient::relay(SecretString::from(TOKEN))
        .with_relay_base(server.uri())
        .with_ledger(None)
}

/// A Responses API answer with `output` items and these token counts.
fn spark(output: Vec<Value>, input: u64, cached: u64, out: u64) -> Value {
    json!({
        "id": "resp_1",
        "object": "response",
        "status": "completed",
        "model": CHAT_MODEL,
        "output": output,
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "usage": {
            "input_tokens": input,
            "input_tokens_details": {"cached_tokens": cached},
            "output_tokens": out,
            "output_tokens_details": {"reasoning_tokens": out / 2},
            "total_tokens": input + out
        }
    })
}

fn reasoning() -> Value {
    json!({"type": "reasoning", "id": "rs_1", "summary": [], "status": "completed"})
}

fn message(text: &str) -> Value {
    json!({
        "type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": text, "annotations": []}]
    })
}

fn function_call(call_id: &str, name: &str, arguments: Value) -> Value {
    json!({
        "type": "function_call", "id": format!("fc_{call_id}"), "call_id": call_id, "name": name,
        "arguments": arguments.to_string(), "status": "completed"
    })
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

async fn answer_once(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(RESPONSES))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(response)
        .up_to_n_times(1)
        .mount(server)
        .await;
}

async fn bodies(server: &MockServer) -> Vec<Value> {
    let requests: Vec<Request> = server.received_requests().await.unwrap();
    (requests.iter())
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

/// Every JSON object inside `value`, at any depth.
fn objects(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
    let mut out = Vec::new();
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        match v {
            Value::Object(map) => {
                out.push(map);
                stack.extend(map.values());
            }
            Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    out
}

/// What the relay takes from a client, and counts as text only (no media reservation): its fields, the output cap, no
/// stored context or reasoning, and only text items.
fn assert_relay_takes(body: &Value) {
    let fields: Vec<&String> = body.as_object().unwrap().keys().collect();
    for f in &fields {
        assert!(RELAY_FIELDS.contains(&f.as_str()), "the relay refuses {f}");
    }
    for f in [
        "include",
        "truncation",
        "previous_response_id",
        "conversation",
    ] {
        assert!(body.get(f).is_none(), "{f} is sent");
    }
    // Default reasoning effort: the field is left out.
    assert!(body.get("reasoning").is_none());
    let max = body["max_output_tokens"].as_u64().expect("an output cap");
    assert!((1..=RELAY_MAX_OUTPUT_TOKENS).contains(&max), "{max}");
    // Room for long reasoning: billed only when used.
    assert_eq!(max, 8192);
    assert_eq!(body["model"], CHAT_MODEL);
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["parallel_tool_calls"], true);
    assert_eq!(body["instructions"], system_prompt());
    let sent = body["tools"].as_array().unwrap();
    let names: Vec<&str> = sent.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let neutral = tools();
    let expected: Vec<&str> = (neutral.as_array().unwrap().iter())
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, expected);
    for (tool, neutral) in sent.iter().zip(neutral.as_array().unwrap()) {
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["strict"], true);
        assert_eq!(tool["parameters"], neutral["parameters"]);
        assert_eq!(tool["description"], neutral["description"]);
    }
    // Nothing anywhere is a reasoning item, a stored item or a file.
    for object in objects(body) {
        let kind = object.get("type").and_then(Value::as_str);
        assert_ne!(kind, Some("reasoning"), "{object:?}");
        assert_ne!(kind, Some("item_reference"), "{object:?}");
        assert!(!object.contains_key("file_id"), "{object:?}");
    }
    for item in body["input"].as_array().unwrap() {
        match item["type"].as_str().unwrap() {
            "message" => {
                let role = item["role"].as_str().unwrap();
                let part = if role == "assistant" {
                    "output_text"
                } else {
                    "input_text"
                };
                assert!(role == "user" || role == "assistant", "{item}");
                for p in item["content"].as_array().unwrap() {
                    assert_eq!(p["type"], part, "{item}");
                    assert!(p["text"].is_string());
                }
            }
            "function_call" => {
                // A replayed call names no stored item: no `id`.
                assert!(item.get("id").is_none(), "{item}");
                for k in ["call_id", "name", "arguments"] {
                    assert!(item[k].is_string(), "{item}");
                }
            }
            "function_call_output" => {
                assert!(
                    item["call_id"].is_string() && item["output"].is_string(),
                    "{item}"
                );
            }
            other => panic!("an item the relay counts as media: {other}"),
        }
    }
}

fn user_text(item: &Value) -> &str {
    assert_eq!(
        (&item["type"], &item["role"]),
        (&json!("message"), &json!("user"))
    );
    item["content"][0]["text"].as_str().unwrap()
}

#[tokio::test]
async fn the_wire_body_holds_only_what_the_relay_takes_through_a_refusal_round() {
    let server = MockServer::start().await;
    answer_once(
        &server,
        ok(spark(
            vec![
                reasoning(),
                message("Cutting the quiet part \u{2014} done."),
                function_call(
                    "call_1",
                    "cutRange",
                    json!({"startSec": 12.3, "endSec": 19.7}),
                ),
            ],
            1800,
            1700,
            300,
        )),
    )
    .await;
    answer_once(
        &server,
        ok(spark(
            vec![
                reasoning(),
                message("I'll cut \u{201c}there\u{201d}."),
                function_call(
                    "call_2",
                    "cutRange",
                    json!({"startSec": 1.0, "endSec": 1.4}),
                ),
            ],
            2000,
            1800,
            200,
        )),
    )
    .await;
    let (p, inputs, ctx) = talk();
    let out = plan(&chat(&server), &[], &ctx, "cut there", &inputs, &p)
        .await
        .unwrap();

    let sent = bodies(&server).await;
    assert_eq!(sent.len(), 2);
    for body in &sent {
        assert_relay_takes(body);
    }
    // The context first, then the message.
    let first = sent[0]["input"].as_array().unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(user_text(&first[0]), ctx.text);
    assert_eq!(user_text(&first[1]), "cut there");
    // The refusal round sends the whole first input again, then the answer's text and call, and the call's output.
    let second = sent[1]["input"].as_array().unwrap();
    assert_eq!(&second[..2], &first[..]);
    assert_eq!(second.len(), 5);
    assert_eq!(second[2]["role"], "assistant");
    assert_eq!(
        second[2]["content"][0]["text"],
        "Cutting the quiet part \u{2014} done."
    );
    assert_eq!(
        second[3],
        json!({
            "type": "function_call", "call_id": "call_1", "name": "cutRange",
            "arguments": json!({"startSec": 12.3, "endSec": 19.7}).to_string()
        })
    );
    assert_eq!(second[4]["type"], "function_call_output");
    assert_eq!(second[4]["call_id"], "call_1");
    assert!(
        (second[4]["output"].as_str().unwrap())
            .starts_with("cutRange: startSec 12.3 is not within 0.3 s"),
        "{}",
        second[4]
    );

    let proposal = out.compiled.proposal.expect("the cut is proposed");
    assert_eq!(proposal.ranges.len(), 1);
    assert!(proposal.ranges[0].start <= secs(1.0) && proposal.ranges[0].end >= secs(1.4));
    assert_eq!(out.reply, "I'll cut \u{201c}there\u{201d}.");
    // Each call priced from its usage, cached input at the cached price.
    let expected = spark_cost(1800.0, 1700.0, 300.0) + spark_cost(2000.0, 1800.0, 200.0);
    assert!(
        (out.usage_usd - expected).abs() < 1e-12,
        "{} vs {expected}",
        out.usage_usd
    );
}

#[tokio::test]
async fn the_cached_prefix_is_the_same_bytes_on_the_wire_for_two_messages() {
    let server = MockServer::start().await;
    for text in ["Which part?", "Done."] {
        answer_once(&server, ok(spark(vec![message(text)], 1800, 1700, 100))).await;
    }
    let (p, inputs, ctx) = talk();
    let model = chat(&server);
    let first = plan(&model, &[], &ctx, "cut the boring bit", &inputs, &p)
        .await
        .unwrap();
    let history = vec![first.turn("cut the boring bit")];
    plan(&model, &history, &ctx, "make it square", &inputs, &p)
        .await
        .unwrap();
    let sent = bodies(&server).await;
    let bytes = |v: &Value| serde_json::to_string(v).unwrap();
    for field in ["instructions", "tools"] {
        assert_eq!(bytes(&sent[0][field]), bytes(&sent[1][field]), "{field}");
    }
    assert_eq!(bytes(&sent[0]["input"][0]), bytes(&sent[1]["input"][0]));
    // The conversation after the context, the new message last.
    let input = sent[1]["input"].as_array().unwrap();
    assert_eq!(input.len(), 4);
    assert_eq!(user_text(&input[1]), "cut the boring bit");
    assert_eq!(input[2]["role"], "assistant");
    assert_eq!(user_text(&input[3]), "make it square");
}

#[test]
fn the_body_is_built_from_the_neutral_request_alone() {
    let request = ModelRequest {
        system: "Be brief.".into(),
        tools: json!([{"name": "undo", "description": "Undo.", "parameters": {"type": "object"}}]),
        input: vec![
            Item::Context {
                text: "## Project".into(),
            },
            Item::User { text: "hi".into() },
            Item::Assistant {
                text: "Hello.".into(),
            },
        ],
    };
    let body = wire_body(&request);
    assert_eq!(body["instructions"], "Be brief.");
    assert_eq!(
        body["tools"],
        json!([{"type": "function", "name": "undo", "description": "Undo.",
                "parameters": {"type": "object"}, "strict": true}])
    );
    assert_eq!(
        body["input"],
        json!([
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "## Project"}]},
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hello."}]}
        ])
    );
}

#[tokio::test]
async fn budget_402_keeps_the_relays_sentence_and_changes_nothing() {
    let server = MockServer::start().await;
    let said = "This device has used its Klipolo credit.";
    answer_once(
        &server,
        ResponseTemplate::new(402).set_body_json(json!({"error": "budget", "message": said})),
    )
    .await;
    let (p, inputs, ctx) = talk();
    let before = p.clone();
    let e = plan(&chat(&server), &[], &ctx, "make it square", &inputs, &p)
        .await
        .unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::Budget);
    assert_eq!(e.message, said);
    assert_eq!(e.usage_usd, 0.0);
    assert_eq!(p, before);
    assert_eq!(bodies(&server).await.len(), 1, "a 402 is final");
}

fn busy() -> ResponseTemplate {
    ResponseTemplate::new(429)
        .insert_header("Retry-After", "1")
        .set_body_json(
            json!({"error": "busy", "message": "Too many requests at once from this device."}),
        )
}

fn neutral_request() -> ModelRequest {
    ModelRequest {
        system: "Be brief.".into(),
        tools: json!([]),
        input: vec![Item::User { text: "hi".into() }],
    }
}

/// `spark` with another `status`, and these fields beside it.
fn ended(status: &str, extra: Value, output: Vec<Value>) -> Value {
    let mut body = spark(output, 1800, 1700, 4096);
    body["status"] = json!(status);
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

fn spark_cost(input: f64, cached: f64, output: f64) -> f64 {
    (input - cached) * SPARK_INPUT_USD + cached * SPARK_CACHED_INPUT_USD + output * SPARK_OUTPUT_USD
}

#[tokio::test]
async fn a_cut_off_answer_is_an_error_that_still_counts() {
    let server = MockServer::start().await;
    // Cut off at the output cap: its call may be partial, so nothing of it is used, and no refusal round is asked.
    answer_once(
        &server,
        ok(ended(
            "incomplete",
            json!({"incomplete_details": {"reason": "max_output_tokens"}}),
            vec![
                reasoning(),
                function_call(
                    "call_1",
                    "cutRange",
                    json!({"startSec": 12.3, "endSec": 19.7}),
                ),
            ],
        )),
    )
    .await;
    let (p, inputs, ctx) = talk();
    let before = p.clone();
    let e = plan(&chat(&server), &[], &ctx, "cut there", &inputs, &p)
        .await
        .unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::Other);
    assert_eq!(e.detail, "failed: cut off: max_output_tokens");
    assert!((e.usage_usd - spark_cost(1800.0, 1700.0, 4096.0)).abs() < 1e-12);
    assert_eq!(p, before);
    assert_eq!(bodies(&server).await.len(), 1);
}

#[tokio::test]
async fn a_failed_answer_is_an_error_that_still_counts() {
    let server = MockServer::start().await;
    answer_once(
        &server,
        ok(ended(
            "failed",
            json!({"error": {"code": "server_error", "message": "Something broke."}}),
            vec![],
        )),
    )
    .await;
    let e = chat(&server).respond(&neutral_request()).await.unwrap_err();
    assert_eq!(
        e,
        ModelError::Other {
            detail: "answer failed: server_error".into(),
            usage_usd: spark_cost(1800.0, 1700.0, 4096.0),
        }
    );
}

#[tokio::test]
async fn busy_is_asked_again_twice_then_reported() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RESPONSES))
        .respond_with(busy())
        .mount(&server)
        .await;
    let e = chat(&server).respond(&neutral_request()).await.unwrap_err();
    assert_eq!(e, ModelError::Busy);
    assert_eq!(bodies(&server).await.len(), 3, "the first try and two more");
}

#[tokio::test]
async fn a_plain_text_429_is_busy_and_asked_again() {
    let server = MockServer::start().await;
    // Traefik's rate limit answers in plain text, not the relay's JSON.
    answer_once(
        &server,
        ResponseTemplate::new(429).set_body_raw("Too Many Requests", "text/plain"),
    )
    .await;
    answer_once(&server, busy()).await;
    answer_once(&server, ok(spark(vec![message("Hi.")], 10, 0, 5))).await;
    let answer = chat(&server).respond(&neutral_request()).await.unwrap();
    assert_eq!(answer.text, "Hi.");
    assert_eq!(bodies(&server).await.len(), 3);
}

#[tokio::test]
async fn a_rate_limit_that_is_not_busy_is_not_asked_again() {
    let server = MockServer::start().await;
    answer_once(
        &server,
        ResponseTemplate::new(429)
            .insert_header("Retry-After", "30")
            .set_body_json(json!({"error": "meta_error", "message": "Rate limit reached."})),
    )
    .await;
    let e = chat(&server).respond(&neutral_request()).await.unwrap_err();
    assert_eq!(e, ModelError::Busy);
    assert_eq!(bodies(&server).await.len(), 1);
}

#[tokio::test]
async fn server_and_network_failures_map_to_neutral_errors() {
    let cases = [
        (
            ResponseTemplate::new(503).set_body_json(
                json!({"error": "not_configured", "message": "Relay not configured."}),
            ),
            ModelError::Unavailable,
        ),
        (ResponseTemplate::new(502), ModelError::Unavailable),
        (ResponseTemplate::new(401), ModelError::Refused),
        (
            ResponseTemplate::new(403).set_body_json(
                json!({"error": "not_allowed", "message": "This access code is not valid."}),
            ),
            ModelError::Refused,
        ),
    ];
    for (response, expected) in cases {
        let server = MockServer::start().await;
        answer_once(&server, response).await;
        let e = chat(&server).respond(&neutral_request()).await.unwrap_err();
        assert_eq!(e, expected);
    }
    // A request refused as sent is a failure with a detail for logs, never the token.
    let server = MockServer::start().await;
    answer_once(
        &server,
        ResponseTemplate::new(400).set_body_json(json!({
            "error": "unsupported",
            "message": format!("The relay does not accept these fields: include. {TOKEN}")
        })),
    )
    .await;
    let e = chat(&server).respond(&neutral_request()).await.unwrap_err();
    let ModelError::Other { detail, .. } = e else {
        panic!("{e:?}");
    };
    assert!(detail.contains("include"), "{detail}");
    assert!(!detail.contains(TOKEN), "{detail}");
    // Nothing answers at all: a port that was free a moment ago (a dropped mock server goes back to wiremock's pool
    // and still answers).
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let model = MetaChat::new(
        MetaClient::relay(SecretString::from(TOKEN))
            .with_relay_base(format!("http://127.0.0.1:{port}"))
            .with_ledger(None),
    );
    let e = model.respond(&neutral_request()).await.unwrap_err();
    assert_eq!(e, ModelError::Network);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_holds_its_own_slot_never_the_analysiss() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RESPONSES))
        .respond_with(
            ok(spark(vec![message("Hi.")], 10, 0, 5)).set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    // The analysis's three relay slots are all taken by calls that never end.
    let meta = Meta::new(client(&server));
    for _ in 0..3 {
        let meta = meta.clone();
        tokio::spawn(async move {
            meta.call(std::future::pending::<Result<(), MetaError>>())
                .await
        });
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let model = chat(&server);
    let request = neutral_request();
    let answered = tokio::time::timeout(Duration::from_secs(10), model.respond(&request)).await;
    assert_eq!(
        answered
            .expect("chat never waits for the analysis")
            .unwrap()
            .text,
        "Hi."
    );

    // Two chat messages at once go one after the other: chat holds one of the device's four relay calls.
    let (a, b) = (model.clone(), model.clone());
    let started = Instant::now();
    let (ra, rb) = tokio::join!(a.respond(&request), b.respond(&request));
    assert!(ra.is_ok() && rb.is_ok());
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "{:?}",
        started.elapsed()
    );
}
