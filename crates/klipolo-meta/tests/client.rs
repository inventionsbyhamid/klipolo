//! The client against a local mock server. No test reaches Meta or the relay:
//! every client here points both base URLs at a mock or at a closed port.

use std::path::Path;

use klipolo_meta::{Auth, MetaClient, MetaError, ResponseOut, SecretString, Usage};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "sk-meta-test-key-0123456789";
const TOKEN: &str = "relay-test-token-abcdefghij";
/// Nothing listens on the discard port, so a call sent here fails at once.
const CLOSED: &str = "http://127.0.0.1:9";
const BUDGET_MESSAGE: &str =
    "This device has used its Klipolo credit. Add your own Meta key in Settings.";

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

/// The person's own key, with Meta mocked at `{server}/v1`.
fn direct(server: &MockServer) -> MetaClient {
    MetaClient::direct(secret(KEY))
        .with_meta_base(format!("{}/v1", server.uri()))
        .with_relay_base(CLOSED)
}

/// A relay device token, with the relay mocked at `{server}`.
fn relay(server: &MockServer) -> MetaClient {
    MetaClient::relay(secret(TOKEN))
        .with_relay_base(server.uri())
        .with_meta_base(CLOSED)
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn json_body(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

/// Stand-in audio: the client sends the bytes as they are.
fn wav() -> Vec<u8> {
    b"RIFF\x24\x00\x00\x00WAVEfmt test audio".to_vec()
}

/// The headers and bytes of the multipart part called `name`.
fn part(request: &Request, name: &str) -> (String, Vec<u8>) {
    let body = &request.body;
    let marker = format!("name=\"{name}\"");
    let start = find(body, marker.as_bytes()).unwrap_or_else(|| panic!("no part {name}"));
    let headers_end = start + find(&body[start..], b"\r\n\r\n").unwrap();
    let data_start = headers_end + 4;
    let data_end = data_start + find(&body[data_start..], b"\r\n--").unwrap();
    let headers = String::from_utf8_lossy(&body[start..headers_end]).into_owned();
    (headers, body[data_start..data_end].to_vec())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn only_request(server: &MockServer) -> Request {
    let mut requests = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1, "expected one request");
    requests.remove(0)
}

#[test]
fn is_relay_follows_auth() {
    assert!(!MetaClient::direct(secret(KEY)).is_relay());
    assert!(MetaClient::relay(secret(TOKEN)).is_relay());
}

#[test]
fn debug_output_redacts_secrets() {
    let direct = MetaClient::direct(secret(KEY)).with_align_token(secret(TOKEN));
    let relay = MetaClient::relay(secret(TOKEN));
    let auth = Auth::Key(secret(KEY));
    for text in [
        format!("{direct:?}"),
        format!("{relay:?}"),
        format!("{auth:?}"),
    ] {
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.contains(TOKEN), "{text}");
    }
}

#[test]
fn user_messages_are_plain_and_name_settings_where_needed() {
    let errors = [
        MetaError::Budget {
            message: "This device has used its Klipolo credit \u{2014} add a key.".into(),
        },
        MetaError::NotAllowed,
        MetaError::Unauthorized,
        MetaError::RateLimited {
            retry_after: Some(7),
        },
        MetaError::RateLimited { retry_after: None },
        MetaError::Overloaded,
        MetaError::BadRequest {
            message: "Invalid audio \u{2013} expected WAV".into(),
        },
        MetaError::Network("connection refused".into()),
        MetaError::AlignUnavailable,
        MetaError::Decode("expected value at line 1".into()),
    ];
    for error in &errors {
        let text = error.user_message();
        assert!(!text.is_empty());
        assert!(
            !text.contains('\u{2014}') && !text.contains('\u{2013}'),
            "dash in {text:?}"
        );
    }
    assert!(MetaError::Unauthorized.user_message().contains("Settings"));
    assert!(MetaError::RateLimited {
        retry_after: Some(7)
    }
    .user_message()
    .contains("7 seconds"));
}

// Transcription.

#[tokio::test]
async fn transcribe_parses_spike_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(json_body(&fixture("asr_diarization.json")))
        .mount(&server)
        .await;

    let result = direct(&server).transcribe(wav(), &[]).await.unwrap();

    assert_eq!(result.duration_ms, 155_040);
    assert_eq!(result.turns.len(), 9);
    let first = &result.turns[0];
    assert_eq!((first.start_ms, first.end_ms), (60, 10_780));
    assert_eq!(first.speaker.as_deref(), Some("A"));
    assert!(first.text.starts_with("But it's also also super rewarding"));
    assert_eq!(result.turns[8].speaker.as_deref(), Some("F"));
}

