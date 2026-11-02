//! Chat changes a recording's camera and sounds in the open project, end to end through the app's `ChatSession`: the
//! model is a fake that answers from a script, the project a `MemoryProjects` with its own history, so every undo step
//! is the real `Document`'s. Nothing here needs an analysis.
//!
//! The project: c1 plays Recording 1 (a screen, a camera, a mic and the computer's sound) for 20 s, c2 Recording 2 (a
//! screen, a camera and a mic) for 20 s.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use klipolo_analysis::providers::images::ImageMaker;
use klipolo_app::analysis::AssetAnalysis;
use klipolo_app::chat::{
    ChatOutcome, ChatRequest, ChatSession, Findings, MemoryProjects, Models, Readiness,
};
use klipolo_chat::{
    AnalysisState, BoxFuture, FunctionCall, GO_ON, Item, LanguageModel, ModelError, ModelRequest,
    ModelResponse,
};
use klipolo_model::ops::Op;
use klipolo_model::project::{
    Asset, AssetKind, FormatKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole,
    SourceRun,
};
use klipolo_model::speech::Levels;
use klipolo_model::time::Us;
use klipolo_model::timeline::{CameraLayout, CameraMode, Clip, ClipSource};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn file(id: &str, kind: AssetKind) -> Asset {
    let picture = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: format!("{id}.mov"),
        duration_us: secs(30.0),
        width: if picture { 1920 } else { 0 },
        height: if picture { 1080 } else { 0 },
        fps: if picture { 30.0 } else { 0.0 },
        rotation: None,
        has_audio: !picture,
        audio_lead_us: None,
        made_with: None,
    }
}

