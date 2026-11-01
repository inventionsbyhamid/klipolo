//! The recording tools: a recording's camera (`setCameraMode`, `adjustCamera`, `detachCamera`) and its recorded sounds
//! (`muteSource`, `setSourceVolume`, `separateSound`), read from the model's calls and compiled against a project
//! made here, `two_takes()`:
//! - c1 (clip "a"): Recording 1, 0 to 30 s, with a screen, a camera, a mic and the computer's sound; its camera as a
//!   take starts (a bubble at the bottom right, split under the screen in portrait);
//! - c2 (clip "b"): "broll.mov", 30 to 40 s, an imported file;
//! - c3 (clip "c"): Recording 2, 40 to 60 s, with a screen, a camera and a mic; its camera a bubble in every format.
//!
//! Each change is an addition: applied at once as one undo step. The tests judge the project the ops make, as a hand
//! edit through the same model ops would leave it.

use std::collections::{HashMap, VecDeque};
use std::pin::pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use klipolo_chat::compile::Compiled;
use klipolo_chat::tools::{BubbleChange, BubbleSize, Sound};
use klipolo_chat::{
    Audience, BoxFuture, FunctionCall, Inputs, LanguageModel, ModelError, ModelRequest,
    ModelResponse, Problem, Refs, ToolCall, View, build_context, compile, plan,
};
use klipolo_model::coded::ErrorParam;
use klipolo_model::layout::{Anchor, BoxN};
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::{
    Asset, AssetKind, FormatKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole,
    SourceRun,
};
use klipolo_model::time::Us;
use klipolo_model::timeline::{
    CameraLayout, CameraMode, CameraShape, Clip, ClipSource, ItemContent,
};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn function(name: &str, arguments: Value) -> FunctionCall {
    FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    }
}

/// One call as the model wrote it, read by the tools' own reader.
fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall::parse(&function(name, arguments.clone()))
        .unwrap_or_else(|e| panic!("{name} {arguments}: {e}"))
}

fn asset(id: &str, kind: AssetKind, length: f64, has_audio: bool) -> Asset {
    let picture = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: format!("{id}.mov"),
        duration_us: secs(length),
        width: if picture { 1920 } else { 0 },
        height: if picture { 1080 } else { 0 },
        fps: if picture { 30.0 } else { 0.0 },
        rotation: None,
        has_audio,
        audio_lead_us: None,
        made_with: None,
    }
}

/// A single-run source whose file is `file`, from the recording's start for `length` seconds.
fn source(id: &str, role: SourceRole, file: &str, length: f64) -> RecordingSource {
    let code = match role {
        SourceRole::Screen => "screen",
        SourceRole::Camera => "camera",
        SourceRole::Mic => "mic",
        SourceRole::SystemAudio => "computerSound",
        _ => "gameSound",
    };
    RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{code}"),
        runs: vec![SourceRun {
            asset_id: file.into(),
            offset_us: 0,
            duration_us: secs(length),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

fn recording(id: &str, sources: Vec<RecordingSource>) -> Recording {
    Recording {
        id: id.into(),
        kind: RecordingKind::Screen,
        sources,
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }
}

/// The project of the module's comment, in landscape alone.
fn two_takes() -> Project {
    let mut p = Project::new("Takes");
    p.id = "takes".into();
    p.assets = vec![
        asset("s1", AssetKind::Video, 60.0, false),
        asset("cam1", AssetKind::Video, 60.0, false),
        asset("mic1", AssetKind::Audio, 60.0, true),
        asset("sys1", AssetKind::Audio, 60.0, true),
        asset("broll", AssetKind::Video, 10.0, true),
        asset("s2", AssetKind::Video, 60.0, false),
        asset("cam2", AssetKind::Video, 60.0, false),
        asset("mic2", AssetKind::Audio, 60.0, true),
    ];
    p.recordings = vec![
        recording(
            "rec1",
            vec![
                source("screen", SourceRole::Screen, "s1", 60.0),
                source("camera", SourceRole::Camera, "cam1", 60.0),
                source("mic", SourceRole::Mic, "mic1", 60.0),
                source("system", SourceRole::SystemAudio, "sys1", 60.0),
            ],
        ),
        recording(
            "rec2",
            vec![
                source("screen", SourceRole::Screen, "s2", 60.0),
                source("camera", SourceRole::Camera, "cam2", 60.0),
                source("mic", SourceRole::Mic, "mic2", 60.0),
            ],
        ),
    ];
    let mut a = Clip::new(ClipSource::Recording { id: "rec1".into() }, 0, secs(30.0));
    a.id = "a".into();
    a.camera = Some(CameraLayout::default());
    let mut b = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
    b.id = "b".into();
    let mut c = Clip::new(ClipSource::Recording { id: "rec2".into() }, 0, secs(20.0));
    c.id = "c".into();
    c.camera = Some(CameraLayout {
        mode_overrides: Default::default(),
        ..CameraLayout::default()
    });
    p.timeline.main = vec![a, b, c];
    p
}

/// `two_takes()` with a portrait version too.
fn with_portrait() -> Project {
    let mut p = two_takes();
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p
}

fn inputs(p: &Project) -> Inputs {
    Inputs {
        refs: Refs::of(p),
        ..Inputs::default()
    }
}

fn compiled(p: &Project, calls: &[ToolCall]) -> Compiled {
    compile(p, &inputs(p), calls)
}

/// The project once `calls`' additions are applied; panics on a refusal or a proposal.
fn after(p: &Project, calls: &[ToolCall]) -> Project {
    let out = compiled(p, calls);
    assert_eq!(out.refusals, vec![], "nothing refused");
    assert!(out.proposal.is_none(), "an addition, never proposed");
    let tx = out.additions.expect("additions");
    assert_eq!(tx.origin, Some(Origin::Chat));
    apply_ops(p, &tx.ops).expect("the additions apply")
}

/// The one refusal of `calls`: its reason and who it is for.
fn refused(p: &Project, calls: &[ToolCall]) -> (String, Audience) {
    let out = compiled(p, calls);
    assert!(
        out.additions.is_none(),
        "nothing applied: {:?}",
        out.additions
    );
    assert_eq!(out.refusals.len(), 1, "{:?}", out.refusals);
    let r = &out.refusals[0];
    (r.reason.clone(), r.audience)
}

fn clip<'p>(p: &'p Project, id: &str) -> &'p Clip {
    p.timeline.main.iter().find(|c| c.id == id).unwrap()
}

fn camera<'p>(p: &'p Project, id: &str) -> &'p CameraLayout {
    clip(p, id).camera.as_ref().unwrap()
}