#[tokio::test]
async fn transcribe_sends_a_diarization_request_with_keywords() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(json_body(&fixture("asr_diarization.json")))
        .mount(&server)
        .await;

    let keywords = ["Kaiden".to_owned(), "Onishi".to_owned()];
    direct(&server).transcribe(wav(), &keywords).await.unwrap();

    let request = only_request(&server).await;
    let session = request
        .url
        .query_pairs()
        .find(|(k, _)| k == "sessionId")
        .map(|(_, v)| v.into_owned())
        .expect("sessionId query parameter");
    assert!(
        session.starts_with("klipolo-") && session.len() > 20,
        "{session}"
    );

    let (headers, spec) = part(&request, "request");
    assert!(
        headers
            .to_lowercase()
            .contains("content-type: application/json"),
        "{headers}"
    );
    let spec: Value = serde_json::from_slice(&spec).unwrap();
    assert_eq!(
        spec,
        json!({
            "mode": "DIARIZATION",
            "model": "muse-voice-transcribe-1.0",
            "audioEncoding": "WAV",
            "languageBias": ["English"],
            "keywords": ["Kaiden", "Onishi"],
        })
    );
    let (headers, audio) = part(&request, "audio");
    assert!(headers.contains("filename=\"audio.wav\""), "{headers}");
    assert!(
        headers.to_lowercase().contains("content-type: audio/wav"),
        "{headers}"
    );
    assert_eq!(audio, wav());
}

#[tokio::test]
async fn transcribe_without_keywords_leaves_them_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_body(&fixture("asr_diarization.json")))
        .mount(&server)
        .await;

    direct(&server).transcribe(wav(), &[]).await.unwrap();

    let (_, spec) = part(&only_request(&server).await, "request");
    let spec: Value = serde_json::from_slice(&spec).unwrap();
    assert!(spec.get("keywords").is_none(), "{spec}");
}

// Modes.

#[tokio::test]
async fn direct_mode_sends_bearer_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(json_body(&fixture("asr_diarization.json")))
        .expect(1)
        .mount(&server)
        .await;

    let client = direct(&server);
    assert!(!client.is_relay());
    client.transcribe(wav(), &[]).await.unwrap();
}

#[tokio::test]
async fn relay_mode_prefixes_meta_v1_and_sends_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/asr/transcribe"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(json_body(&fixture("asr_diarization.json")))
        .expect(1)
        .mount(&server)
        .await;

    let client = relay(&server);
    assert!(client.is_relay());
    let result = client.transcribe(wav(), &[]).await.unwrap();
    assert_eq!(result.turns.len(), 9);
}

// Status codes.

async fn transcribe_error(response: ResponseTemplate) -> MetaError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response)
        .mount(&server)
        .await;
    relay(&server).transcribe(wav(), &[]).await.unwrap_err()
}

#[tokio::test]
async fn status_402_maps_to_budget_with_relay_message() {
    let error = transcribe_error(
        ResponseTemplate::new(402)
            .set_body_json(json!({"error": "budget", "message": BUDGET_MESSAGE})),
    )
    .await;

    assert_eq!(
        error,
        MetaError::Budget {
            message: BUDGET_MESSAGE.into()
        }
    );
    assert_eq!(error.user_message(), BUDGET_MESSAGE);
}

#[tokio::test]
async fn status_503_maps_to_overloaded() {
    let meta_overloaded = json!({"error": {
        "code": "service_overloaded",
        "message": "The backend is temporarily overloaded. Please retry.",
        "param": null,
        "type": "server_error",
    }});
    let error = transcribe_error(ResponseTemplate::new(503).set_body_json(meta_overloaded)).await;
    assert_eq!(error, MetaError::Overloaded);

    let relay_off = json!({"error": "not_configured", "message": "Relay not configured."});
    let error = transcribe_error(ResponseTemplate::new(503).set_body_json(relay_off)).await;
    assert_eq!(error, MetaError::Overloaded);
}

