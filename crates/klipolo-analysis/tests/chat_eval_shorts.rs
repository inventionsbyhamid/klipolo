//! klipolo-chat's eval of a short made from one request (`evals/shorts_cases.json` on a long talk,
//! `evals/shorts_recording_cases.json` on screen recordings with clicks), run through the dev CLI `klipolo-chat-eval
//! --cases` against a mock Meta that answers each case with a plan as the prompt's recipe makes one: every case passes,
//! so the sets, their projects and their expectations hold together before a paid run, and the plan line says what
//! the plan came to.

use std::process::{Command, Output};

use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-meta-key-not-real";
const TALK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../klipolo-chat/evals/shorts_cases.json"
);
const RECORDINGS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../klipolo-chat/evals/shorts_recording_cases.json"
);
const SHORT: &str = "Convert this long video into a 60 second short with nice animations, sound effects and text animations";

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

fn range(start: f64, end: f64) -> Value {
    json!({"startSec": start, "endSec": end})
}

fn sticker(file: &str, at: f64) -> Value {
    json!({"file": file, "size": "small", "position": "top-right", "atSec": at, "durationSec": 2.5})
}

fn animate(enter: &str, sound: &str) -> Value {
    json!({"layer": "last", "enter": enter, "exit": null, "emphasis": "pulse", "sound": sound})
}

/// A short of the talk's first 155 s as the recipe plans it: four turns kept (about 60 s), vertical, following the
/// astronaut, word pop captions, a hook quoting the video, two pack stickers popping in with sounds, music under it.
fn talk_plan() -> Vec<(&'static str, Value)> {
    vec![
        (
            "findMoments",
            json!({"query": "best moments", "ranges": [{"startSec": 27.18, "endSec": 50.94, "label": "an answer"}]}),
        ),
        (
            "keepOnly",
            json!({"ranges": [range(0.06, 10.78), range(27.18, 50.94), range(53.18, 59.5), range(63.82, 83.18)]}),
        ),
        (
            "setFormat",
            json!({"format": "portrait", "keepOthers": false}),
        ),
        ("followSubject", json!({"description": "female astronaut"})),
        ("setCaptions", json!({"on": true, "style": "word-pop"})),
        (
            "addText",
            json!({"text": "it's also super rewarding", "preset": "title", "atSec": 0.06, "durationSec": 2.0,
                   "animation": "pop"}),
        ),
        ("addImage", sticker("Sparkles", 30.0)),
        ("animate", animate("pop", "Pop")),
        ("addImage", sticker("Lightbulb", 66.0)),
        ("animate", animate("bounce", "Ding")),
        (
            "addMusic",
            json!({"file": "Upbeat loop", "volume": null, "duck": null, "atSec": null}),
        ),
    ]
}

/// A short of the two takes as the recipe plans it (an answer from the first, the whole second), zooming in on their
/// clicks.
fn recording_plan() -> Vec<(&'static str, Value)> {
    vec![
        (
            "keepOnly",
            json!({"ranges": [range(27.18, 50.94), range(60.0, 105.0)]}),
        ),
        (
            "setFormat",
            json!({"format": "portrait", "keepOthers": false}),
        ),
        ("autoZoom", json!({"on": true, "scale": null})),
        ("setCaptions", json!({"on": true, "style": "bold-box"})),
        (
            "addText",
            json!({"text": "it's also super rewarding", "preset": "title", "atSec": 27.18, "durationSec": 2.0,
                   "animation": "typewriter"}),
        ),
        ("addImage", sticker("Thumbs up", 30.0)),
        ("animate", animate("pop", "Pop")),
        (
            "addMusic",
            json!({"file": "Chill beat", "volume": null, "duck": null, "atSec": null}),
        ),
    ]
}

async fn meta(answers: &[(&str, Vec<(&'static str, Value)>)]) -> MockServer {
    let server = MockServer::start().await;
    for (message, calls) in answers {
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(body_string_contains(*message))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(spark("Here is your short.", calls)),
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
async fn a_planned_short_of_the_talk_passes_and_says_what_it_came_to() {
    let server = meta(&[(SHORT, talk_plan())]).await;
    let out = run(&env(&server), &["--cases", TALK]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("PASS short-talk"), "{stdout}{stderr}");
    assert!(stdout.contains("1/1 passed"), "{stdout}{stderr}");
    let plan = stdout
        .lines()
        .find(|l| l.starts_with("     plan: "))
        .expect("a plan line");
    // The four turns, each snapped to the words it stands on.
    assert!(
        plan.contains("| proposed keepOnly |") && plan.contains("60.9 s once applied"),
        "{plan}"
    );
    assert!(plan.contains("0 refused"), "{plan}");
    assert_eq!(out.status.code(), Some(0));
    assert!(!stdout.contains(KEY) && !stderr.contains(KEY));
}

#[tokio::test]
async fn a_planned_short_of_the_recordings_zooms_on_their_clicks() {
    let server = meta(&[
        (SHORT, recording_plan()),
        (
            "zoom in on my clicks",
            vec![("autoZoom", json!({"on": true, "scale": null}))],
        ),
    ])
    .await;
    let out = run(&env(&server), &["--cases", RECORDINGS]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("2/2 passed"), "{stdout}{stderr}");
    let plan = stdout
        .lines()
        .find(|l| l.starts_with("     plan: "))
        .expect("a plan line");
    assert!(
        plan.contains("autoZoom") && plan.contains("0 refused"),
        "{plan}"
    );
    assert_eq!(out.status.code(), Some(0));
}

#[tokio::test]
async fn a_plan_missing_a_part_fails_naming_it() {
    let mut plan = talk_plan();
    plan.retain(|(name, _)| *name != "setCaptions");
    let server = meta(&[(SHORT, plan)]).await;
    let out = run(&env(&server), &["--cases", TALK]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL short-talk"), "{stdout}");
    assert!(stdout.contains("why: #0: no setCaptions"), "{stdout}");
    assert_eq!(out.status.code(), Some(1));
}
