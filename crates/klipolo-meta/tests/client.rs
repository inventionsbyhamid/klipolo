//! The client against a local mock server. No test reaches Meta or the relay:
//! every client here points both base URLs at a mock or at a closed port, and
//! none writes to a spend ledger unless the test gives it its own.

use std::path::Path;

use klipolo_meta::{
    pricing, AlignResult, AlignTurn, AlignedWord, AsrTurn, Auth, ExposeSecret, MetaClient,
    MetaError, ResponseOut, SecretString, Usage,
};
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
        .with_ledger(None)
}

/// A relay device token, with the relay mocked at `{server}`.
fn relay(server: &MockServer) -> MetaClient {
    MetaClient::relay(secret(TOKEN))
        .with_relay_base(server.uri())
        .with_meta_base(CLOSED)
        .with_ledger(None)
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
        MetaError::NotAllowed { message: None },
        MetaError::NotAllowed {
            message: Some("Klipolo AI is off for this device \u{2014} add a code".into()),
        },
        MetaError::Unauthorized,
        MetaError::RateLimited {
            retry_after: Some(7),
            code: None,
            message: None,
        },
        MetaError::RateLimited {
            retry_after: None,
            code: None,
            message: None,
        },
        MetaError::RateLimited {
            retry_after: Some(1),
            code: None,
            message: Some("Too many requests at once \u{2014} wait".into()),
        },
        MetaError::Overloaded { message: None },
        MetaError::Overloaded {
            message: Some("Relay not configured.".into()),
        },
        MetaError::BadRequest {
            message: "Invalid audio \u{2013} expected WAV".into(),
        },
        MetaError::Network("connection refused".into()),
        MetaError::AlignUnavailable,
        MetaError::Decode("expected value at line 1".into()),
        MetaError::LocalFile {
            name: "clip.mp4".into(),
        },
        MetaError::Refused { message: None },
        MetaError::Refused {
            message: Some("The prompt \u{2014} as written \u{2014} breaks the policy".into()),
        },
        MetaError::TimedOut {
            status: 504,
            message: None,
        },
        MetaError::TimedOut {
            status: 408,
            message: Some("Too slow \u{2014} try again".into()),
        },
        MetaError::NotConfigured,
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
    assert!(MetaError::NotAllowed { message: None }
        .user_message()
        .contains("Settings"));
    assert_eq!(
        MetaError::NotAllowed {
            message: Some("Klipolo AI is off for this device \u{2014} add a code".into())
        }
        .user_message(),
        "Klipolo AI is off for this device, add a code."
    );
    assert_eq!(
        MetaError::Overloaded { message: None }.user_message(),
        "Meta is busy right now. Try again in a moment."
    );
    assert_eq!(
        MetaError::Overloaded {
            message: Some("Relay not configured.".into())
        }
        .user_message(),
        "Relay not configured."
    );
}

fn wait_message(retry_after: Option<u64>) -> String {
    MetaError::RateLimited {
        retry_after,
        code: None,
        message: None,
    }
    .user_message()
}

#[test]
fn rate_limit_waits_read_like_a_person_wrote_them() {
    let start = "Too many requests right now. Try again";
    assert_eq!(wait_message(None), format!("{start} in a moment."));
    assert_eq!(wait_message(Some(0)), format!("{start} in a moment."));
    assert_eq!(wait_message(Some(1)), format!("{start} in 1 second."));
    assert_eq!(wait_message(Some(90)), format!("{start} in 90 seconds."));
    assert_eq!(wait_message(Some(91)), format!("{start} in 2 minutes."));
    assert_eq!(wait_message(Some(5400)), format!("{start} in 90 minutes."));
    assert_eq!(wait_message(Some(5401)), format!("{start} later today."));
    assert_eq!(wait_message(Some(86_400)), format!("{start} later today."));
    assert_eq!(wait_message(Some(86_401)), format!("{start} later."));
    for seconds in [None, Some(7), Some(50_400)] {
        assert!(!wait_message(seconds).contains("Meta"), "{seconds:?}");
    }
}

// Transcription.