/// What clip `clip_id` plays of recording `rec`'s source `source`.
fn mix<'p>(p: &'p Project, clip_id: &str, rec: &str, source: &str) -> &'p SourceMix {
    let s = p.recording(rec).unwrap().source(source).unwrap();
    clip(p, clip_id).mix_for(s)
}

fn default_mix<'p>(p: &'p Project, rec: &str, source: &str) -> &'p SourceMix {
    &p.recording(rec).unwrap().source(source).unwrap().mix
}

fn label(out: &Compiled) -> (String, Option<Vec<(String, ErrorParam)>>) {
    let tx = out.additions.as_ref().unwrap();
    let params = (tx.params.clone()).map(|p| p.into_iter().collect::<Vec<_>>());
    (tx.label.clone(), params)
}

/// The layers playing file `asset`.
fn layers_of(p: &Project, asset: &str) -> Vec<klipolo_model::timeline::Item> {
    (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .filter(|i| matches!(i.content.played(), Some((a, _)) if a == asset))
        .cloned()
        .collect()
}

// ---- Reading the calls.

#[test]
fn the_camera_tools_read_the_models_own_words() {
    assert_eq!(
        call(
            "setCameraMode",
            json!({"clips": null, "mode": "splitBottom", "format": "portrait"})
        ),
        ToolCall::SetCameraMode {
            clips: None,
            mode: CameraMode::SplitBottom,
            format: Some(FormatKind::Portrait),
        }
    );
    assert_eq!(
        call(
            "adjustCamera",
            json!({"clips": ["c1", "c3-c5"], "position": "topLeft", "size": "large", "sizeStep": null,
                   "shape": "square", "border": false, "mirror": null, "format": null})
        ),
        ToolCall::AdjustCamera {
            clips: Some(vec!["c1".into(), "c3-c5".into()]),
            change: BubbleChange {
                position: Some(Anchor::TopLeft),
                size: Some(BubbleSize::Large),
                shape: Some(CameraShape::Square),
                border: Some(false),
                ..BubbleChange::default()
            },
        }
    );
    // An empty list of clips is every clip, as null is.
    assert_eq!(
        call("detachCamera", json!({"clips": []})),
        ToolCall::DetachCamera { clips: None }
    );
    assert_eq!(
        call("detachCamera", json!({})),
        ToolCall::DetachCamera { clips: None }
    );
}

#[test]
fn the_camera_words_are_the_models_own_names() {
    let tools = klipolo_chat::tools();
    let tool = |name: &str| {
        (tools.as_array().unwrap().iter())
            .find(|t| t["name"] == name)
            .unwrap()
            .clone()
    };
    let modes = &tool("setCameraMode")["parameters"]["properties"]["mode"]["enum"];
    assert_eq!(modes, &json!(["bubble", "splitBottom", "hidden"]));
    for word in modes.as_array().unwrap() {
        let _: CameraMode = serde_json::from_value(word.clone()).unwrap();
    }
    let adjust = tool("adjustCamera");
    let shapes = &adjust["parameters"]["properties"]["shape"]["enum"];
    assert_eq!(shapes, &json!(["circle", "rounded", "square", null]));
    let places = &adjust["parameters"]["properties"]["position"]["enum"];
    assert_eq!(places.as_array().unwrap().len(), 10);
    for word in (places.as_array().unwrap().iter()).chain(shapes.as_array().unwrap()) {
        if word.is_null() {
            continue;
        }
        let parsed: Result<Anchor, _> = serde_json::from_value(word.clone());
        let shape: Result<CameraShape, _> = serde_json::from_value(word.clone());
        assert!(parsed.is_ok() || shape.is_ok(), "{word}");
    }
}

#[test]
fn the_sound_tools_read_a_kind_a_percent_and_which_one() {
    assert_eq!(
        call(
            "muteSource",
            json!({"source": "mic", "muted": true, "which": null, "clips": ["c3"]})
        ),
        ToolCall::MuteSource {
            source: Sound::Mic,
            muted: true,
            which: None,
            clips: Some(vec!["c3".into()]),
        }
    );
    // The percent becomes the model's gain, to two decimals; which one is trimmed text.
    assert_eq!(
        call(
            "setSourceVolume",
            json!({"source": "computerSound", "volume": 62.5, "which": " 2 ", "clips": null})
        ),
        ToolCall::SetSourceVolume {
            source: Sound::ComputerSound,
            volume: 0.63,
            which: Some("2".into()),
            clips: None,
        }
    );
    assert_eq!(
        call(
            "separateSound",
            json!({"source": "gameSound", "which": "", "clips": null})
        ),
        ToolCall::SeparateSound {
            source: Sound::GameSound,
            which: None,
            clips: None,
        }
    );
    // The model allows 0 to 200 %: anything else is refused, never clamped.
    let loud = ToolCall::parse(&function(
        "setSourceVolume",
        json!({"source": "mic", "volume": 250, "which": null, "clips": null}),
    ))
    .unwrap_err();
    assert_eq!(loud.field.as_deref(), Some("volume"));
    assert_eq!(
        loud.to_string(),
        "setSourceVolume: volume must be between 0 and 200, not 250."
    );
    let kind = ToolCall::parse(&function(
        "muteSource",
        json!({"source": "voice", "muted": true, "which": null, "clips": null}),
    ))
    .unwrap_err();
    assert!(matches!(kind.problem, Problem::NotOneOf { .. }), "{kind}");
    let refs = ToolCall::parse(&function(
        "muteSource",
        json!({"source": "mic", "muted": true, "which": null, "clips": "c1"}),
    ))
    .unwrap_err();
    assert_eq!(
        refs.to_string(),
        "muteSource: clips must be a list or null, not text."
    );
}

#[test]
fn a_recording_tool_names_the_timeline_only_when_it_names_clips() {
    let every = call("detachCamera", json!({"clips": null}));
    assert!(!every.names_timeline());
    let some = call(
        "muteSource",
        json!({"source": "mic", "muted": true, "which": null, "clips": ["c1"]}),
    );
    assert!(some.names_timeline());
}

// ---- setCameraMode.

fn mode(clips: Value, mode: &str, format: Value) -> ToolCall {
    call(
        "setCameraMode",
        json!({"clips": clips, "mode": mode, "format": format}),
    )
}

#[test]
fn hide_my_camera_hides_every_camera_in_every_format() {
    let p = two_takes();
    let out = compiled(&p, &[mode(json!(null), "hidden", json!(null))]);
    assert_eq!(
        label(&out),
        (
            "chat.setCameraMode".to_owned(),
            Some(vec![("mode".to_owned(), ErrorParam::from("hidden"))])
        )
    );
    let done = after(&p, &[mode(json!(null), "hidden", json!(null))]);
    for id in ["a", "c"] {
        let cam = camera(&done, id);
        assert_eq!(cam.mode, CameraMode::Hidden, "{id}");
        assert!(cam.mode_overrides.is_empty(), "{id}");
        for f in [FormatKind::Landscape, FormatKind::Portrait] {
            assert_eq!(cam.mode_for(f), CameraMode::Hidden);
        }
    }
}

#[test]
fn a_split_is_for_the_vertical_version_only() {
    let p = with_portrait();
    let done = after(&p, &[mode(json!(["c3"]), "splitBottom", json!(null))]);
    let cam = camera(&done, "c");
    assert_eq!(cam.mode_for(FormatKind::Portrait), CameraMode::SplitBottom);
    assert_eq!(cam.mode_for(FormatKind::Landscape), CameraMode::Bubble);
    // The other recording's camera is as it was.
    assert_eq!(camera(&done, "a"), camera(&p, "a"));

    // A video with no vertical version is told to add it first.
    let (why, who) = refused(
        &two_takes(),
        &[mode(json!(null), "splitBottom", json!(null))],
    );
    assert_eq!(
        why,
        "Your video has no vertical (9:16) version. Add it first."
    );
    assert_eq!(who, Audience::Person);
}

#[test]
fn a_camera_already_shown_that_way_changes_nothing() {
    let p = two_takes();
    let out = compiled(&p, &[mode(json!(["c3"]), "bubble", json!(null))]);
    assert_eq!(out.additions, None);
    assert_eq!(out.unchanged, vec![0]);
    assert_eq!(out.refusals, vec![]);
}

#[test]
fn camera_refs_that_name_no_camera_go_back_to_the_model() {
    let p = two_takes();
    let (why, who) = refused(&p, &[mode(json!(["c9"]), "hidden", json!(null))]);
    assert_eq!(why, "setCameraMode: there is no clip c9.");
    assert_eq!(who, Audience::Model);
    let (why, who) = refused(&p, &[mode(json!(["c2"]), "hidden", json!(null))]);
    assert_eq!(why, "setCameraMode: none of those clips has a camera.");
    assert_eq!(who, Audience::Model);
    // A run as the context folds it names every clip in it.
    let done = after(&p, &[mode(json!(["c1-c3"]), "hidden", json!(null))]);
    assert_eq!(camera(&done, "a").mode, CameraMode::Hidden);
    assert_eq!(camera(&done, "c").mode, CameraMode::Hidden);
}

#[test]
fn a_video_with_no_camera_says_so() {
    let mut p = two_takes();
    for c in &mut p.timeline.main {
        c.camera = None;
    }
    let (why, who) = refused(&p, &[mode(json!(null), "hidden", json!(null))]);
    assert_eq!(why, "No clip in your video has a camera.");
    assert_eq!(who, Audience::Person);
}

#[test]
fn a_camera_that_is_its_own_layer_hides_and_shows_by_its_track() {
    let p = after(
        &two_takes(),
        &[call("detachCamera", json!({"clips": ["c1"]}))],
    );
    let layer = layers_of(&p, "cam1").remove(0);
    let track = |p: &Project| {
        (p.timeline.tracks.iter())
            .find(|t| t.items.iter().any(|i| i.id == layer.id))
            .unwrap()
            .clone()
    };
    assert!(!track(&p).hidden);
    let hidden = after(&p, &[mode(json!(["c1"]), "hidden", json!(null))]);
    assert!(track(&hidden).hidden, "its layer's track is hidden");
    // "Show my camera" shows that layer again, and says so.
    let out = compiled(&hidden, &[mode(json!(["c1"]), "bubble", json!(null))]);
    assert_eq!(
        out.notes,
        vec!["That camera is its own layer, so its layer shows again."]
    );
    let shown = apply_ops(&hidden, &out.additions.unwrap().ops).unwrap();
    assert!(!track(&shown).hidden);
    // A split can't reach it.
    let (why, who) = refused(
        &with_split(&p),
        &[mode(json!(["c1"]), "splitBottom", json!(null))],
    );
    assert_eq!(
        why,
        "That camera is its own layer now, so it can't go back into its clip."
    );
    assert_eq!(who, Audience::Person);
    // Beside a camera still in its clip, the other changes and the separate one is noted.
    let out = compiled(&p, &[mode(json!(null), "hidden", json!("landscape"))]);
    assert_eq!(
        out.notes,
        vec!["Cameras that are their own layer were left as they are."]
    );
}

fn with_split(p: &Project) -> Project {
    let mut p = p.clone();
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p
}

// ---- adjustCamera.

fn adjust(args: Value) -> ToolCall {
    let mut all = json!({"clips": null, "position": null, "size": null, "sizeStep": null, "shape": null,
                         "border": null, "mirror": null, "format": null});
    for (k, v) in args.as_object().unwrap() {
        all[k] = v.clone();
    }
    call("adjustCamera", all)
}

fn close(a: BoxN, b: BoxN) -> bool {
    [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)]
        .iter()
        .all(|(x, y)| (x - y).abs() < 1e-9)
}

