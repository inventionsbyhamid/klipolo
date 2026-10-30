//! The client against a local mock server. No test reaches Meta or the relay:
//! every client here points both base URLs at a mock or at a closed port.

use std::path::Path;

use klipolo_meta::{Auth, MetaClient, MetaError, SecretString};
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