#[tokio::test]
async fn transcribe_parses_a_real_answer() {
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

/// Meta's non-streaming time limit (504), the relay's wait for Meta (504) and a body that took too long to arrive
/// (408) are timeouts, not a busy service.
#[tokio::test]
async fn statuses_408_and_504_are_timeouts() {
    let meta_limit = json!({"error": {
        "code": "gateway_timeout",
        "message": "Request exceeded the non-streaming server time limit before a response was produced.",
        "type": "server_error",
    }});
    let error = transcribe_error(ResponseTemplate::new(504).set_body_json(meta_limit)).await;
    assert_eq!(
        error,
        MetaError::TimedOut {
            status: 504,
            message: Some(
                "Request exceeded the non-streaming server time limit before a response was produced."
                    .into()
            ),
        }
    );
    let relay_slow = json!({"error": "request_timeout", "message": "The request took too long to arrive. Try again."});
    let error = transcribe_error(ResponseTemplate::new(408).set_body_json(relay_slow)).await;
    assert_eq!(
        error,
        MetaError::TimedOut {
            status: 408,
            message: Some("The request took too long to arrive. Try again.".into()),
        }
    );
    let bare = transcribe_error(ResponseTemplate::new(504)).await;
    assert_eq!(
        bare,
        MetaError::TimedOut {
            status: 504,
            message: None
        }
    );
    assert_eq!(
        bare.user_message(),
        "Meta took too long to answer. Try again."
    );
    assert_eq!(bare.to_string(), "timed out (504)");
    assert!(!bare.is_busy());
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
    assert_eq!(
        error,
        MetaError::Overloaded {
            message: Some("The backend is temporarily overloaded. Please retry.".into())
        }
    );

    let relay_off = json!({"error": "not_configured", "message": "Relay not configured."});
    let error = transcribe_error(ResponseTemplate::new(503).set_body_json(relay_off)).await;
    assert_eq!(error, MetaError::NotConfigured);
    assert_eq!(
        error.user_message(),
        "Klipolo's AI service isn't set up yet. Try again later or add your own Meta key in Settings."
    );

    // A bare code is not shown; the plain fallback is.
    let bare = json!({"error": "charging_unavailable"});
    let error = transcribe_error(ResponseTemplate::new(503).set_body_json(bare)).await;
    assert_eq!(error, MetaError::Overloaded { message: None });
    let explained = json!({
        "error": "charging_unavailable",
        "message": "Klipolo cannot record what this costs right now. Try again later.",
    });
    let error = transcribe_error(ResponseTemplate::new(503).set_body_json(explained)).await;
    assert_eq!(
        error.user_message(),
        "Klipolo cannot record what this costs right now. Try again later."
    );
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
        MetaError::NotAllowed {
            message: Some("Klipolo AI is off for this device.".into())
        }
    );
    assert_eq!(
        transcribe_error(ResponseTemplate::new(403)).await,
        MetaError::NotAllowed { message: None }
    );
    assert_eq!(
        transcribe_error(ResponseTemplate::new(429).insert_header("retry-after", "7")).await,
        MetaError::RateLimited {
            retry_after: Some(7),
            code: None,
            message: None,
        }
    );
    assert_eq!(
        transcribe_error(ResponseTemplate::new(429)).await,
        MetaError::RateLimited {
            retry_after: None,
            code: None,
            message: None,
        }
    );
    let meta_auth =
        json!({"error": "meta_auth", "message": "The relay could not use its Meta key."});
    assert_eq!(
        transcribe_error(ResponseTemplate::new(502).set_body_json(meta_auth)).await,
        MetaError::Overloaded {
            message: Some("The relay could not use its Meta key.".into())
        }
    );
    assert_eq!(
        transcribe_error(ResponseTemplate::new(500)).await,
        MetaError::Overloaded { message: None }
    );
}

/// A refused request's own text (Meta's, about fields the person never sees) goes to the log only: the person reads a
/// plain sentence with a next step.
#[tokio::test]
async fn a_refused_requests_own_text_stays_in_the_log() {
    let meta_bad = json!({"error": {
        "message": "Invalid 'input[0].content[1].file_id': file-9 was not found",
        "type": "invalid_request_error",
    }});
    let error = transcribe_error(ResponseTemplate::new(400).set_body_json(meta_bad)).await;

    assert_eq!(
        error.user_message(),
        "Meta could not process this request. Try again."
    );
    assert!(
        error.to_string().contains("file-9 was not found"),
        "{error}"
    );
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/v1/elsewhere", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(path("/v1/elsewhere"))
        .respond_with(json_body(&fixture("responses_function_calls.json")))
        .expect(0)
        .mount(&server)
        .await;

    let error = direct(&server)
        .respond(json!({"model": "muse-spark-1.3"}))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        MetaError::BadRequest {
            message: "The server answered with status 302.".into()
        }
    );
}

#[tokio::test]
async fn unreachable_server_is_a_network_error() {
    let client = MetaClient::direct(secret(KEY))
        .with_meta_base(format!("{CLOSED}/v1"))
        .with_ledger(None);
    let error = client.transcribe(wav(), &[]).await.unwrap_err();
    assert!(matches!(error, MetaError::Network(_)), "{error:?}");
}

#[tokio::test]
async fn unreadable_answer_is_a_decode_error() {
    let error = transcribe_error(ResponseTemplate::new(200).set_body_string("not json")).await;
    assert!(matches!(error, MetaError::Decode(_)), "{error:?}");
}