#[tokio::test]
async fn other_statuses_map_to_their_errors() {
    let meta_bad = json!({"error": {"message": "Unsupported   audio\nencoding", "type": "invalid_request_error"}});
    assert_eq!(
        transcribe_error(ResponseTemplate::new(400).set_body_json(meta_bad)).await,
        MetaError::BadRequest {
            message: "Unsupported audio encoding".into()
        }
    );
    let relay_big = json!({"error": "too_large", "message": "The audio is larger than 32 MB."});
    assert_eq!(
        transcribe_error(ResponseTemplate::new(413).set_body_json(relay_big)).await,
        MetaError::BadRequest {
            message: "The audio is larger than 32 MB.".into()
        }
    );
    assert!(matches!(
        transcribe_error(ResponseTemplate::new(404).set_body_string("<html>nope</html>")).await,
        MetaError::BadRequest { .. }
    ));
    assert_eq!(
        transcribe_error(ResponseTemplate::new(401)).await,
        MetaError::Unauthorized
    );
    let no_relay_ai =
        json!({"error": "no_relay_ai", "message": "Klipolo AI is off for this device."});
    assert_eq!(
        transcribe_error(ResponseTemplate::new(403).set_body_json(no_relay_ai)).await,
        MetaError::NotAllowed
    );
    assert_eq!(
        transcribe_error(ResponseTemplate::new(429).insert_header("retry-after", "7")).await,
        MetaError::RateLimited {
            retry_after: Some(7)
        }
    );
    assert_eq!(
        transcribe_error(ResponseTemplate::new(429)).await,
        MetaError::RateLimited { retry_after: None }
    );
    let meta_auth =
        json!({"error": "meta_auth", "message": "The relay could not use its Meta key."});
    assert_eq!(
        transcribe_error(ResponseTemplate::new(502).set_body_json(meta_auth)).await,
        MetaError::Overloaded
    );
    assert_eq!(
        transcribe_error(ResponseTemplate::new(500)).await,
        MetaError::Overloaded
    );
}

#[tokio::test]
async fn unreachable_server_is_a_network_error() {
    let client = MetaClient::direct(secret(KEY)).with_meta_base(format!("{CLOSED}/v1"));
    let error = client.transcribe(wav(), &[]).await.unwrap_err();
    assert!(matches!(error, MetaError::Network(_)), "{error:?}");
}

#[tokio::test]
async fn unreadable_answer_is_a_decode_error() {
    let error = transcribe_error(ResponseTemplate::new(200).set_body_string("not json")).await;
    assert!(matches!(error, MetaError::Decode(_)), "{error:?}");
}

// Files.

/// A file in the temp folder, removed when dropped.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("klipolo-meta-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn uploaded() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "file-1812852566410237",
        "bytes": 14,
        "created_at": 1793357458,
        "expires_at": 1793443858,
        "filename": "upload.mp4",
        "object": "file",
        "purpose": "user_data",
        "status": "uploaded",
    }))
}

#[tokio::test]
async fn upload_file_streams_the_file_and_returns_its_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(uploaded())
        .expect(1)
        .mount(&server)
        .await;
    let video = TempFile::new("My holiday.mp4", b"fake mp4 bytes");

    let id = direct(&server)
        .upload_file(&video.0, "video/mp4")
        .await
        .unwrap();

    assert_eq!(id, "file-1812852566410237");
    let request = only_request(&server).await;
    assert_eq!(part(&request, "purpose").1, b"user_data");
    assert_eq!(part(&request, "expires_after[anchor]").1, b"created_at");
    assert_eq!(part(&request, "expires_after[seconds]").1, b"86400");
    let (headers, bytes) = part(&request, "file");
    // The person's file name stays on the Mac.
    assert!(headers.contains("filename=\"upload.mp4\""), "{headers}");
    assert!(
        headers.to_lowercase().contains("content-type: video/mp4"),
        "{headers}"
    );
    assert_eq!(bytes, b"fake mp4 bytes");
}