#[test]
fn move_my_bubble_to_the_top_left_keeps_its_size() {
    let p = two_takes();
    let was = camera(&p, "c").bubble.rect;
    let done = after(
        &p,
        &[adjust(json!({"clips": ["c3"], "position": "topLeft"}))],
    );
    let bubble = &camera(&done, "c").bubble;
    assert_eq!(bubble.anchor, Anchor::TopLeft);
    let want = BoxN {
        x: 0.04,
        y: 0.04,
        w: was.w,
        h: was.h,
    };
    assert!(close(bubble.rect, want), "{:?}", bubble.rect);
    assert_eq!(camera(&done, "a"), camera(&p, "a"), "the other recording's");
}

#[test]
fn make_the_bubble_bigger_and_square() {
    let p = two_takes();
    let was = camera(&p, "a").bubble.rect;
    let done = after(
        &p,
        &[adjust(
            json!({"clips": ["c1"], "sizeStep": "bigger", "shape": "square"}),
        )],
    );
    let cam = camera(&done, "a");
    assert_eq!(cam.shape, CameraShape::Square);
    // Both sides grow by a quarter around the bottom right corner, which stays where it was.
    let r = cam.bubble.rect;
    assert!((r.w - was.w * 1.25).abs() < 1e-9 && (r.h - was.h * 1.25).abs() < 1e-9);
    assert!((r.x + r.w - (was.x + was.w)).abs() < 1e-9);
    assert!((r.y + r.h - (was.y + was.h)).abs() < 1e-9);
    // A size is the same bubble in every format: large is 0.28 of a 16:9 frame's width.
    let large = after(&p, &[adjust(json!({"clips": ["c1"], "size": "large"}))]);
    let r = camera(&large, "a").bubble.rect;
    let want = BoxN {
        x: 1.0 - 0.04 - 0.28,
        y: 1.0 - 0.04 - 0.28 * 1920.0 / 1080.0,
        w: 0.28,
        h: 0.28 * 1920.0 / 1080.0,
    };
    assert!(close(r, want), "{r:?}");
}

