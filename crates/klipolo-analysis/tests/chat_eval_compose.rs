//! klipolo-chat's eval of edits composed from the general tools (`evals/compose_cases.json`: custom motion, words by
//! unit, a layer's card fields and style, an effect, ducking), run through the dev CLI `klipolo-chat-eval --cases`
//! against a mock Meta that answers each case as it should be answered: every case passes, so the set, its project and
//! its expectations hold together before a paid run; and a wrong answer fails naming what it lacked.

use std::process::{Command, Output};

use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-meta-key-not-real";
const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../klipolo-chat/evals/compose_cases.json"
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

fn key(property: &str, at: f64, value: f64, ease: Value) -> Value {
    json!({"property": property, "atSec": at, "value": value, "ease": ease})
}

fn pair(key: &str, value: Value) -> Value {
    json!({"key": key, "value": value})
}

/// A volume key: a level at a time from the song's start, easing in and out.
fn level(at: f64, value: f64) -> Value {
    json!({"atSec": at, "value": value, "ease": "inOut"})
}

/// The answer each case asks for.
fn ideal(message: &str) -> Vec<(&'static str, Value)> {
    match message {
        "slide the title in from the left with a little overshoot" => vec![(
            "setMotion",
            json!({"layer": "t1", "keys": [key("x", 0.0, -0.6, Value::Null), key("x", 0.6, 0.0, json!("spring 0.3"))],
                   "presets": null}),
        )],
        "make the logo spin in and bounce" => vec![(
            "setMotion",
            json!({"layer": "i1", "keys": [
                key("rotation", 0.0, -360.0, Value::Null), key("rotation", 0.8, 0.0, json!("spring 0.5")),
                key("scale", 0.0, 0.0, Value::Null), key("scale", 0.8, 1.0, json!("spring 0.5"))
            ], "presets": null}),
        )],
        "make each word pop in one by one" => vec![(
            "animateText",
            json!({"layer": "t1", "set": [pair("by", json!("word")), pair("scale", json!(0)),
                   pair("opacity", json!(0)), pair("ease", json!("spring 0.4")), pair("stagger", json!(0.15))]}),
        )],
        "slowly zoom into the speaker over five seconds" => vec![(
            "cannotDo",
            json!({"reason": "Klipolo can't zoom slowly into the main video yet."}),
        )],
        "fade the music down while I'm talking" => vec![(
            "setLayer",
            json!({"layer": "m1", "set": [], "volumeKeys": [
                level(0.0, 0.5), level(1.0, 0.15), level(150.0, 0.15), level(152.0, 0.5)
            ]}),
        )],
        "duck the music between 0:10 and 0:20" => vec![(
            "setLayer",
            json!({"layer": "m1", "set": [], "volumeKeys": [
                level(9.5, 0.5), level(10.0, 0.15), level(20.0, 0.15), level(20.5, 0.5)
            ]}),
        )],
        "make the sticker float up and down gently" => vec![(
            "setMotion",
            json!({"layer": "i2", "keys": null, "presets": "loop float 0.6 2s"}),
        )],
        "make the title yellow with a black outline" => vec![(
            "styleText",
            json!({"layer": "t1", "set": [pair("color", json!("#FFD02E")), pair("outline", json!("#000000"))]}),
        )],
        "put the logo in the top right corner and make it a bit see-through" => vec![(
            "setLayer",
            json!({"layer": "i1", "set": [pair("box", json!("0.85 0.04 0.12 0.2")), pair("opacity", json!(0.7))]}),
        )],
        "add a soft vignette to the video" => vec![(
            "setEffects",
            json!({"target": "c1", "name": "vignette", "set": [pair("softness", json!(0.8))]}),
        )],
        other => panic!("no ideal answer for {other:?}"),
    }
}

const MESSAGES: [&str; 10] = [
    "slide the title in from the left with a little overshoot",
    "make the logo spin in and bounce",
    "make each word pop in one by one",
    "slowly zoom into the speaker over five seconds",
    "fade the music down while I'm talking",
    "make the sticker float up and down gently",
    "make the title yellow with a black outline",
    "put the logo in the top right corner and make it a bit see-through",
    "add a soft vignette to the video",
    "duck the music between 0:10 and 0:20",
];

async fn meta(answer: impl Fn(&str) -> Vec<(&'static str, Value)>) -> MockServer {
    let server = MockServer::start().await;
    for message in MESSAGES {
        // The message as the request carries it, quoted in JSON.
        let quoted = serde_json::to_string(message).unwrap();
        let quoted = &quoted[1..quoted.len() - 1];
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(body_string_contains(quoted))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(spark("Done.", &answer(message))),
            )
            .mount(&server)
            .await;
    }
    server
}

fn env(server: &MockServer) -> [(&'static str, String); 2] {
    [
        ("MODEL_API_KEY", KEY.to_owned()),
        ("KLIPOLO_META_BASE", server.uri()),
    ]
}

#[tokio::test]
async fn every_composed_edit_answered_as_it_should_be_passes() {
    let server = meta(ideal).await;
    let out = run(&env(&server), &["--cases", CASES]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("11/11 passed"), "{stdout}{stderr}");
    assert_eq!(out.status.code(), Some(0));
    assert!(!stdout.contains(KEY) && !stderr.contains(KEY));
}

#[tokio::test]
async fn a_preset_where_custom_motion_was_asked_fails_the_case() {
    // A plain slide with no overshoot, as the slide preset alone would give.
    let server = meta(|message| match message {
        "slide the title in from the left with a little overshoot" => vec![(
            "animate",
            json!({"layer": "t1", "enter": "fromLeft", "exit": null, "emphasis": null, "sound": null}),
        )],
        other => ideal(other),
    })
    .await;
    let out = run(
        &env(&server),
        &["--cases", CASES, "--only", "slide-title-overshoot"],
    )
    .await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL slide-title-overshoot"), "{stdout}");
    assert_eq!(out.status.code(), Some(1));
}

/// What a paid run answered, accepted where it is good behaviour: with captions on, asking whether "each word" means
/// the title or the captions; ducking the music "while I'm talking", which is what ducking does. With captions off, the
/// words to pop are the title's, so a question there fails. A stretch ("between 0:10 and 0:20") still wants volume keys.
#[tokio::test]
async fn a_question_about_captions_and_ducking_while_talking_pass_where_they_fit() {
    let server = meta(|message| match message {
        "make each word pop in one by one" => vec![(
            "askUser",
            json!({"question": "Do you mean the title Space station Q&A or the captions?"}),
        )],
        "fade the music down while I'm talking" | "duck the music between 0:10 and 0:20" => vec![(
            "setLayer",
            json!({"layer": "m1", "set": [pair("duck", json!(true))], "volumeKeys": null}),
        )],
        other => ideal(other),
    })
    .await;
    let only = |id: &'static str| ["--cases", CASES, "--only", id];
    for (id, passes) in [
        ("word-pop", true),
        ("word-pop-captions-off", false),
        ("music-down-talking", true),
        ("duck-music-stretch", false),
    ] {
        let out = run(&env(&server), &only(id)).await;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let said = if passes { "PASS" } else { "FAIL" };
        assert!(stdout.contains(&format!("{said} {id}")), "{id}: {stdout}");
    }
}