/// Through Klipolo's relay (an access code), a call that gets no answer, or a failure the relay doesn't explain, names
/// Klipolo's server: Meta is never reached directly. With the person's own key the same failures name Meta. A
/// sentence of the relay's own is kept either way.
#[tokio::test]
async fn relay_failures_name_klipolos_server_and_key_failures_name_meta() {
    let unreachable = MetaClient::relay(secret(TOKEN))
        .with_relay_base(CLOSED)
        .with_meta_base(CLOSED)
        .with_ledger(None)
        .transcribe(wav(), &[])
        .await
        .unwrap_err();
    let cases = [
        (
            unreachable,
            "Could not reach Klipolo's server. Check your internet connection and try again.",
            "Could not reach Meta. Check your internet connection and try again.",
        ),
        (
            transcribe_error(ResponseTemplate::new(503)).await,
            "Klipolo's server is busy right now. Try again in a moment.",
            "Meta is busy right now. Try again in a moment.",
        ),
        (
            transcribe_error(ResponseTemplate::new(400).set_body_json(
                json!({"error": "invalid_request", "message": "The audio must be mono."}),
            ))
            .await,
            "Klipolo's server could not process this request. Try again.",
            "Meta could not process this request. Try again.",
        ),
        (
            transcribe_error(ResponseTemplate::new(200).set_body_string("not json")).await,
            "Klipolo's server sent an answer the app could not read. Try again.",
            "Meta sent an answer Klipolo could not read. Try again.",
        ),
        (
            transcribe_error(ResponseTemplate::new(504)).await,
            "Klipolo's server took too long to answer. Try again.",
            "Meta took too long to answer. Try again.",
        ),
        (
            transcribe_error(ResponseTemplate::new(402)).await,
            "This device has used its Klipolo credit.",
            "Your Meta credit has run out.",
        ),
    ];
    for (error, relay, key) in cases {
        assert_eq!(error.relay_user_message(), relay, "{error:?}");
        assert_eq!(error.user_message(), key, "{error:?}");
    }
    // The relay's own sentence stays.
    let explained = transcribe_error(ResponseTemplate::new(502).set_body_json(
        json!({"error": "meta_unreachable", "message": "Klipolo couldn't reach Meta. Try again in a moment."}),
    ))
    .await;
    assert_eq!(
        explained.relay_user_message(),
        "Klipolo couldn't reach Meta. Try again in a moment."
    );
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

    assert_eq!(
        error,
        MetaError::LocalFile {
            name: "klipolo-meta-no-such-file.mp4".into()
        }
    );
    // Not Meta's fault, and no raw OS text.
    assert_eq!(
        error.user_message(),
        "Klipolo could not read klipolo-meta-no-such-file.mp4."
    );
    assert!(!error.to_string().contains("os error"), "{error}");
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
        Err(MetaError::Overloaded { message: None })
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
async fn respond_reads_structured_output_from_a_real_answer() {
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
fn output_text_joins_parts_and_puts_a_newline_between_messages() {
    let text = |t: &str| json!({"type": "output_text", "text": t});
    let raw = json!({"output": [
        {"type": "message", "content": [text("Going vertical "), text("for TikTok.")]},
        {"type": "function_call", "call_id": "call_1", "name": "setFormat", "arguments": "{}"},
        {"type": "message", "content": [text("Captions are on.")]},
    ]});
    assert_eq!(
        ResponseOut::parse(raw).unwrap().output_text.as_deref(),
        Some("Going vertical for TikTok.\nCaptions are on.")
    );
}

#[test]
fn response_status_is_exposed_so_callers_can_reject_cut_off_answers() {
    let status = |raw: Value| ResponseOut::parse(raw).unwrap().status;
    let fixture: Value = serde_json::from_str(&fixture("responses_function_calls.json")).unwrap();
    assert_eq!(status(fixture).as_deref(), Some("completed"));
    let cut_off = json!({
        "status": "incomplete",
        "incomplete_details": {"reason": "max_output_tokens"},
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "{\"moments\": ["}]}],
    });
    assert_eq!(status(cut_off).as_deref(), Some("incomplete"));
    assert_eq!(status(json!({"output": []})), None);
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

// Pricing.