#[test]
fn a_bubble_placed_for_one_format_keeps_the_others() {
    let p = with_portrait();
    let done = after(
        &p,
        &[adjust(
            json!({"clips": ["c3"], "position": "topRight", "format": "portrait"}),
        )],
    );
    let bubble = &camera(&done, "c").bubble;
    assert_eq!(bubble.rect, camera(&p, "c").bubble.rect, "landscape stays");
    let portrait = bubble.overrides[&FormatKind::Portrait];
    assert!((portrait.y - 0.04).abs() < 1e-9 && (portrait.x + portrait.w - 0.96).abs() < 1e-9);
}

#[test]
fn a_bubble_change_that_names_nothing_goes_back_to_the_model() {
    let (why, who) = refused(&two_takes(), &[adjust(json!({}))]);
    assert_eq!(
        why,
        "adjustCamera: say what changes: position, size, sizeStep, shape, border or mirror."
    );
    assert_eq!(who, Audience::Model);
}

#[test]
fn a_camera_only_take_has_no_bubble_but_takes_the_mirror() {
    let mut p = two_takes();
    p.recordings[1].primary = "camera".into();
    let (why, who) = refused(
        &p,
        &[adjust(json!({"clips": ["c3"], "position": "topLeft"}))],
    );
    assert_eq!(
        why,
        "Recording 2's camera is the video itself, so it has no bubble."
    );
    assert_eq!(who, Audience::Person);
    let done = after(&p, &[adjust(json!({"clips": ["c3"], "mirror": false}))]);
    assert!(!camera(&done, "c").mirror);
}

