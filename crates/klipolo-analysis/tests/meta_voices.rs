//! Muse Spark through the Meta adapter as the `VoiceMatcher`: the window uploaded and asked about with the people on
//! screen numbered from the left, a strict answer read into a person, nobody, or unclear, priced by its usage, and the
//! upload deleted. Against a mock Meta: nothing here reaches Meta.

use std::path::PathBuf;

use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{Heard, ScreenBox, VoiceMatcher};
use klipolo_meta::{MetaClient, SecretString};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-meta-key-not-real";

fn video() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("window.mp4");
    std::fs::write(&file, b"a few seconds").unwrap();
    (dir, file)
}

fn two() -> Vec<ScreenBox> {
    vec![
        ScreenBox {
            left: 0.1,
            top: 0.2,
            right: 0.4,
            bottom: 1.0,
        },
        ScreenBox {
            left: 0.6,
            top: 0.15,
            right: 0.9,
            bottom: 1.0,
        },
    ]
}

/// Spark's answer with `text` as its output, 1,200 tokens in and 300 out.
fn answer(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "resp_v", "object": "response", "model": "muse-spark-1.3", "status": "completed",
        "output": [{"type": "message", "id": "msg_v", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}]}],
        "usage": {"input_tokens": 1200, "output_tokens": 300},
    }))
}

async fn ask(text: &str) -> (Heard, f64, Value) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "file-v", "object": "file"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1/files/file-v"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "file-v", "deleted": true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(answer(text))
        .expect(1)
        .mount(&server)
        .await;
    let meta = Meta::new(
        MetaClient::direct(SecretString::from(KEY))
            .with_meta_base(format!("{}/v1", server.uri()))
            .with_ledger(None),
    );
    let (_dir, file) = video();
    let got = meta.who_speaks(&file, &two()).await.unwrap();
    server.verify().await;
    let sent = (server.received_requests().await.unwrap().into_iter())
        .find(|r| r.url.path().ends_with("/responses"))
        .unwrap()
        .body_json()
        .unwrap();
    (got.heard, got.cost_usd, sent)
}

#[tokio::test(flavor = "multi_thread")]
async fn asks_which_numbered_person_speaks_with_a_strict_answer() {
    let (heard, cost, sent) = ask(r#"{"speaker": "2"}"#).await;
    assert_eq!(heard, Heard::Person { person: 2 });
    // At Spark's list prices.
    let want = 1200.0 * 1.25 / 1e6 + 300.0 * 4.25 / 1e6;
    assert!((cost - want).abs() < 1e-9, "{cost}");
    // The window by its upload, at 2 frames a second, and the people numbered from the left with their boxes.
    assert_eq!(sent["model"], "muse-spark-1.3");
    let content = &sent["input"][0]["content"];
    assert_eq!(
        content[1],
        json!({"type": "input_video", "file_id": "file-v", "fps": 2.0})
    );
    let prompt = content[0]["text"].as_str().unwrap();
    assert!(
        prompt.contains("1: left 0.10, top 0.20, right 0.40, bottom 1.00"),
        "{prompt}"
    );
    assert!(
        prompt.contains("2: left 0.60, top 0.15, right 0.90, bottom 1.00"),
        "{prompt}"
    );
    // Strict: one of the people's numbers, or none.
    let format = &sent["text"]["format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["strict"], true);
    assert_eq!(
        format["schema"]["properties"]["speaker"]["enum"],
        json!(["1", "2", "none"])
    );
    assert_eq!(format["schema"]["additionalProperties"], false);
    assert_eq!(sent["reasoning"], json!({"effort": "low"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_on_screen_and_answers_that_dont_say_are_told_apart() {
    assert_eq!(ask(r#"{"speaker": "none"}"#).await.0, Heard::Nobody);
    // Paid for all the same, so never an error: someone not listed, or an answer that doesn't read, says nothing.
    for text in [
        r#"{"speaker": "3"}"#,
        r#"{"speaker": "0"}"#,
        "not json",
        r#"{"who": "1"}"#,
    ] {
        let (heard, cost, _) = ask(text).await;
        assert_eq!(heard, Heard::Unclear, "{text}");
        assert!(cost > 0.0);
    }
}

#[test]
fn a_window_is_priced_from_its_frames_sound_and_prompt() {
    let meta = Meta::new(MetaClient::direct(SecretString::from(KEY)).with_ledger(None));
    let three = meta.estimate_usd(3.0);
    // About a third of a cent: 6 frames and 3 s of sound, the prompt, and a short answer.
    assert!(three > 0.001 && three < 0.005, "{three}");
    assert!(meta.estimate_usd(2.0) < three);
}