fn assert_usd(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

fn spark_usage(input_tokens: u64, cached_tokens: u64, output_tokens: u64) -> Usage {
    Usage {
        input_tokens,
        cached_tokens,
        output_tokens,
        ..Usage::default()
    }
}

#[test]
fn spark_usd_counts_cached_tokens() {
    let usage = spark_usage(10_000, 4_000, 500);
    assert_usd(
        pricing::spark_usd(usage),
        6000.0 * 1.25e-6 + 4000.0 * 0.15e-6 + 500.0 * 4.25e-6,
    );
}

#[test]
fn spark_usd_never_counts_more_cached_than_input() {
    assert_usd(
        pricing::spark_usd(spark_usage(100, 500, 0)),
        100.0 * 0.15e-6,
    );
}

#[test]
fn asr_usd_prices_audio_by_the_hour() {
    assert_usd(pricing::asr_usd(3_600_000), 0.18);
    assert_usd(pricing::asr_usd(155_040), 0.18 * 155.04 / 3600.0);
}

#[test]
fn spark_video_tokens_match_the_measured_counts() {
    // A first trial measured 42,340 input tokens at the default 2 fps and 22,060
    // at 1 fps for 155 s, each with a prompt of about 180 tokens.
    assert_eq!(pricing::spark_video_tokens(155.0, 2.0), 310 * 131 + 1550);
    assert_eq!(pricing::spark_video_tokens(155.0, 1.0), 155 * 131 + 1550);
    assert_eq!(pricing::spark_video_tokens(0.4, 2.0), 131 + 4);
    assert_eq!(pricing::spark_video_tokens(0.0, 2.0), 0);
    assert_eq!(pricing::spark_video_tokens(-3.0, 2.0), 0);
    assert_eq!(pricing::spark_video_tokens(f64::NAN, 2.0), 0);
}

#[test]
fn sam_frames_usd_estimates_a_video_before_it_is_sent() {
    // A 60 s short at 2 frames a second, a 10 s clip (under the 50-frame minimum) and nothing at all.
    assert_usd(pricing::sam_frames_usd(120), 0.024);
    assert_usd(pricing::sam_frames_usd(20), 0.01);
    assert_usd(pricing::sam_frames_usd(0), 0.0);
}

#[test]
fn sam_usd_bills_at_least_50_frames_per_video() {
    let frames = |video_frames_processed| Usage {
        video_frames_processed,
        ..Usage::default()
    };
    assert_usd(pricing::sam_usd(frames(160)), 0.032);
    assert_usd(pricing::sam_usd(frames(10)), 0.01);
    assert_usd(pricing::sam_usd(frames(0)), 0.0);
}

// Spend ledger.

#[tokio::test]
async fn ledger_gets_one_row_per_successful_call_with_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(json_body(&fixture("asr_diarization.json")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(json_body(&fixture("responses_function_calls.json")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .respond_with(uploaded())
        .mount(&server)
        .await;
    let ledger = TempFile::new("ledger.csv", b"");
    let video = TempFile::new("ledger.mp4", b"fake mp4 bytes");
    let client = direct(&server).with_ledger(Some(ledger.0.clone()));

    client.transcribe(wav(), &[]).await.unwrap();
    client
        .respond(json!({"model": "muse-spark-1.3"}))
        .await
        .unwrap();
    client
        .respond(json!({"model": "muse-spark-1.3"}))
        .await
        .unwrap_err();
    client.upload_file(&video.0, "video/mp4").await.unwrap();

    let text = std::fs::read_to_string(&ledger.0).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(
        lines[0],
        "time_utc,model,purpose,input_tokens,output_tokens,frames_or_seconds,est_cost_usd"
    );
    let rows: Vec<Vec<&str>> = lines[1..].iter().map(|l| l.split(',').collect()).collect();
    assert!(
        rows[0][0].ends_with('Z') && rows[0][0].len() == 20,
        "{}",
        rows[0][0]
    );
    assert_eq!(
        rows[0][1..],
        [
            "muse-voice-transcribe-1.0",
            "asr/transcribe",
            "",
            "",
            "155.0s",
            "0.007752"
        ]
    );
    assert_eq!(
        rows[1][1..],
        ["muse-spark-1.3", "responses", "1810", "820", "", "0.003793"]
    );
}

#[tokio::test]
async fn ledger_names_relay_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/responses"))
        .respond_with(json_body(&fixture("responses_function_calls.json")))
        .mount(&server)
        .await;
    let ledger = TempFile::new("relay-ledger.csv", b"");

    relay(&server)
        .with_ledger(Some(ledger.0.clone()))
        .respond(json!({"model": "muse-spark-1.3"}))
        .await
        .unwrap();

    let text = std::fs::read_to_string(&ledger.0).unwrap();
    assert!(
        text.lines()
            .nth(1)
            .unwrap()
            .contains(",responses via relay,"),
        "{text}"
    );
}

// Word alignment.

fn aligned() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "words": [
            {"text": "But", "startMs": 80, "endMs": 260, "score": 0.93, "turn": 0},
            {"text": "also", "startMs": 700, "endMs": 720, "score": 0.0, "turn": 0},
        ],
        "skipped": [{"turn": 1, "reason": "The turn has no words to align."}],
    }))
}

fn align_turns() -> Vec<AlignTurn> {
    vec![
        AlignTurn {
            start_ms: 60,
            end_ms: 10_780,
            text: "But it's also also super rewarding".into(),
        },
        AlignTurn {
            start_ms: 14_380,
            end_ms: 20_860,
            text: String::new(),
        },
    ]
}

#[tokio::test]
async fn align_without_a_relay_token_is_unavailable() {
    let server = MockServer::start().await;
    let client = direct(&server).with_relay_base(server.uri());

    let error = client.align(wav(), &align_turns()).await.unwrap_err();

    assert_eq!(error, MetaError::AlignUnavailable);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn align_posts_audio_and_turns_to_the_relay_with_the_device_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(aligned())
        .expect(1)
        .mount(&server)
        .await;
    // Key mode: Meta calls use the key, alignment the device's relay token.
    let client = direct(&server)
        .with_relay_base(server.uri())
        .with_align_token(secret(TOKEN));

    let result = client.align(wav(), &align_turns()).await.unwrap();

    assert_eq!(
        result.words[0],
        AlignedWord {
            text: "But".into(),
            start_ms: 80,
            end_ms: 260,
            score: 0.93,
            turn: 0
        }
    );
    assert_eq!(result.words.len(), 2);
    assert_eq!(
        result.skipped,
        [(1, "The turn has no words to align.".to_owned())]
    );
    let request = only_request(&server).await;
    let (_, turns) = part(&request, "turns");
    assert_eq!(
        serde_json::from_slice::<Value>(&turns).unwrap(),
        json!([
            {"startMs": 60, "endMs": 10780, "text": "But it's also also super rewarding"},
            {"startMs": 14380, "endMs": 20860, "text": ""},
        ])
    );
    let (headers, audio) = part(&request, "audio");
    assert!(headers.contains("filename=\"audio.wav\""), "{headers}");
    assert_eq!(audio, wav());
}

#[tokio::test]
async fn relay_mode_aligns_with_its_own_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(aligned())
        .expect(1)
        .mount(&server)
        .await;

    relay(&server).align(wav(), &align_turns()).await.unwrap();
}

#[tokio::test]
async fn align_quota_used_up_is_rate_limited() {
    let quota =
        "This device has used today's 120 minutes of word timing. It resets at midnight UTC.";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "50400")
                .set_body_json(json!({"error": "align_quota", "message": quota})),
        )
        .mount(&server)
        .await;

    let error = relay(&server)
        .align(wav(), &align_turns())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        MetaError::RateLimited {
            retry_after: Some(50_400),
            code: Some("align_quota".into()),
            message: Some(quota.into()),
        }
    );
    assert_eq!(error.user_message(), quota);
}