#[tokio::test]
async fn upload_file_that_cannot_be_read_makes_no_call() {
    let server = MockServer::start().await;
    let missing = std::env::temp_dir().join("klipolo-meta-no-such-file.mp4");

    let error = direct(&server)
        .upload_file(&missing, "video/mp4")
        .await
        .unwrap_err();

    assert!(matches!(error, MetaError::BadRequest { .. }), "{error:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_file_sends_delete_and_treats_a_missing_file_as_deleted() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v1/files/file-1"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "file-1", "deleted": true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1/files/file-gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1/files/file-busy"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let client = direct(&server);
    client.delete_file("file-1").await.unwrap();
    client.delete_file("file-gone").await.unwrap();
    assert_eq!(
        client.delete_file("file-busy").await,
        Err(MetaError::Overloaded)
    );
}

#[tokio::test]
async fn delete_file_refuses_ids_that_are_not_plain_ids() {
    let server = MockServer::start().await;
    let client = direct(&server);
    for id in ["", "../responses", "file 1", "file-1?x=y"] {
        let error = client.delete_file(id).await.unwrap_err();
        assert!(
            matches!(error, MetaError::BadRequest { .. }),
            "{id}: {error:?}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn relay_mode_uploads_and_deletes_under_meta_v1() {
    let server = MockServer::start().await;
    let bearer = format!("Bearer {TOKEN}");
    Mock::given(method("POST"))
        .and(path("/meta/v1/files"))
        .and(header("authorization", bearer.as_str()))
        .respond_with(uploaded())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/meta/v1/files/file-1812852566410237"))
        .and(header("authorization", bearer.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"deleted": true})))
        .expect(1)
        .mount(&server)
        .await;
    let video = TempFile::new("relay.mp4", b"fake mp4 bytes");

    let client = relay(&server);
    let id = client.upload_file(&video.0, "video/mp4").await.unwrap();
    client.delete_file(&id).await.unwrap();
}

// Responses.

#[tokio::test]
async fn respond_extracts_function_calls_and_usage() {
    let server = MockServer::start().await;
    let body = json!({
        "model": "muse-spark-1.3",
        "instructions": "You are the edit planner inside Klipolo.",
        "input": [{"type": "message", "role": "user", "content": [
            {"type": "input_text", "text": "When we go vertical keep the camera on the woman astronaut"},
        ]}],
        "tools": [{"type": "function", "name": "setFormat", "strict": true, "parameters": {
            "type": "object",
            "properties": {"format": {"type": "string", "enum": ["16:9", "9:16", "1:1"]}},
            "required": ["format"],
            "additionalProperties": false,
        }}],
        "max_output_tokens": 4000,
    });
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(wiremock::matchers::body_json(&body))
        .respond_with(json_body(&fixture("responses_function_calls.json")))
        .expect(1)
        .mount(&server)
        .await;

    let out = direct(&server).respond(body).await.unwrap();

    assert_eq!(
        out.output_text.as_deref(),
        Some("Going vertical and keeping the focus on her.")
    );
    let calls: Vec<_> = out
        .function_calls
        .iter()
        .map(|c| (c.name.as_str(), c.call_id.as_str(), c.arguments.as_str()))
        .collect();
    assert_eq!(
        calls,
        [
            ("setFormat", "call_1", r#"{"format":"9:16"}"#),
            (
                "followSubject",
                "call_2",
                r#"{"description":"woman astronaut"}"#
            ),
        ]
    );
    assert_eq!(
        out.usage,
        Usage {
            input_tokens: 1810,
            cached_tokens: 1777,
            output_tokens: 820,
            reasoning_tokens: 703,
            video_frames_processed: 0,
        }
    );
    assert_eq!(out.raw["id"], "resp_5f0c2a9e41d7b3c8a6e90d12");
}

#[tokio::test]
async fn respond_reads_structured_output_from_spike_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/responses"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(json_body(&fixture("responses_json_schema.json")))
        .mount(&server)
        .await;

    let out = relay(&server)
        .respond(json!({"model": "muse-spark-1.3", "input": "find it"}))
        .await
        .unwrap();

    let found: Value = serde_json::from_str(out.output_text.as_deref().unwrap()).unwrap();
    assert_eq!(
        (found["found"].clone(), found["start_sec"].clone()),
        (json!(true), json!(67.0))
    );
    assert!(out.function_calls.is_empty());
    assert_eq!(
        (out.usage.input_tokens, out.usage.cached_tokens),
        (42_291, 0)
    );
    assert_eq!(
        (out.usage.output_tokens, out.usage.reasoning_tokens),
        (517, 454)
    );
}

#[test]
fn response_without_output_or_usage_parses_empty() {
    let out =
        ResponseOut::parse(json!({"id": "resp_1", "status": "completed", "output": []})).unwrap();
    assert_eq!(out.output_text, None);
    assert!(out.function_calls.is_empty());
    assert_eq!(out.usage, Usage::default());
}

#[test]
fn function_call_without_a_name_is_a_decode_error() {
    let raw =
        json!({"output": [{"type": "function_call", "call_id": "call_1", "arguments": "{}"}]});
    assert!(matches!(ResponseOut::parse(raw), Err(MetaError::Decode(_))));
    assert!(matches!(
        ResponseOut::parse(json!([1, 2])),
        Err(MetaError::Decode(_))
    ));
}

#[test]
fn sam_usage_counts_video_frames() {
    let raw = json!({"output": [], "usage": {"video_frames_processed": 160}});
    assert_eq!(
        ResponseOut::parse(raw)
            .unwrap()
            .usage
            .video_frames_processed,
        160
    );
}
