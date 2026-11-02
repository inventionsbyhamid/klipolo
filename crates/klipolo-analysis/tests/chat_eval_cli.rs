//! The dev CLI `klipolo-chat-eval`, run as a process against a mock Meta: it sends klipolo-chat's eval cases with the
//! key from the environment, prints pass or fail per case and the total cost, and never prints the key.

use std::process::{Command, Output};

use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-chat-eval-test-key-0123456789";

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

/// A mock Meta that answers every message with one `undo` call.
async fn meta_that_undoes() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "completed",
            "model": "muse-spark-1.3",
            "output": [
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Undone."}]},
                {"type": "function_call", "call_id": "call_1", "name": "undo", "arguments": "{}"}
            ],
            "usage": {"input_tokens": 1000, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 100}
        })))
        .mount(&server)
        .await;
    server
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn every_case_is_judged_and_the_cost_added_up_without_the_key() {
    let server = meta_that_undoes().await;
    let env = [
        ("MODEL_API_KEY", KEY.to_owned()),
        ("KLIPOLO_META_BASE", server.uri()),
    ];
    let out = run(&env, &[]).await;
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    assert!(stdout.contains("PASS undo"), "{stdout}");
    assert!(stdout.contains("FAIL vertical"), "{stdout}");
    assert!(stdout.contains("     reply: Undone."), "{stdout}");
    // Two model calls a case: the stub answers what came of its calls with the same calls, which ends the message.
    assert!(
        stdout.contains("1/39 passed (target 37); 78 model calls; total $0.12"),
        "{stdout}"
    );
    assert!(!stdout.contains(KEY) && !stderr.contains(KEY));
    assert_eq!(server.received_requests().await.unwrap().len(), 78);

    // One case on its own must pass.
    let out = run(&env, &["--only", "undo"]).await;
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    assert!(stdout.contains("1/1 passed (target 1)"), "{stdout}");
}

#[tokio::test]
async fn a_failed_case_counts_its_model_calls_and_says_why() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(402)
                .set_body_json(json!({"error": {"message": "Your key has no credit left."}})),
        )
        .mount(&server)
        .await;
    let env = [
        ("MODEL_API_KEY", KEY.to_owned()),
        ("KLIPOLO_META_BASE", server.uri()),
    ];
    let out = run(&env, &["--only", "vertical", "--only", "undo"]).await;
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("why: Budget: Your key has no credit left."),
        "{stdout}"
    );
    assert!(
        stdout.contains("0/2 passed (target 2); 2 model calls; total $0.0000"),
        "{stdout}"
    );
}

#[tokio::test]
async fn without_a_key_nothing_is_sent() {
    let server = meta_that_undoes().await;
    let out = run(&[("KLIPOLO_META_BASE", server.uri())], &[]).await;
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(2), "{stdout}{stderr}");
    assert!(stderr.contains("Set MODEL_API_KEY."), "{stderr}");
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

/// `setClip` on `clip` with `values`, every other value null.
fn clip(clip: &str, values: Value) -> (&'static str, Value) {
    let mut args = json!({
        "clip": clip, "speed": null, "volume": null, "muted": null, "framing": null, "framingFormat": null,
    });
    for (k, v) in values.as_object().unwrap() {
        args[k] = v.clone();
    }
    ("setClip", args)
}