#[tokio::test]
async fn relay_busy_is_rate_limited_with_its_reason() {
    let busy = "Too many requests at once from this device. Wait for one to finish.";
    let error = transcribe_error(
        ResponseTemplate::new(429)
            .insert_header("retry-after", "1")
            .set_body_json(json!({"error": "busy", "message": busy})),
    )
    .await;
    assert_eq!(
        error,
        MetaError::RateLimited {
            retry_after: Some(1),
            code: Some("busy".into()),
            message: Some(busy.into()),
        }
    );
    assert_eq!(error.user_message(), busy);
}

/// Traefik's rate limit in front of the relay answers 429 in plain text, with no error code: a busy moment to wait out
/// and send again, like the relay's own `busy`, never a stop.
#[tokio::test]
async fn a_429_with_no_error_code_is_busy() {
    let traefik = ResponseTemplate::new(429).set_body_raw("Too Many Requests", "text/plain");
    let error = transcribe_error(traefik).await;
    assert_eq!(
        error,
        MetaError::RateLimited {
            retry_after: None,
            code: None,
            message: None,
        }
    );
    assert!(error.is_busy());
    assert_eq!(
        error.user_message(),
        "Too many requests right now. Try again in a moment."
    );
}

#[test]
fn only_a_busy_moment_is_busy() {
    let limited = |code: Option<&str>| MetaError::RateLimited {
        retry_after: Some(1),
        code: code.map(str::to_owned),
        message: None,
    };
    assert!(limited(Some("busy")).is_busy());
    assert!(limited(None).is_busy());
    // JSON with a reason but no code is a server's own rate limit: a stop.
    assert!(!MetaError::RateLimited {
        retry_after: Some(15),
        code: None,
        message: Some("Rate limit exceeded.".into()),
    }
    .is_busy());
    // A used-up allowance or Meta's own rate limit is a stop, not a moment.
    assert!(!limited(Some("align_quota")).is_busy());
    assert!(!limited(Some("rate_limit_exceeded")).is_busy());
    assert!(!limited(Some("meta_error")).is_busy());
    assert!(!MetaError::Overloaded { message: None }.is_busy());
    assert!(!MetaError::Budget {
        message: String::new()
    }
    .is_busy());
}

/// A JSON 429 whose code is null still says why: it is that server's own limit, not Traefik's plain text, so it stays
/// a stop.
#[tokio::test]
async fn a_json_429_with_a_null_code_is_not_busy() {
    let error = transcribe_error(
        ResponseTemplate::new(429)
            .insert_header("retry-after", "15")
            .set_body_json(json!({"error": {
                "code": null,
                "message": "Rate limit exceeded. Please retry after 15 seconds.",
                "type": "rate_limit_error",
            }})),
    )
    .await;
    assert_eq!(
        error,
        MetaError::RateLimited {
            retry_after: Some(15),
            code: None,
            message: Some("Rate limit exceeded. Please retry after 15 seconds.".into()),
        }
    );
    assert!(!error.is_busy());
}

/// Meta's own 429 names its code inside `error`; a "code" that isn't one (words, or text holding a secret) is dropped.
#[tokio::test]
async fn a_rate_limits_code_is_kept_only_when_it_is_a_code() {
    let error = transcribe_error(ResponseTemplate::new(429).set_body_json(json!({
        "error": {"code": "rate_limit_exceeded", "message": "Slow down."}
    })))
    .await;
    assert_eq!(
        error,
        MetaError::RateLimited {
            retry_after: None,
            code: Some("rate_limit_exceeded".into()),
            message: Some("Slow down.".into()),
        }
    );
    for odd in [
        json!({"error": "Too many requests"}),
        json!({"error": "key relay-test-token", "message": "Busy."}),
        json!({"error": {"code": 429, "message": "Busy."}}),
        json!({"error": "x".repeat(65)}),
    ] {
        let error = transcribe_error(ResponseTemplate::new(429).set_body_json(odd.clone())).await;
        assert!(
            matches!(error, MetaError::RateLimited { code: None, .. }),
            "{odd}: {error:?}"
        );
    }
}

#[test]
fn align_result_reads_and_writes_the_relays_json() {
    let wire = json!({
        "words": [{"text": "Hi", "startMs": 14400, "endMs": 14610, "score": 0.81, "turn": 1}],
        "skipped": [{"turn": 0, "reason": "The turn is past the end of the audio."}],
    });
    let result: AlignResult = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(
        result.skipped,
        [(0, "The turn is past the end of the audio.".to_owned())]
    );
    assert_eq!(serde_json::to_value(&result).unwrap(), wire);
}

#[tokio::test]
async fn align_answer_without_words_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"skipped": []})))
        .mount(&server)
        .await;

    let error = relay(&server)
        .align(wav(), &align_turns())
        .await
        .unwrap_err();
    assert!(matches!(error, MetaError::Decode(_)), "{error:?}");
}

#[test]
fn align_turn_comes_from_an_asr_turn() {
    let turn = AsrTurn {
        start_ms: 5,
        end_ms: 9,
        text: "Hello".into(),
        speaker: Some("A".into()),
    };
    assert_eq!(
        AlignTurn::from(&turn),
        AlignTurn {
            start_ms: 5,
            end_ms: 9,
            text: "Hello".into()
        }
    );
}

