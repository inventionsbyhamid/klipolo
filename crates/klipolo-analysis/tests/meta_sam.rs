//! SAM through the Meta adapter as the `SubjectTracker`: the small copy uploaded, a real recorded answer read into
//! boxes as parts of the picture, priced by the frames billed, and the upload deleted on every path. Against a mock Meta
//! and relay: nothing here reaches Meta.

use std::path::PathBuf;

use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{ProviderErrorKind, SubjectTracker};
use klipolo_meta::{MetaClient, SecretString};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "test-meta-key-not-real";
const TOKEN: &str = "test-relay-token-not-real";
/// SAM's real answer to "person" on 160 frames of NASA footage at 854 × 480.
const PERSON: &str = include_str!("../../klipolo-sam/tests/fixtures/person_concept_one_bit.txt");

fn video() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("copy.mp4");
    std::fs::write(&file, b"a small copy").unwrap();
    (dir, file)
}

fn answer(text: &str, frames: u64) -> Value {
    json!({
        "id": "resp_1",
        "object": "response",
        "model": "sam-3.1",
        "status": "completed",
        "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}],
        "usage": {"input_tokens": 0, "output_tokens": 0, "video_frames_processed": frames}
    })
}

async fn mount_files(server: &MockServer, prefix: &str, id: &str) {
    Mock::given(method("POST"))
        .and(path(format!("{prefix}/files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id, "object": "file"})))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{prefix}/files/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id, "deleted": true})))
        .expect(1)
        .mount(server)
        .await;
}

async fn requests(server: &MockServer, verb: &str, path_end: &str) -> Vec<Request> {
    (server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter())
    .filter(|r| r.method.as_str() == verb && r.url.path().ends_with(path_end))
    .collect()
}

fn direct(server: &MockServer) -> Meta {
    Meta::new(
        MetaClient::direct(SecretString::from(KEY))
            .with_meta_base(format!("{}/v1", server.uri()))
            .with_ledger(None),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn tracks_a_phrase_and_reads_the_real_answer_into_boxes() {
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-sam").await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer(PERSON, 160)))
        .expect(1)
        .mount(&server)
        .await;
    let meta = direct(&server);
    let (_dir, file) = video();
    let tracking = meta.track(&file, "person").await.unwrap();
    server.verify().await;

    // The request: SAM, the phrase and the upload by its id.
    let sent: Value = requests(&server, "POST", "/responses").await[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "sam-3.1");
    assert_eq!(sent["input"][0]["content"][0]["text"], "person");
    assert_eq!(sent["input"][0]["content"][1]["file_id"], "file-sam");

    // 160 frames, 8 or 9 people on each; frame 0's first box (536, 91 to 661, 472 of 854 × 480) as parts.
    assert_eq!(tracking.frames.len(), 160);
    let first = tracking.frames[0].boxes[0];
    assert_eq!(first.id, 0);
    assert!((first.left - 536.0 / 854.0).abs() < 1e-9);
    assert!((first.top - 91.0 / 480.0).abs() < 1e-9);
    assert!((first.right - 661.0 / 854.0).abs() < 1e-9);
    assert!((first.bottom - 472.0 / 480.0).abs() < 1e-9);
    assert!((first.confidence.unwrap() - 0.925_925_9).abs() < 1e-6);
    // Billed by the frames processed: $0.20 a thousand.
    assert_eq!(tracking.frames_billed, 160);
    assert!((tracking.cost_usd - 0.032).abs() < 1e-12);
    // Its price before a call is the same.
    assert!((meta.estimate_usd(160) - 0.032).abs() < 1e-12);
    assert_eq!(meta.model(), "sam-3.1");
    assert_eq!(meta.max_frames(), 15_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn through_the_relay_and_on_a_failure_the_upload_is_deleted() {
    // A used-up credit through the relay: a budget stop, with the relay's sentence, and the upload deleted.
    let server = MockServer::start().await;
    mount_files(&server, "/meta/v1", "file-sam-relay").await;
    let sentence = "This device used its Klipolo credit. Add your Meta API key in Settings.";
    Mock::given(method("POST"))
        .and(path("/meta/v1/responses"))
        .respond_with(
            ResponseTemplate::new(402)
                .set_body_json(json!({"error": "budget_exhausted", "message": sentence})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let meta = Meta::new(
        MetaClient::relay(SecretString::from(TOKEN))
            .with_relay_base(server.uri())
            .with_ledger(None),
    );
    let (_dir, file) = video();
    let err = meta.track(&file, "person").await.unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Budget);
    assert_eq!(err.user_message(), sentence);
    server.verify().await;

    // An answer that isn't SAM's frame lines: a failed request, its upload deleted too.
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-sam-bad").await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer("not frames", 50)))
        .mount(&server)
        .await;
    let err = direct(&server).track(&file, "person").await.unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Failed);
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_found_is_an_empty_tracking_still_billed() {
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-sam-none").await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer("", 20)))
        .mount(&server)
        .await;
    let (_dir, file) = video();
    let tracking = direct(&server).track(&file, "person").await.unwrap();
    assert!(tracking.frames.iter().all(|f| f.boxes.is_empty()));
    // Under the 50-frame minimum: billed as 50.
    assert!((tracking.cost_usd - 0.01).abs() < 1e-12);
    server.verify().await;
}