#[test]
fn a_separate_cameras_layer_moves_and_takes_the_shape_as_its_corners() {
    let p = after(
        &two_takes(),
        &[call("detachCamera", json!({"clips": ["c1"]}))],
    );
    let out = compiled(
        &p,
        &[adjust(
            json!({"clips": ["c1"], "position": "topLeft", "shape": "square", "border": true}),
        )],
    );
    assert_eq!(
        out.notes,
        vec!["A camera that is its own layer has no border."]
    );
    let done = apply_ops(&p, &out.additions.unwrap().ops).unwrap();
    let layer = layers_of(&done, "cam1").remove(0);
    assert_eq!(layer.layout.anchor, Anchor::TopLeft);
    assert_eq!(layer.layout.radius, 0.0);
}

// ---- detachCamera.

#[test]
fn separate_the_camera_makes_it_a_layer_of_its_own() {
    let p = two_takes();
    let out = compiled(&p, &[call("detachCamera", json!({"clips": null}))]);
    assert_eq!(label(&out).0, "chat.detachCamera");
    let ops = &out.additions.as_ref().unwrap().ops;
    assert_eq!(
        ops,
        &vec![
            Op::DetachSource {
                clip_id: "a".into(),
                source_id: "camera".into()
            },
            Op::DetachSource {
                clip_id: "c".into(),
                source_id: "camera".into()
            },
        ]
    );
    let done = apply_ops(&p, ops).unwrap();
    assert_eq!(layers_of(&done, "cam1").len(), 1);
    assert_eq!(layers_of(&done, "cam2").len(), 1);
    assert_eq!(camera(&done, "a").mode, CameraMode::Hidden);
    // Again: it is already its own layer, and the person is told so.
    let (why, who) = refused(&done, &[call("detachCamera", json!({"clips": null}))]);
    assert_eq!(why, "That camera is already its own layer.");
    assert_eq!(who, Audience::Person);
}

#[test]
fn a_hidden_camera_stays_in_its_clip() {
    let p = after(&two_takes(), &[mode(json!(["c3"]), "hidden", json!(null))]);
    let out = compiled(&p, &[call("detachCamera", json!({"clips": null}))]);
    assert_eq!(
        out.additions.unwrap().ops,
        vec![Op::DetachSource {
            clip_id: "a".into(),
            source_id: "camera".into()
        }]
    );
    assert_eq!(
        out.notes,
        vec!["A hidden camera stays hidden in its clip. Show it first to make it its own layer."]
    );
}