/// The answer each case asks for, with times from clip A's transcript and scene log.
fn ideal(message: &str) -> Vec<(&'static str, Value)> {
    let square = ("setFormat", json!({"format": "square", "keepOthers": true}));
    let portrait = (
        "setFormat",
        json!({"format": "portrait", "keepOthers": true}),
    );
    let pauses = ("removeSilences", json!({"minPauseSec": 1}));
    let ask = ("askUser", json!({"question": "Which part do you mean?"}));
    let cut = |a: f64, b: f64| ("cutRange", json!({"startSec": a, "endSec": b}));
    let karaoke = ("setCaptions", json!({"on": true, "style": "karaoke"}));
    match message {
        "Make this vertical for TikTok" => vec![portrait],
        "Cut out the underwater training footage at the start" => vec![cut(0.0, 13.5)],
        "Remove the part where Peyton asks her question" => vec![cut(85.5, 95.5)],
        "blur it" | "Make it better" | "Cut the boring part" => vec![ask],
        // A title card with its picture named but not its words: ask for them.
        "make a title card with trams behind it" => vec![(
            "askUser",
            json!({"question": "What should the title card say?"}),
        )],
        "Get rid of the ums and uhs" => vec![("removeFillers", json!({}))],
        "Add those big bold word-by-word captions that light up as people talk" => vec![karaoke],
        "When we go vertical keep the camera on the woman astronaut" => vec![
            portrait,
            ("followSubject", json!({"description": "woman astronaut"})),
        ],
        "Make a vertical short of this and keep whoever is talking in frame" => {
            vec![
                portrait,
                ("followSubject", json!({"description": "person"})),
            ]
        }
        "Blur the kids' faces, we don't have consent from the school" => {
            vec![("blurObjects", json!({"description": "kids' faces"}))]
        }
        "Square for Instagram, and remove any pause longer than 1 second" => vec![square, pauses],
        "Delete everything the male astronaut says about dealing with challenges" => {
            vec![cut(98.0, 149.5)]
        }
        "keep only the part where she floats sideways" => vec![(
            "keepOnly",
            json!({"ranges": [{"startSec": 67.0, "endSec": 85.0}]}),
        )],
        "cut from 0:13 to the end of her answer" => vec![cut(13.0, 83.2)],
        "make captions karaoke and fix Caden to Kaiden" => vec![
            karaoke,
            (
                "fixCaptionWords",
                json!({"find": "Caden", "replace": "Kaiden"}),
            ),
        ],
        "remove pauses over a second and make it square" => vec![pauses, square],
        "blur the license plate" => vec![("blurObjects", json!({"description": "license plate"}))],
        "undo that" => vec![("undo", json!({}))],
        // The media cases: the person's lofi file from the library, the title card template with their words, a
        // whoosh at each scene cut, and a sticker when she floats.
        "add lofi as background music, quieter when I talk" => vec![(
            "addMusic",
            json!({"file": "lofi-beat.mp3", "volume": null, "duck": true, "atSec": 0.0}),
        )],
        // A title card asked for by name; a plain title over the video.
        "put a title card 'Day 1 in Lisbon' for 3 seconds at the start" => vec![(
            "makeTitleCard",
            json!({"title": "Day 1 in Lisbon", "prompt": "Lisbon", "atSec": 0.0, "durationSec": 3.0}),
        )],
        "put a title 'Day 1 in Lisbon' at the start" => vec![(
            "addText",
            json!({"text": "Day 1 in Lisbon", "preset": "title", "atSec": 0.0, "durationSec": null}),
        )],
        "a whoosh at every topic change" => vec![(
            "addSound",
            json!({"name": "Whoosh", "atSec": [12.8, 23.0, 51.5, 62.5, 85.5, 95.5, 149.5]}),
        )],
        "add a sticker of a paper plane in the top right when she mentions flying" => vec![(
            "makeSticker",
            json!({"prompt": "paper plane", "position": "top-right", "atSec": 67.0, "durationSec": null}),
        )],
        // A sound on the word it goes with, by the words said there.
        "add a pop sound right when she says float" => vec![(
            "addSound",
            json!({"name": "Pop", "atWords": ["float and do"], "atSec": null}),
        )],
        // Words on screen in a text preset, in the person's own words.
        "add a lower third that says Student questions at 13.5 seconds" => vec![(
            "addText",
            json!({"text": "Student questions", "preset": "lower-third", "atSec": 13.5, "durationSec": null}),
        )],
        "put Thanks for watching at the bottom for the last 3 seconds" => vec![(
            "addText",
            json!({"text": "Thanks for watching", "preset": "caption", "atSec": 152.04, "durationSec": 3.0}),
        )],
        // A title card whose background the person didn't name: the model chooses one.
        "make a title card that says Day 1 in Lisbon" => vec![(
            "makeTitleCard",
            json!({"title": "Day 1 in Lisbon", "prompt": "Lisbon street at golden hour", "atSec": null, "durationSec": null}),
        )],
        // The clip tools, on clip A in clips (an intro of b-roll, a middle and an end), beside a song's video, or under
        // music and a b-roll layer.
        "slow the intro down to half speed" => vec![clip("c1", json!({"speed": 0.5}))],
        "speed up the middle part 1.5x" => vec![clip("c2", json!({"speed": 1.5}))],
        "mute the b-roll" => vec![clip("c1", json!({"muted": true}))],
        "turn the music clip down a bit" => vec![clip("c2", json!({"volume": 0.6}))],
        "fill the vertical frame" => vec![clip(
            "all",
            json!({"framing": "fill", "framingFormat": "portrait"}),
        )],
        "add a crossfade between the two clips" => vec![(
            "setTransition",
            json!({"after": "c1", "kind": "dissolve", "durationSec": null}),
        )],
        "turn the music down" => vec![clip("m1", json!({"volume": 0.15}))],
        "mute the music" => vec![clip("m1", json!({"muted": true}))],
        "slow the b-roll layer to half speed" => vec![clip("v1", json!({"speed": 0.5}))],
        "remove the transitions" => vec![(
            "setTransition",
            json!({"after": "all", "kind": "none", "durationSec": null}),
        )],
        other => panic!("no ideal answer for {other:?}"),
    }
}

#[tokio::test]
async fn ideal_answers_pass_every_case() {
    let cases: Value =
        serde_json::from_str(include_str!("../../klipolo-chat/evals/cases.json")).unwrap();
    let server = MockServer::start().await;
    for case in cases["cases"].as_array().unwrap() {
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
    let out = run(&env, &[]).await;
    let (stdout, stderr) = text(&out);
    assert!(stdout.contains("39/39 passed"), "{stdout}{stderr}");
    // A case that allows more than one answer says which passed: the first for each (a question is the second).
    let sticker = &stdout[stdout.find("PASS plane-sticker").expect("the sticker case")..];
    assert!(
        sticker.contains("\n     passed on alternative #0 of 2\n"),
        "{stdout}"
    );
    assert_eq!(stdout.matches("     passed on alternative ").count(), 4);
    assert_eq!(stdout.matches("     passed on alternative #0").count(), 4);
    assert_eq!(out.status.code(), Some(0));
    // Two calls each (the stub answers what came of its calls with the same calls, which ends the message): clip A's
    // missing filler words are told to the person, never sent back.
    assert!(stdout.contains("78 model calls"), "{stdout}");
    assert!(
        stdout.contains("     reply: There are no filler words to remove."),
        "{stdout}"
    );
}
