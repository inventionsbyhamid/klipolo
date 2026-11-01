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

/// SAM's answer as the stream it sends, in the shape of a recorded real one: the response created and in progress, its
/// message and text part added, one text delta per frame line, the text and the message done, then the completed
/// response carrying the whole text again and the frames billed.
fn answer(text: &str, frames: u64) -> ResponseTemplate {
    answer_ending(text, frames, "completed")
}

/// [`answer`] ending as `status`: `incomplete` for one Meta stopped part-way.
fn answer_ending(text: &str, frames: u64, status: &str) -> ResponseTemplate {
    let message = |status: &str, text: &str| {
        json!({"type": "message", "id": "msg_1", "role": "assistant", "status": status,
               "content": [{"type": "output_text", "text": text, "annotations": [], "logprobs": []}]})
    };
    let part = json!({"type": "output_text", "text": "", "annotations": [], "logprobs": []});
    let mut events = vec![
        json!({"type": "response.created", "sequence_number": 0,
               "response": {"id": "resp_1", "model": "sam-3.1", "status": "in_progress", "output": []}}),
        json!({"type": "response.in_progress", "sequence_number": 1,
               "response": {"id": "resp_1", "model": "sam-3.1", "status": "in_progress", "output": []}}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": message("in_progress", "")}),
        json!({"type": "response.content_part.added", "item_id": "msg_1", "part": part}),
    ];
    for line in text.split_inclusive('\n') {
        events
            .push(json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": line}));
    }
    events.extend([
        json!({"type": "response.output_text.done", "item_id": "msg_1", "text": text}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": message("completed", text)}),
        json!({"type": format!("response.{status}"), "response": {
            "id": "resp_1", "object": "response", "model": "sam-3.1", "status": status,
            "output": [message("completed", text)],
            "usage": {"input_tokens": 0, "output_tokens": 0, "video_frames_processed": frames}
        }}),
    ]);
    let body: String = (events.iter())
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect();
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
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
        .respond_with(answer(PERSON, 160))
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
    assert_eq!(sent["stream"], true);
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
        .respond_with(answer("not frames", 50))
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
        .respond_with(answer("", 20))
        .mount(&server)
        .await;
    let (_dir, file) = video();
    let tracking = direct(&server).track(&file, "person").await.unwrap();
    assert!(tracking.frames.iter().all(|f| f.boxes.is_empty()));
    // Under the 50-frame minimum: billed as 50.
    assert!((tracking.cost_usd - 0.01).abs() < 1e-12);
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_line_that_doesnt_read_is_skipped_and_the_rest_of_the_paid_answer_kept() {
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-sam-bad-line").await;
    let mut lines: Vec<&str> = PERSON.lines().collect();
    lines[5] = "<5f>0<|box;x1=1;y1=";
    let text = lines.join("\n");
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(answer(&text, 160))
        .mount(&server)
        .await;
    let (_dir, file) = video();
    let tracking = direct(&server).track(&file, "person").await.unwrap();
    assert_eq!(tracking.frames.len(), 159);
    assert!(!tracking.frames.iter().any(|f| f.frame == 5));
    assert!(!tracking.cut_off);
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_meta_stopped_part_way_is_kept_and_said_to_be_cut_off() {
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-sam-cut").await;
    // The first 40 frames, the last of them cut off mid-line.
    let mut text: String = PERSON.lines().take(40).collect::<Vec<_>>().join("\n");
    text.push_str("\n<40f>3<|box;x1=");
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(answer_ending(&text, 160, "incomplete"))
        .mount(&server)
        .await;
    let (_dir, file) = video();
    let tracking = direct(&server).track(&file, "person").await.unwrap();
    assert_eq!(tracking.frames.len(), 40);
    assert!(tracking.cut_off);
    // Billed for every frame sent.
    assert!((tracking.cost_usd - 0.032).abs() < 1e-12);
    server.verify().await;
}