// Devices.

const DEVICE_ID: &str = "6f1c3c2e-8d5b-4b8e-9a51-2f7d0c4e9b10";

#[tokio::test]
async fn register_device_sends_its_id_and_code_and_returns_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(wiremock::matchers::body_json(
            json!({"device_id": DEVICE_ID, "access_code": "TEST-CODE-0001"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "fresh-device-token",
            "relay_ai": true,
            "budget_usd": 2.0,
            "spent_usd": 0.25,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let info = MetaClient::register_device(
        &format!("{}/", server.uri()),
        DEVICE_ID,
        Some(" TEST-CODE-0001 "),
    )
    .await
    .unwrap();

    assert_eq!(
        info.token.as_ref().map(|t| t.expose_secret()),
        Some("fresh-device-token")
    );
    assert!(info.relay_ai);
    assert_eq!((info.budget_usd, info.spent_usd), (2.0, 0.25));
    assert!(!format!("{info:?}").contains("fresh-device-token"));
}

#[tokio::test]
async fn register_device_without_a_code_sends_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(wiremock::matchers::body_json(
            json!({"device_id": DEVICE_ID}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "fresh-device-token",
            "relay_ai": false,
            "budget_usd": 0.0,
            "spent_usd": 0.0,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let info = MetaClient::register_device(&server.uri(), DEVICE_ID, None)
        .await
        .unwrap();
    assert!(!info.relay_ai);
}

#[tokio::test]
async fn register_device_with_an_invalid_code_is_not_allowed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "invalid_code",
            "message": "This access code is not valid. Check the code or add your own Meta key in Settings.",
        })))
        .mount(&server)
        .await;

    let error = MetaClient::register_device(&server.uri(), DEVICE_ID, Some("WRONG"))
        .await
        .unwrap_err();
    let reason =
        "This access code is not valid. Check the code or add your own Meta key in Settings.";
    assert_eq!(
        error,
        MetaError::NotAllowed {
            message: Some(reason.into())
        }
    );
    assert_eq!(error.user_message(), reason);
}

#[tokio::test]
async fn register_device_answer_without_a_token_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"relay_ai": true})))
        .mount(&server)
        .await;

    let error = MetaClient::register_device(&server.uri(), DEVICE_ID, None)
        .await
        .unwrap_err();
    assert!(matches!(error, MetaError::Decode(_)), "{error:?}");
}

#[tokio::test]
async fn device_status_reads_devices_me_with_the_device_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "relay_ai": true,
            "budget_usd": 2.0,
            "spent_usd": 1.5,
        })))
        .expect(2)
        .mount(&server)
        .await;

    let keyed = direct(&server)
        .with_relay_base(server.uri())
        .with_align_token(secret(TOKEN));
    for client in [keyed, relay(&server)] {
        let info = client.device_status().await.unwrap();
        assert!(info.token.is_none());
        assert!(info.relay_ai);
        assert_eq!((info.budget_usd, info.spent_usd), (2.0, 1.5));
    }
}

