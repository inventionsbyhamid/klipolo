//! klipolo-chat's eval of a recording's camera and sounds (`evals/recording_cases.json`), run through the dev CLI
//! `klipolo-chat-eval --cases` against a mock Meta that answers each case as it should be answered: every case passes,
//! so the set, its project and its expectations hold together before a paid run.

use std::process::{Command, Output};

use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-meta-key-not-real";
const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../klipolo-chat/evals/recording_cases.json"
);

async fn run(env: &[(&str, String)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_klipolo-chat-eval"));
    command
        .args(args)
        .env_remove("MODEL_API_KEY")
        .env_remove("KLIPOLO_RELAY_TOKEN")
        .env_remove("KLIPOLO_SPEND_LEDGER")
        .env_remove("KLIPOLO_META_BASE")
        .env_remove("KLIPOLO_RELAY_BASE");
    for (k, v) in env {
        command.env(k, v);
    }
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

/// A Responses API answer with `text` and these calls.
fn spark(text: &str, calls: &[(&str, Value)]) -> Value {
    let mut output = vec![json!({
        "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]
    })];
    for (i, (name, args)) in calls.iter().enumerate() {
        output.push(json!({
            "type": "function_call", "call_id": format!("call_{i}"), "name": name, "arguments": args.to_string()
        }));
    }
    json!({
        "status": "completed",
        "model": "muse-spark-1.3",
        "output": output,
        "usage": {"input_tokens": 1000, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 100}
    })
}

/// The answer each case asks for: the second recording is c2, the computer's sound is Recording 1's.
fn ideal(message: &str) -> Vec<(&'static str, Value)> {
    let camera = |mode: &str, format: Value| json!({"clips": null, "mode": mode, "format": format});
    let bubble = |position: Value, size_step: Value, shape: Value| {
        json!({"clips": null, "position": position, "size": null, "sizeStep": size_step, "shape": shape,
               "border": null, "mirror": null, "format": null})
    };
    match message {
        "hide my camera" => vec![("setCameraMode", camera("hidden", Value::Null))],
        "make the camera a split screen in vertical" => {
            vec![("setCameraMode", camera("splitBottom", json!("portrait")))]
        }
        "move my bubble to the top left" => vec![(
            "adjustCamera",
            bubble(json!("topLeft"), Value::Null, Value::Null),
        )],
        "make the bubble bigger and square" => vec![(
            "adjustCamera",
            bubble(Value::Null, json!("bigger"), json!("square")),
        )],
        "mute the mic in the second recording" => vec![(
            "muteSource",
            json!({"source": "mic", "muted": true, "which": null, "clips": ["c2"]}),
        )],
        "turn the system sound down" => vec![(
            "setSourceVolume",
            json!({"source": "computerSound", "volume": 50, "which": null, "clips": null}),
        )],
        "separate the camera so I can cut it" => vec![("detachCamera", json!({"clips": null}))],
        "separate the sound" => vec![(
            "separateSound",
            json!({"source": "mic", "which": null, "clips": null}),
        )],
        other => panic!("no ideal answer for {other:?}"),
    }
}

#[tokio::test]
async fn ideal_answers_pass_every_recording_case() {
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(CASES).unwrap()).unwrap();
    let cases = cases["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    let server = MockServer::start().await;
    for case in cases {
        let message = case["message"].as_str().unwrap();
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(body_string_contains(message))
            .respond_with(ResponseTemplate::new(200).set_body_json(spark("OK.", &ideal(message))))
            .mount(&server)
            .await;
    }
    let env = [
        ("MODEL_API_KEY", KEY.to_owned()),
        ("KLIPOLO_META_BASE", server.uri()),
    ];
    let out = run(&env, &["--cases", CASES]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("8/8 passed (target 8); 8 model calls"),
        "{stdout}{stderr}"
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(!stdout.contains(KEY) && !stderr.contains(KEY));
}
