//! An app changes a recording's camera and sounds through chat's own tools, on a `FakeHost` holding the chat crate's
//! two-clip fixture with a take added after it: c3 and c4 play Recording 1 (a screen, a camera and a mic) from 0 to
//! 10 s and from 10 to 20 s. Each change applies at once as one undo step the app made; what it left as it was is said
//! in its result; a refusal for the person is the app's to pass on, and nothing changes.

#[path = "support/agent.rs"]
mod agent;
#[path = "support/fake_host.rs"]
mod fake_host;

use std::sync::Arc;

use agent::{Agent, assert_refused, outcome, text};
use fake_host::FakeHost;
use klipolo_mcp::rpc::Mcp;
use klipolo_mcp::words::{DONE, READ_FIRST, fill};
use klipolo_model::Transaction;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::{
    Asset, AssetKind, FormatKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole,
    SourceRun,
};
use klipolo_model::time::Us;
use klipolo_model::timeline::{CameraLayout, CameraMode, Clip, ClipSource};
use serde_json::json;

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
        duration_us: secs(60.0),
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
            duration_us: secs(60.0),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

/// The two-clip fixture with the take as c3 and c4.
fn with_a_take() -> Arc<FakeHost> {
    let host = FakeHost::two_clips();
    host.change_project(|p| {
        for (id, kind) in [
            ("screen-file", AssetKind::Video),
            ("camera-file", AssetKind::Video),
            ("mic-file", AssetKind::Audio),
        ] {
            p.assets.push(file(id, kind));
        }
        p.recordings.push(Recording {
            id: "take".into(),
            kind: RecordingKind::Screen,
            sources: vec![
                source("screen", SourceRole::Screen, "screen-file"),
                source("camera", SourceRole::Camera, "camera-file"),
                source("mic", SourceRole::Mic, "mic-file"),
            ],
            primary: "screen".into(),
            cursor: None,
            markers: Vec::new(),
            game: None,
        });
        for (id, from, to) in [("t1", 0.0, 10.0), ("t2", 10.0, 20.0)] {
            let mut clip = Clip::new(
                ClipSource::Recording { id: "take".into() },
                secs(from),
                secs(to),
            );
            clip.id = id.into();
            clip.camera = Some(CameraLayout::default());
            p.timeline.main.push(clip);
        }
    });
    host
}

#[test]
fn an_app_mutes_a_recordings_mic_as_one_step_it_made() {
    let host = with_a_take();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let before = host.revision();
    let result = agent.call(
        "muteSource",
        json!({"source": "mic", "muted": true, "which": null, "clips": null}),
    );
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(text(&result), fill(DONE, &[("tools", "muteSource")]));
    assert_eq!(
        result["structuredContent"],
        json!({"outcome": "done", "revision": before + 1})
    );
    let project = host.project();
    let mic = &project
        .recording("take")
        .unwrap()
        .source("mic")
        .unwrap()
        .mix;
    assert!(mic.muted && mic.person_muted());
    let s = host.state();
    let [(client, tx)] = s.applied.as_slice() else {
        panic!("one batch: {:?}", s.applied)
    };
    assert_eq!(client, "Agent");
    assert_eq!(tx.label, "chat.muteSource");
    assert_eq!(
        tx.origin,
        Some(Origin::External {
            client: "Agent".into()
        })
    );
    let params = tx.params.as_ref().expect("params");
    assert_eq!(params.get("source"), Some(&ErrorParam::from("mic")));
    assert_eq!(params.get("mute"), Some(&ErrorParam::from("on")));
    assert_eq!(params.get("client"), Some(&ErrorParam::from("Agent")));
}

#[test]
fn an_app_names_a_recordings_clips_once_it_has_read_them() {
    let host = with_a_take();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let separate = || agent.call("detachCamera", json!({"clips": ["c3"]}));
    // Clip refs name the timeline: read it first.
    assert_refused(&separate(), READ_FIRST);
    let context = agent.call("getContext", json!({}));
    assert!(
        text(&context).contains("c3 144.0-154.0 Recording 1 (camera splitBottom, landscape bubble bottomRight circle, border on, mirrored; sounds mic)"),
        "{}",
        text(&context)
    );
    // Its second clip reads the same.
    assert!(
        text(&context).contains("c4 154.0-164.0 Recording 1 (as c3)"),
        "{}",
        text(&context)
    );
    let result = separate();
    assert_eq!(text(&result), fill(DONE, &[("tools", "detachCamera")]));
    let project = host.project();
    let t1 = &project.timeline.main[2];
    assert_eq!(t1.camera.as_ref().unwrap().mode, CameraMode::Hidden);
    let t2 = &project.timeline.main[3];
    assert_eq!(
        t2.camera,
        Some(CameraLayout::default()),
        "c4's camera stays"
    );
    // Again: the person's sentence, and nothing changes.
    let revision = host.revision();
    assert_refused(&separate(), "That camera is already its own layer.");
    assert_eq!(host.revision(), revision);
}

#[test]
fn an_app_is_told_what_its_call_left_as_it_was() {
    let host = with_a_take();
    // The person separated c3's camera by hand.
    host.person_edit(&Transaction {
        label: "undo.separateCamera".into(),
        ops: vec![Op::DetachSource {
            clip_id: "t1".into(),
            source_id: "camera".into(),
        }],
        ..Transaction::default()
    });
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let result = agent.call(
        "setCameraMode",
        json!({"clips": null, "mode": "hidden", "format": "landscape"}),
    );
    assert_eq!(outcome(&result), "done", "{result}");
    assert_eq!(
        text(&result),
        format!(
            "{} Cameras that are their own layer were left as they are.",
            fill(DONE, &[("tools", "setCameraMode")])
        )
    );
    let project = host.project();
    let t2 = project.timeline.main[3].camera.as_ref().unwrap();
    assert_eq!(t2.mode_for(FormatKind::Landscape), CameraMode::Hidden);
    assert_eq!(t2.mode_for(FormatKind::Portrait), CameraMode::SplitBottom);
}

#[test]
fn a_sound_the_video_lacks_is_the_persons_to_hear_about() {
    let host = with_a_take();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let revision = host.revision();
    let result = agent.call(
        "setSourceVolume",
        json!({"source": "computerSound", "volume": 50, "which": null, "clips": null}),
    );
    assert_refused(&result, "Your video has no computer sound.");
    assert_eq!(host.revision(), revision);
}

#[test]
fn an_app_may_zoom_in_on_clicks_and_hears_when_there_are_none() {
    // Listed as an edit, as chat's other tools are.
    let tools = klipolo_mcp::tools::list();
    let zoom = (tools.as_array().unwrap().iter())
        .find(|t| t["name"] == "autoZoom")
        .expect("autoZoom is listed");
    assert_eq!(zoom["annotations"]["readOnlyHint"], json!(false));
    // Klipolo knows no click in this take: the person's to hear, and nothing changes.
    let host = with_a_take();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let revision = host.revision();
    let result = agent.call("autoZoom", json!({"on": true, "scale": null}));
    assert_refused(
        &result,
        "These screen recordings have no clicks to zoom in on.",
    );
    assert_eq!(host.revision(), revision);
}