#[tokio::test]
async fn device_status_without_a_relay_token_is_unauthorized() {
    let server = MockServer::start().await;
    let client = direct(&server).with_relay_base(server.uri());

    assert_eq!(
        client.device_status().await.unwrap_err(),
        MetaError::Unauthorized
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

// Secrets never in error text.

/// The variant's name. No wildcard arm: a new variant fails to compile here
/// until `error_text_never_contains_key` covers it.
fn variant(error: &MetaError) -> &'static str {
    match error {
        MetaError::Budget { .. } => "Budget",
        MetaError::NotAllowed { .. } => "NotAllowed",
        MetaError::Unauthorized => "Unauthorized",
        MetaError::RateLimited { .. } => "RateLimited",
        MetaError::Overloaded { .. } => "Overloaded",
        MetaError::BadRequest { .. } => "BadRequest",
        MetaError::Network(_) => "Network",
        MetaError::AlignUnavailable => "AlignUnavailable",
        MetaError::Decode(_) => "Decode",
        MetaError::LocalFile { .. } => "LocalFile",
        MetaError::Refused { .. } => "Refused",
        MetaError::TimedOut { .. } => "TimedOut",
        MetaError::NotConfigured => "NotConfigured",
    }
}

const VARIANTS: usize = 13;

/// A server that answers every request with `status` and a body quoting `echo`.
async fn echoing(status: u16, echo: &str) -> MockServer {
    let server = MockServer::start().await;
    let response = if status == 200 {
        // Fields of the wrong type: serde's message quotes the value.
        ResponseTemplate::new(200).set_body_json(json!({"audioDurationMs": echo, "relay_ai": echo}))
    } else {
        ResponseTemplate::new(status)
            .insert_header("retry-after", "3")
            .set_body_json(json!({"error": {"message": echo}, "message": echo}))
    };
    Mock::given(wiremock::matchers::any())
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn error_text_never_contains_key() {
    const CODE: &str = "ACCESS-CODE-9f8e7d";
    let mut errors: Vec<(MetaError, Vec<&str>)> = Vec::new();
    for status in [
        200, 400, 401, 402, 403, 404, 408, 413, 422, 429, 500, 502, 503, 504,
    ] {
        // Key mode with a relay token for alignment: both are its secrets.
        let server = echoing(status, &format!("key {KEY} token {TOKEN}")).await;
        let keyed = direct(&server)
            .with_relay_base(server.uri())
            .with_align_token(secret(TOKEN));
        errors.push((
            keyed.transcribe(wav(), &[]).await.unwrap_err(),
            vec![KEY, TOKEN],
        ));
        errors.push((
            keyed.align(wav(), &align_turns()).await.unwrap_err(),
            vec![KEY, TOKEN],
        ));
        if status != 200 {
            let refused = keyed
                .respond(json!({"model": "muse-spark-1.3"}))
                .await
                .unwrap_err();
            errors.push((refused, vec![KEY, TOKEN]));
        }

        let server = echoing(status, &format!("token {TOKEN}")).await;
        errors.push((
            relay(&server).transcribe(wav(), &[]).await.unwrap_err(),
            vec![TOKEN],
        ));
        errors.push((
            relay(&server).device_status().await.unwrap_err(),
            vec![TOKEN],
        ));

        let server = echoing(status, &format!("code {CODE}")).await;
        let registered = MetaClient::register_device(&server.uri(), DEVICE_ID, Some(CODE)).await;
        errors.push((registered.unwrap_err(), vec![CODE]));
    }
    // A content policy refusal keeps Meta's reason, redacted.
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"code": "content_policy_violation", "message": format!("key {KEY} token {TOKEN}")}
        })))
        .mount(&server)
        .await;
    let keyed = direct(&server).with_align_token(secret(TOKEN));
    errors.push((
        keyed.transcribe(wav(), &[]).await.unwrap_err(),
        vec![KEY, TOKEN],
    ));
    // The relay without its Meta key.
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": "not_configured", "message": format!("token {TOKEN}")
        })))
        .mount(&server)
        .await;
    errors.push((
        relay(&server).transcribe(wav(), &[]).await.unwrap_err(),
        vec![TOKEN],
    ));
    let unreachable = MetaClient::direct(secret(KEY))
        .with_meta_base(format!("{CLOSED}/v1"))
        .with_ledger(None);
    errors.push((
        unreachable.transcribe(wav(), &[]).await.unwrap_err(),
        vec![KEY],
    ));
    let server = MockServer::start().await;
    let missing = std::env::temp_dir().join(format!("klipolo-meta-{KEY}.mp4"));
    errors.push((
        direct(&server)
            .upload_file(&missing, "video/mp4")
            .await
            .unwrap_err(),
        vec![KEY],
    ));
    let no_token = direct(&server).with_relay_base(server.uri());
    errors.push((
        no_token.align(wav(), &align_turns()).await.unwrap_err(),
        vec![KEY],
    ));

    let mut seen = std::collections::BTreeSet::new();
    for (error, secrets) in &errors {
        seen.insert(variant(error));
        for text in [
            error.to_string(),
            format!("{error:?}"),
            error.user_message(),
        ] {
            for secret in secrets {
                assert!(
                    !text.contains(secret),
                    "{} shows a secret: {text}",
                    variant(error)
                );
            }
        }
    }
    assert_eq!(seen.len(), VARIANTS, "variants covered: {seen:?}");
    // The echoes did reach every variant that keeps a server's message, redacted.
    for name in [
        "Budget",
        "NotAllowed",
        "RateLimited",
        "Overloaded",
        "BadRequest",
        "Decode",
        "Refused",
        "TimedOut",
    ] {
        assert!(
            errors
                .iter()
                .any(|(e, _)| variant(e) == name && format!("{e:?}").contains("[redacted]")),
            "no redacted {name}"
        );
    }
    let client = direct(&server).with_align_token(secret(TOKEN));
    for text in [format!("{client:?}"), format!("{:?}", relay(&server))] {
        assert!(!text.contains(KEY) && !text.contains(TOKEN), "{text}");
    }
}

// Timeouts.

const SHORT: std::time::Duration = std::time::Duration::from_millis(300);

#[tokio::test]
async fn small_calls_give_up_after_the_short_timeout() {
    let server = MockServer::start().await;
    let slow = ResponseTemplate::new(200)
        .set_body_json(
            json!({"relay_ai": true, "budget_usd": 2.0, "spent_usd": 0.0, "deleted": true}),
        )
        .set_delay(std::time::Duration::from_secs(5));
    Mock::given(wiremock::matchers::any())
        .respond_with(slow)
        .mount(&server)
        .await;
    let client = relay(&server).with_short_timeout(SHORT);

    let started = std::time::Instant::now();
    let status = client.device_status().await.unwrap_err();
    let delete = client.delete_file("file-1").await.unwrap_err();

    assert!(matches!(status, MetaError::Network(_)), "{status:?}");
    assert!(matches!(delete, MetaError::Network(_)), "{delete:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn long_calls_outlast_the_short_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_body(&fixture("asr_diarization.json")).set_delay(SHORT * 3))
        .mount(&server)
        .await;

    let result = direct(&server)
        .with_short_timeout(SHORT)
        .transcribe(wav(), &[])
        .await
        .unwrap();
    assert_eq!(result.turns.len(), 9);
}

/// A server that reads one request, sends `head` (status, headers and the
/// start of a body) and then stalls.
fn stalling_server(head: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let Ok(n) = stream.read(&mut buffer) else {
                return;
            };
            request.extend_from_slice(&buffer[..n]);
            let text = String::from_utf8_lossy(&request).to_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            if n == 0 {
                return;
            }
        }
        let _ = stream.write_all(head.as_bytes());
        std::thread::sleep(std::time::Duration::from_secs(5));
    });
    format!("http://{address}")
}