// ---- muteSource and setSourceVolume.

fn mute(clips: Value, muted: bool) -> ToolCall {
    call(
        "muteSource",
        json!({"source": "mic", "muted": muted, "which": null, "clips": clips}),
    )
}

#[test]
fn mute_the_mic_everywhere_mutes_every_recordings_mic() {
    let p = two_takes();
    let out = compiled(&p, &[mute(json!(null), true)]);
    assert_eq!(
        label(&out),
        (
            "chat.muteSource".to_owned(),
            Some(vec![
                ("mute".to_owned(), ErrorParam::from("on")),
                ("source".to_owned(), ErrorParam::from("mic")),
            ])
        )
    );
    let done = apply_ops(&p, &out.additions.unwrap().ops).unwrap();
    for rec in ["rec1", "rec2"] {
        let m = default_mix(&done, rec, "mic");
        assert!(m.muted && m.person_muted(), "{rec}");
    }
    // The computer's sound is as it was.
    assert_eq!(default_mix(&done, "rec1", "system"), &SourceMix::default());
}

#[test]
fn muting_the_mic_in_one_recording_leaves_the_other_recordings_unchanged() {
    let p = two_takes();
    let done = after(&p, &[mute(json!(["c3"]), true)]);
    assert!(
        mix(&done, "c", "rec2", "mic").muted,
        "the second recording's clip"
    );
    // Its default and the first recording are as they were.
    assert_eq!(
        default_mix(&done, "rec2", "mic"),
        default_mix(&p, "rec2", "mic")
    );
    assert_eq!(done.recording("rec1"), p.recording("rec1"));
    assert_eq!(clip(&done, "a"), clip(&p, "a"));
    // Unmuting it there takes the clip's own mix away: it follows the default again.
    let back = after(&done, &[mute(json!(["c3"]), false)]);
    assert!(clip(&back, "c").source_mix.is_empty());
    assert_eq!(back, p);
}

#[test]
fn turn_the_system_sound_down() {
    let p = two_takes();
    let volume = call(
        "setSourceVolume",
        json!({"source": "computerSound", "volume": 40, "which": null, "clips": null}),
    );
    let out = compiled(&p, std::slice::from_ref(&volume));
    assert_eq!(
        label(&out),
        (
            "chat.setSourceVolume".to_owned(),
            Some(vec![
                ("percent".to_owned(), ErrorParam::Number(40.0)),
                ("source".to_owned(), ErrorParam::from("computerSound")),
            ])
        )
    );
    let done = after(&p, &[volume]);
    assert_eq!(default_mix(&done, "rec1", "system").volume, 0.4);
    assert_eq!(done.recording("rec2"), p.recording("rec2"));
}

#[test]
fn everywhere_reaches_the_clips_with_a_mix_of_their_own() {
    let p = after(&two_takes(), &[mute(json!(["c1"]), true)]);
    assert!(mix(&p, "a", "rec1", "mic").muted);
    // Unmuting everywhere unmutes it there too, and the clip follows the default again.
    let done = after(&p, &[mute(json!(null), false)]);
    assert!(!mix(&done, "a", "rec1", "mic").muted);
    assert!(clip(&done, "a").source_mix.is_empty());
}

#[test]
fn a_sound_the_video_lacks_is_said_to_the_person() {
    let p = two_takes();
    let game = call(
        "muteSource",
        json!({"source": "gameSound", "muted": true, "which": null, "clips": null}),
    );
    assert_eq!(
        refused(&p, &[game]),
        ("Your video has no game sound.".to_owned(), Audience::Person)
    );
    assert_eq!(
        refused(&p, &[mute(json!(["c2"]), true)]),
        (
            "That clip is an imported file, not a recording, so it has no mic.".to_owned(),
            Audience::Person
        )
    );
    // The camera of a take has no sound of its own: Klipolo's picture files carry none.
    let camera_sound = call(
        "muteSource",
        json!({"source": "videoSound", "muted": true, "which": null, "clips": null}),
    );
    assert_eq!(
        refused(&p, &[camera_sound]).0,
        "Your video has no camera sound."
    );
}

#[test]
fn which_mic_when_a_recording_has_two() {
    let mut p = two_takes();
    p.assets.push(asset("mic1b", AssetKind::Audio, 60.0, true));
    p.recordings[0]
        .sources
        .push(source("mic-2", SourceRole::Mic, "mic1b", 60.0));
    let second = call(
        "muteSource",
        json!({"source": "mic", "muted": true, "which": "2", "clips": ["c1"]}),
    );
    let done = after(&p, &[second]);
    assert!(mix(&done, "a", "rec1", "mic-2").muted);
    assert!(!mix(&done, "a", "rec1", "mic").muted);
    // As the context names it.
    let named = call(
        "muteSource",
        json!({"source": "mic", "muted": true, "which": "Mic 2", "clips": ["c1"]}),
    );
    assert_eq!(after(&p, &[named]), done);
    let third = call(
        "muteSource",
        json!({"source": "mic", "muted": true, "which": "third", "clips": ["c1"]}),
    );
    assert_eq!(
        refused(&p, &[third]),
        ("Which mic: 1, 2?".to_owned(), Audience::Person)
    );
}