fn source(id: &str, role: SourceRole, file: &str) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs: vec![SourceRun {
            asset_id: file.into(),
            offset_us: 0,
            duration_us: secs(30.0),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

/// The project of the module's comment, landscape and portrait.
fn two_takes() -> Project {
    let mut p = Project::new("Takes");
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    for n in [1, 2] {
        let mut sources = vec![
            source("screen", SourceRole::Screen, &format!("screen{n}")),
            source("camera", SourceRole::Camera, &format!("camera{n}")),
            source("mic", SourceRole::Mic, &format!("mic{n}")),
        ];
        p.assets.push(file(&format!("screen{n}"), AssetKind::Video));
        p.assets.push(file(&format!("camera{n}"), AssetKind::Video));
        p.assets.push(file(&format!("mic{n}"), AssetKind::Audio));
        if n == 1 {
            sources.push(source("system", SourceRole::SystemAudio, "system1"));
            p.assets.push(file("system1", AssetKind::Audio));
        }
        p.recordings.push(Recording {
            id: format!("rec{n}"),
            kind: RecordingKind::Screen,
            sources,
            primary: "screen".into(),
            cursor: None,
            markers: Vec::new(),
            game: None,
        });
        let mut clip = Clip::new(
            ClipSource::Recording {
                id: format!("rec{n}"),
            },
            0,
            secs(20.0),
        );
        clip.id = format!("take{n}");
        clip.camera = Some(CameraLayout::default());
        p.timeline.main.push(clip);
    }
    p
}

/// Every file analyzed, with nothing found: none of these tools reads words or scenes.
struct Nothing;

impl Findings for Nothing {
    fn found(&self, _asset_id: &str) -> Option<Arc<AssetAnalysis>> {
        None
    }

    fn state(&self, _asset_id: &str) -> AnalysisState {
        AnalysisState::Done
    }

    fn levels(&self) -> HashMap<String, Arc<Levels>> {
        HashMap::new()
    }
}

/// A model that answers each message's first request from a script, in order. A request after it, with what came of
/// its calls, is answered as a model that is done: no words, no call; so is a first request past the script.
struct Script(Mutex<VecDeque<ModelResponse>>);

impl LanguageModel for Script {
    fn respond<'a>(
        &'a self,
        request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        let opening = matches!(request.input.last(), Some(Item::User { text }) if text != GO_ON);
        let scripted = if opening {
            self.0.lock().unwrap().pop_front()
        } else {
            None
        };
        let answer = scripted.unwrap_or(ModelResponse {
            text: String::new(),
            calls: Vec::new(),
            usage_usd: 0.0,
        });
        Box::pin(async move { Ok(answer) })
    }
}

/// A key that pays for `model`; no pictures.
struct Key(Arc<Script>);

impl Models for Key {
    fn ready(&self) -> BoxFuture<'_, Readiness> {
        let model = self.0.clone();
        Box::pin(async move { Readiness::Ready { model } })
    }

    fn images(&self) -> BoxFuture<'_, Result<Arc<dyn ImageMaker>, String>> {
        Box::pin(async { Err("No pictures here.".to_owned()) })
    }

    fn picture_usd(&self) -> f64 {
        0.01
    }

    fn recover(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

fn call(name: &str, arguments: Value) -> FunctionCall {
    FunctionCall {
        name: name.into(),
        call_id: format!("call-{name}"),
        arguments: arguments.to_string(),
    }
}

/// A chat on `project` whose model answers `text` with `calls`, once.
fn chat(
    project: Project,
    text: &str,
    calls: Vec<FunctionCall>,
) -> (ChatSession, Arc<MemoryProjects>) {
    let model = Arc::new(Script(Mutex::new(VecDeque::from([ModelResponse {
        text: text.into(),
        calls,
        usage_usd: 0.005,
    }]))));
    let projects =
        Arc::new(MemoryProjects::open(project, PathBuf::from("/tmp/takes.klipolo")).unwrap());
    let session = ChatSession::new(projects.clone(), Arc::new(Nothing), Arc::new(Key(model)));
    (session, projects)
}

fn request(text: &str) -> ChatRequest {
    serde_json::from_value(json!({
        "id": "m1",
        "text": text,
        "selection": null,
        "item": null,
        "playheadUs": 0,
        "format": null,
        "baseRevision": 0,
    }))
    .unwrap()
}

fn reply(outcome: &ChatOutcome) -> Value {
    let v = serde_json::to_value(outcome).unwrap();
    assert_eq!(v["type"], "reply", "{v:#}");
    v
}

#[tokio::test]
async fn a_message_changes_a_recordings_camera_and_sound_in_one_undo_step() {
    let before = two_takes();
    let (session, projects) = chat(
        before.clone(),
        "Your second recording's camera is hidden and its mic is muted.",
        vec![
            call(
                "setCameraMode",
                json!({"clips": ["c2"], "mode": "hidden", "format": null}),
            ),
            call(
                "muteSource",
                json!({"source": "mic", "muted": true, "which": null, "clips": ["c2"]}),
            ),
        ],
    );
    let out = session
        .send(request(
            "hide the camera and mute the mic in the second recording",
        ))
        .await
        .unwrap();
    let r = reply(&out);
    assert_eq!(
        r["text"],
        "Your second recording's camera is hidden and its mic is muted."
    );
    assert_eq!(r["applied"]["undo"]["code"], "chat.andMore");
    assert!(r["proposal"].is_null(), "additions apply at once");
    assert_eq!(projects.undo_steps(), 1, "one undo step");
    let p = projects.project();
    let take2 = &p.timeline.main[1];
    assert_eq!(take2.camera.as_ref().unwrap().mode, CameraMode::Hidden);
    let mic2 = p.recording("rec2").unwrap().source("mic").unwrap();
    assert!(take2.mix_for(mic2).muted);
    // The first recording is as it was.
    assert_eq!(p.timeline.main[0], before.timeline.main[0]);
    assert_eq!(p.recordings[0], before.recordings[0]);
    // One step back: both changes go.
    projects.step(false).unwrap();
    assert_eq!(*projects.project(), before);
}

#[tokio::test]
async fn the_reply_says_what_a_change_left_as_it_was() {
    // Recording 1's camera is its own layer already.
    let before = apply_ops(
        &two_takes(),
        &[Op::DetachSource {
            clip_id: "take1".into(),
            source_id: "camera".into(),
        }],
    )
    .unwrap();
    let (session, projects) = chat(
        before,
        "Your camera is hidden in landscape.",
        vec![call(
            "setCameraMode",
            json!({"clips": null, "mode": "hidden", "format": "landscape"}),
        )],
    );
    let out = session
        .send(request("hide my camera in landscape"))
        .await
        .unwrap();
    let r = reply(&out);
    assert_eq!(
        r["text"],
        "Your camera is hidden in landscape. Cameras that are their own layer were left as they are."
    );
    assert_eq!(r["applied"]["undo"]["code"], "chat.setCameraMode");
    let p = projects.project();
    let take2 = p.timeline.main[1].camera.as_ref().unwrap();
    assert_eq!(take2.mode_for(FormatKind::Landscape), CameraMode::Hidden);
    assert_eq!(
        take2.mode_for(FormatKind::Portrait),
        CameraMode::SplitBottom
    );
}

#[tokio::test]
async fn a_sound_the_video_lacks_is_said_and_nothing_changes() {
    let (session, projects) = chat(
        two_takes(),
        "The game sound is quieter now.",
        vec![call(
            "setSourceVolume",
            json!({"source": "gameSound", "volume": 50, "which": null, "clips": null}),
        )],
    );
    let out = session
        .send(request("turn the game sound down"))
        .await
        .unwrap();
    let r = reply(&out);
    assert_eq!(r["text"], "Your video has no game sound.");
    assert!(r["applied"].is_null());
    assert_eq!(projects.revision(), 0);
}