#[tokio::test]
async fn a_stalled_error_body_is_read_for_the_short_timeout_only() {
    let base = stalling_server(
        "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"error\":",
    );
    let client = MetaClient::direct(secret(KEY))
        .with_meta_base(format!("{base}/v1"))
        .with_relay_base(CLOSED)
        .with_ledger(None)
        .with_short_timeout(SHORT);

    let started = std::time::Instant::now();
    let error = client
        .respond(json!({"model": "muse-spark-1.3"}))
        .await
        .unwrap_err();

    assert_eq!(error, MetaError::Overloaded { message: None });
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

// Streamed responses (SAM through the relay).

/// A server-sent event stream of `events`, each one `data:` line.
fn sse(events: &[Value]) -> ResponseTemplate {
    let body: String = events
        .iter()
        .map(|e| {
            format!(
                "event: {}\ndata: {e}\n\n",
                e["type"].as_str().unwrap_or("x")
            )
        })
        .collect();
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

#[tokio::test]
async fn a_streamed_answer_is_read_to_its_final_usage() {
    let server = MockServer::start().await;
    let text = "<0f>0<|box;x1=1;y1=2;x2=3;y2=4;w=10;h=10|><|mask;x=0;y=0;data=1,1,!a|>\n<1f>";
    let (first, rest) = text.split_at(20);
    Mock::given(method("POST"))
        .and(path("/meta/v1/responses"))
        .respond_with(sse(&[
            json!({"type": "response.created", "response": {"status": "in_progress"}}),
            json!({"type": "response.output_text.delta", "delta": first}),
            json!({"type": "response.output_text.delta", "delta": rest}),
            json!({"type": "response.completed", "response": {
                "model": "sam-3.1", "status": "completed", "output": [],
                "usage": {"input_tokens": 0, "output_tokens": 0, "video_frames_processed": 160}
            }}),
        ]))
        .expect(1)
        .mount(&server)
        .await;
    let out = relay(&server)
        .respond_stream(json!({"model": "sam-3.1", "input": []}))
        .await
        .unwrap();
    assert_eq!(out.output_text.as_deref(), Some(text));
    assert_eq!(out.usage.video_frames_processed, 160);
    assert_eq!(out.status.as_deref(), Some("completed"));
    // Sent as a stream, asking for events.
    let sent = only_request(&server).await;
    assert_eq!(sent.body_json::<Value>().unwrap()["stream"], true);
    assert_eq!(
        sent.headers.get("accept").unwrap().to_str().unwrap(),
        "text/event-stream"
    );
}

#[tokio::test]
async fn a_stream_that_fails_or_ends_early_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse(&[
            json!({"type": "response.created", "response": {}}),
            json!({"type": "response.failed", "response": {"status": "failed", "error": {"message": "The video could not be read."}}}),
        ]))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let failed = direct(&server)
        .respond_stream(json!({"model": "sam-3.1"}))
        .await
        .unwrap_err();
    assert!(
        matches!(&failed, MetaError::Refused { message: Some(m) } if m.contains("could not be read")),
        "{failed:?}"
    );

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse(&[
            json!({"type": "response.output_text.delta", "delta": "<0f>"}),
        ]))
        .mount(&server)
        .await;
    let cut = direct(&server)
        .respond_stream(json!({"model": "sam-3.1"}))
        .await
        .unwrap_err();
    assert!(matches!(cut, MetaError::Decode(_)), "{cut:?}");
}

#[tokio::test]
async fn an_event_spread_over_several_data_lines_is_read_whole() {
    let server = MockServer::start().await;
    // Server-sent events may split one event's data over several `data:` lines, joined by newlines.
    let completed = json!({"type": "response.completed", "response": {
        "model": "sam-3.1", "status": "completed", "output": [],
        "usage": {"video_frames_processed": 60}
    }})
    .to_string();
    // Split where JSON allows a line break: after its first field.
    let (head, tail) = completed.split_at(completed.find(',').unwrap() + 1);
    let body = format!(
        "event: response.output_text.delta\ndata: {{\"type\": \"response.output_text.delta\",\ndata: \"delta\": \"<0f>\"}}\n\n\
         event: response.completed\ndata: {head}\ndata: {tail}\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let out = direct(&server)
        .respond_stream(json!({"model": "sam-3.1"}))
        .await
        .unwrap();
    assert_eq!(out.output_text.as_deref(), Some("<0f>"));
    assert_eq!(out.usage.video_frames_processed, 60);
}

// SAM's video: the relay makes its own copy.

#[tokio::test]
async fn a_sam_video_goes_to_the_relays_sam_videos_and_with_a_key_to_meta_as_it_is() {
    let video = std::env::temp_dir().join(format!("klipolo-meta-sam-{}.mp4", std::process::id()));
    std::fs::write(&video, b"the small copy").unwrap();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/sam/videos"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("content-type", "video/mp4"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "file-made", "object": "file"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        relay(&server).upload_sam_video(&video).await.unwrap(),
        "file-made"
    );
    let sent = only_request(&server).await;
    assert_eq!(sent.body, b"the small copy");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "file-own", "object": "file"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        direct(&server).upload_sam_video(&video).await.unwrap(),
        "file-own"
    );
    let _ = std::fs::remove_file(&video);
}