// ---- separateSound.

fn separate(source: &str, clips: Value) -> ToolCall {
    call(
        "separateSound",
        json!({"source": source, "which": null, "clips": clips}),
    )
}

#[test]
fn separate_the_sound_puts_it_on_a_track_of_its_own() {
    let p = two_takes();
    let out = compiled(&p, &[separate("mic", json!(null))]);
    assert_eq!(
        label(&out),
        (
            "chat.separateSound".to_owned(),
            Some(vec![("source".to_owned(), ErrorParam::from("mic"))])
        )
    );
    let done = apply_ops(&p, &out.additions.unwrap().ops).unwrap();
    for (file, clip_id, rec) in [("mic1", "a", "rec1"), ("mic2", "c", "rec2")] {
        let layers = layers_of(&done, file);
        assert_eq!(layers.len(), 1, "{file}");
        assert!(matches!(layers[0].content, ItemContent::Audio { .. }));
        // The clip no longer plays it, so it isn't heard twice.
        assert!(mix(&done, clip_id, rec, "mic").muted);
    }
    // Again: already on its own track.
    assert_eq!(
        refused(&done, &[separate("mic", json!(null))]),
        (
            "The mic is already on its own track there.".to_owned(),
            Audience::Person
        )
    );
    // Its mix there is its layers' now: a mute everywhere leaves them, and says so.
    let out = compiled(&done, &[mute(json!(["c1"]), false)]);
    assert_eq!(
        out.refusals[0].reason,
        "Where the mic is on its own track, its layers stay as they are."
    );
}

#[test]
fn the_camera_sound_stays_with_its_picture() {
    assert_eq!(
        refused(&two_takes(), &[separate("videoSound", json!(null))]),
        (
            "The camera's sound stays with its picture. You can mute it or change its volume instead.".to_owned(),
            Audience::Person
        )
    );
}

#[test]
fn a_sped_up_clip_keeps_its_sound() {
    let mut p = two_takes();
    p.timeline.main[0].speed = 2.0;
    let out = compiled(&p, &[separate("mic", json!(null))]);
    assert_eq!(
        out.additions.as_ref().unwrap().ops,
        vec![Op::DetachSource {
            clip_id: "c".into(),
            source_id: "mic".into()
        }]
    );
    assert_eq!(
        out.notes,
        vec![
            "The sped-up clip at 0:00 was left as it is: separated sound plays at normal speed only."
        ]
    );
    assert_eq!(
        refused(&p, &[separate("mic", json!(["c1"]))]),
        (
            "Set the clip back to normal speed before separating its sound.".to_owned(),
            Audience::Person
        )
    );
}

// ---- One answer, one step.

#[test]
fn several_recording_changes_are_one_undo_step() {
    let p = with_portrait();
    let calls = [
        mode(json!(null), "splitBottom", json!(null)),
        adjust(json!({"sizeStep": "bigger"})),
        mute(json!(["c3"]), true),
    ];
    let out = compiled(&p, &calls);
    let tx = out.additions.as_ref().unwrap();
    assert_eq!(tx.label, "chat.andMore");
    assert_eq!(out.refusals, vec![]);
    let done = apply_ops(&p, &tx.ops).unwrap();
    assert!(mix(&done, "c", "rec2", "mic").muted);
    assert_eq!(
        camera(&done, "c").mode_for(FormatKind::Portrait),
        CameraMode::SplitBottom
    );
}

// ---- The reply.

/// A model that answers once, from a script.
struct Once(Mutex<VecDeque<ModelResponse>>);

impl LanguageModel for Once {
    fn respond<'a>(
        &'a self,
        _request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        let answer =
            (self.0.lock().unwrap().pop_front()).ok_or_else(|| ModelError::other("no more"));
        Box::pin(async move { answer })
    }
}

fn now<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("the planner waited on a fake that answers at once"),
    }
}

#[test]
fn the_reply_says_what_was_left_as_it_was() {
    let p = after(
        &two_takes(),
        &[call("detachCamera", json!({"clips": ["c1"]}))],
    );
    let model = Once(Mutex::new(VecDeque::from([ModelResponse {
        text: "Your camera is hidden in landscape.".into(),
        calls: vec![function(
            "setCameraMode",
            json!({"clips": null, "mode": "hidden", "format": "landscape"}),
        )],
        usage_usd: 0.0,
    }])));
    let ctx = build_context(
        &p,
        &Default::default(),
        &HashMap::new(),
        &HashMap::new(),
        &View::default(),
        &Default::default(),
    );
    let i = Inputs {
        refs: ctx.refs.clone(),
        ..Inputs::default()
    };
    let out = now(plan(
        &model,
        &[],
        &ctx,
        "hide my camera in landscape",
        &i,
        &p,
    ))
    .unwrap();
    assert_eq!(
        out.reply,
        "Your camera is hidden in landscape. Cameras that are their own layer were left as they are."
    );
    assert!(out.compiled.additions.is_some());
    // Nothing done, but something to say: the person is told why, and the model's text is left out.
    let model = Once(Mutex::new(VecDeque::from([ModelResponse {
        text: "Your camera is its own layer now.".into(),
        calls: vec![function("detachCamera", json!({"clips": ["c1"]}))],
        usage_usd: 0.0,
    }])));
    let out = now(plan(&model, &[], &ctx, "separate my camera", &i, &p)).unwrap();
    assert_eq!(out.reply, "That camera is already its own layer.");
}

// ---- The context.

/// The context's clip lines for `p`, playhead at 0, nothing analyzed.
fn clip_lines(p: &Project) -> Vec<String> {
    let ctx = build_context(
        p,
        &Default::default(),
        &HashMap::new(),
        &HashMap::new(),
        &View::default(),
        &Default::default(),
    );
    (ctx.text.lines())
        .skip_while(|l| *l != "Clips on the main track:")
        .skip(1)
        .take_while(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_context_names_each_recording_with_its_camera_and_sounds() {
    assert_eq!(
        clip_lines(&two_takes()),
        [
            "c1 0.0-30.0 Recording 1 (camera bubble bottomRight circle, border on, mirrored; sounds mic, computer sound)",
            "c2 30.0-40.0 broll.mov",
            "c3 40.0-60.0 Recording 2 (camera bubble bottomRight circle, border on, mirrored; sounds mic)",
        ]
    );
    // A mode for a format the video has is said; one for a format it doesn't is not.
    assert_eq!(
        clip_lines(&with_portrait())[0],
        "c1 0.0-30.0 Recording 1 (camera bubble bottomRight circle, border on, mirrored, portrait splitBottom; sounds mic, \
         computer sound)"
    );
}

#[test]
fn the_context_says_what_the_person_changed() {
    let p = after(
        &two_takes(),
        &[
            mode(json!(["c3"]), "hidden", json!(null)),
            mute(json!(["c3"]), true),
            call(
                "setSourceVolume",
                json!({"source": "computerSound", "volume": 50, "which": null, "clips": null}),
            ),
            adjust(
                json!({"clips": ["c1"], "position": "topLeft", "shape": "rounded", "border": false, "mirror": false}),
            ),
        ],
    );
    assert_eq!(
        clip_lines(&p),
        [
            "c1 0.0-30.0 Recording 1 (camera bubble topLeft rounded, border off, not mirrored; sounds mic, computer sound \
             at 50%)",
            "c2 30.0-40.0 broll.mov",
            "c3 40.0-60.0 Recording 2 (camera hidden; sounds mic muted)",
        ]
    );
    let p = after(
        &two_takes(),
        &[
            call("detachCamera", json!({"clips": ["c1"]})),
            separate("mic", json!(["c1"])),
        ],
    );
    assert_eq!(
        clip_lines(&p)[0],
        "c1 0.0-30.0 Recording 1 (camera on its own layer; sounds mic on its own track, computer sound)"
    );
}

#[test]
fn the_context_numbers_several_mics_and_names_a_camera_only_take() {
    let mut p = two_takes();
    p.assets.push(asset("mic1b", AssetKind::Audio, 60.0, true));
    p.recordings[0]
        .sources
        .push(source("mic-2", SourceRole::Mic, "mic1b", 60.0));
    p.recordings[1].primary = "camera".into();
    p.timeline.main[0].speed = 2.0;
    assert_eq!(
        clip_lines(&p),
        [
            "c1 0.0-15.0 Recording 1 (speed 2x; camera bubble bottomRight circle, border on, mirrored; sounds mic, \
             computer sound, mic 2)",
            "c2 15.0-25.0 broll.mov",
            "c3 25.0-45.0 Recording 2 (camera only, mirrored; sounds mic)",
        ]
    );
}

// ---- The prompt.

#[test]
fn the_prompt_says_how_a_recordings_camera_and_sounds_read_and_change() {
    let prompt = klipolo_chat::prompt::system_prompt();
    let line = (prompt.lines())
        .find(|l| l.contains("Recording 1"))
        .expect("a line on recordings");
    for needle in ["camera and sounds", "clips null", "every recording"] {
        assert!(line.contains(needle), "{line:?} says {needle:?}");
    }
    // One short line, in the rules' style.
    assert!(line.starts_with("- "), "{line:?}");
    assert!(klipolo_chat::estimate_tokens(line) <= 40, "{line:?}");
}
