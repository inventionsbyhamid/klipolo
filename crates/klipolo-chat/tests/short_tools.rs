//! The tools a short is made with: a layer's animation (`animate`: how it comes in, goes out and moves while it shows,
//! with a sound as it comes in), words that come in with an animation (`addText`), and the refs the context gives the
//! words and pictures on screen (`t1`, `i1`). The project is made here:
//! - `talk.mov`, 30 s, played whole, landscape. Words are said from 0.5 s to 13.0 s ("Day one in Lisbon." from 0.5 to
//!   2.0 s); topics change at 10.0 and 20.0 s.
//! - The library: Klipolo's pack sounds `Whoosh` and `Pop` and sticker `Heart`, not in the project yet.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use klipolo_chat::tools::{Emphasis, Entrance, Exit, LayerRef};
use klipolo_chat::{
    AnalysisState, Audience, Compiled, FunctionCall, Inputs, Job, Library, LibraryItem, MediaKind,
    Problem, Refs, SceneEntry, SceneLog, ToolCall, View, build_context, compile, sticker_ops,
};
use klipolo_model::media::text::{TextPreset, add_text_ops};
use klipolo_model::motion::{Motion, MotionUse};
use klipolo_model::params::ParamValue;
use klipolo_model::project::Asset;
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, TrackKind};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn parse(name: &str, arguments: Value) -> Result<ToolCall, klipolo_chat::ToolError> {
    ToolCall::parse(&FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
}

fn call(name: &str, arguments: Value) -> ToolCall {
    parse(name, arguments.clone()).unwrap_or_else(|e| panic!("{name} {arguments}: {e}"))
}

fn asset(id: &str, kind: &str, name: &str, length: f64, size: (u32, u32), audio: bool) -> Asset {
    serde_json::from_value(json!({
        "id": id,
        "kind": kind,
        "path": format!("/media/{name}"),
        "name": name,
        "durationUs": secs(length),
        "width": size.0,
        "height": size.1,
        "fps": if kind == "video" { 30.0 } else { 0.0 },
        "hasAudio": audio,
    }))
    .unwrap()
}

fn whoosh() -> Asset {
    asset("pack-whoosh", "audio", "whoosh.wav", 0.9, (0, 0), true)
}

fn pop() -> Asset {
    asset("pack-pop", "audio", "pop.wav", 0.16, (0, 0), true)
}

fn heart() -> Asset {
    asset("pack-heart", "image", "heart.png", 5.0, (512, 512), false)
}

fn bed() -> Asset {
    asset("pack-bed", "audio", "upbeat-loop.wav", 60.0, (0, 0), true)
}

fn library() -> Library {
    let item = |name: &str, kind, asset| LibraryItem {
        name: name.into(),
        kind,
        asset,
        pack: true,
        tags: Vec::new(),
    };
    Library {
        items: vec![
            item("Whoosh", MediaKind::Sound, whoosh()),
            item("Pop", MediaKind::Sound, pop()),
            item("Heart", MediaKind::Image, heart()),
            item("Upbeat loop", MediaKind::Music, bed()),
        ],
    }
}

fn word(text: &str, start: f64, end: f64) -> Word {
    Word {
        text: text.into(),
        start_us: secs(start),
        end_us: secs(end),
        confidence: 0.9,
        kind: WordKind::Word,
    }
}

/// "talk.mov" played whole.
fn project() -> Project {
    let mut p = Project::new("Trip");
    p.assets = vec![asset("talk", "video", "talk.mov", 30.0, (1920, 1080), true)];
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(30.0));
    clip.id = "t".into();
    p.timeline.main = vec![clip];
    p
}

fn speech() -> Speech {
    let mut speech = Speech::default();
    speech.transcripts.insert(
        "talk".into(),
        Arc::new(Transcript {
            key: "key".into(),
            language: "en".into(),
            model: "test".into(),
            covered_us: secs(30.0),
            complete: true,
            words: vec![
                word("Day", 0.5, 0.8),
                word("one", 0.9, 1.2),
                word("in", 1.3, 1.4),
                word("Lisbon.", 1.5, 2.0),
                word("So", 9.85, 10.15),
                word("we", 10.3, 10.5),
                word("went", 10.6, 10.9),
                word("flying", 12.0, 12.4),
                word("today.", 12.5, 13.0),
            ],
        }),
    );
    speech
}

fn scenes() -> HashMap<String, SceneLog> {
    let scene = |kind: &str, start: f64, end: f64, text: &str| SceneEntry {
        start_us: secs(start),
        end_us: secs(end),
        kind: kind.into(),
        who: String::new(),
        text: text.into(),
        on_screen_text: None,
    };
    HashMap::from([(
        "talk".to_owned(),
        SceneLog {
            items: vec![
                scene("shot", 0.0, 9.9, "a woman at a window"),
                scene("topic", 10.0, 20.0, "the flight"),
                scene("topic", 20.0, 30.0, "the hotel"),
            ],
        },
    )])
}

fn inputs(p: &Project, message: &str) -> Inputs {
    Inputs {
        message: message.into(),
        view: View::default(),
        refs: Refs::of(p),
        speech: speech(),
        scenes: scenes(),
        analysis: (p.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        cursor: None,
        fonts: Default::default(),
        library: library(),
    }
}

fn run(p: &Project, message: &str, calls: &[(&str, Value)]) -> Compiled {
    let calls: Vec<ToolCall> = calls.iter().map(|(n, a)| call(n, a.clone())).collect();
    compile(p, &inputs(p, message), &calls)
}

/// The project with the additions applied.
fn applied(p: &Project, out: &Compiled) -> Project {
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let tx = out.additions.as_ref().expect("additions");
    apply_ops(p, &tx.ops).unwrap()
}

/// Every layer on the tracks of `kind`, by start.
fn layers(p: &Project, kind: TrackKind) -> Vec<Item> {
    let mut out: Vec<Item> = (p.timeline.tracks.iter())
        .filter(|t| t.kind == kind)
        .flat_map(|t| t.items.iter().cloned())
        .collect();
    out.sort_by_key(|i| i.start_us);
    out
}

/// The one refusal, for call `index`, to the model.
fn refused_to_model(out: &Compiled, index: usize) -> &str {
    assert_eq!(out.refusals.len(), 1, "{:#?}", out.refusals);
    let r = &out.refusals[0];
    assert_eq!(
        (r.call_index, r.audience),
        (index, Audience::Model),
        "{r:?}"
    );
    assert!(!r.reason.contains(['\n', '\u{2013}', '\u{2014}']), "{r:?}");
    &r.reason
}

/// A preset's word and its `from`, if any: `("slide", Some("left"))`.
fn preset(u: &Option<MotionUse>) -> Option<(String, Option<String>)> {
    let u = u.as_ref()?;
    let word = (u.preset.as_str())
        .strip_prefix("klipolo.builtin/motion.")
        .unwrap_or(u.preset.as_str())
        .to_owned();
    let from = match u.params.get("from") {
        Some(ParamValue::Text(t)) => Some(t.clone()),
        _ => None,
    };
    Some((word, from))
}

fn named(word: &str) -> Option<(String, Option<String>)> {
    Some((word.to_owned(), None))
}

/// `p` with a title reading "Day one" from 0.5 s and the Heart sticker from 10.0 s, as the canvas and the library place
/// them.
fn with_words_and_picture() -> Project {
    let p = project();
    let mut ops = add_text_ops(&p, TextPreset::Title, "Day one", secs(0.5)).unwrap();
    ops.extend(klipolo_model::media::image::add_image_ops(&p, &heart(), secs(10.0)).unwrap());
    apply_ops(&p, &ops).unwrap()
}

fn animate(layer: &str, values: Value) -> Value {
    let mut args =
        json!({"layer": layer, "enter": null, "exit": null, "emphasis": null, "sound": null});
    for (k, v) in values.as_object().unwrap() {
        args[k] = v.clone();
    }
    args
}

#[test]
fn the_context_names_words_and_pictures_on_screen() {
    let p = with_words_and_picture();
    let ctx = build_context(
        &p,
        &Speech::default(),
        &HashMap::new(),
        &HashMap::new(),
        &View::default(),
        &Library::default(),
    );
    let at = ctx.text.find("Layers:\n").expect("a Layers list");
    let lines: Vec<&str> = ctx.text[at..].lines().skip(1).take(2).collect();
    assert_eq!(
        lines,
        [
            "t1 0.5-3.5 \"Day one\" (words, enter fade, exit fade)",
            "i1 10.0-15.0 heart.png (picture)",
        ]
    );
    let refs = Refs::of(&p);
    let text = layers(&p, TrackKind::Overlay)[0].id.clone();
    let picture = layers(&p, TrackKind::Overlay)[1].id.clone();
    assert_eq!(refs.text("t1"), Some(text.as_str()));
    assert_eq!(refs.picture("i1"), Some(picture.as_str()));
    assert_eq!((refs.text("t2"), refs.picture("i01")), (None, None));
}

#[test]
fn a_layer_ref_key_changes_when_a_word_or_picture_ref_would_name_another() {
    let p = with_words_and_picture();
    let key = klipolo_chat::layers_key(&p);
    let mut fewer = p.clone();
    let first = layers(&p, TrackKind::Overlay)[0].id.clone();
    (fewer.timeline.tracks.iter_mut()).for_each(|t| t.items.retain(|i| i.id != first));
    assert_ne!(klipolo_chat::layers_key(&fewer), key);
}

#[test]
fn animate_reads_its_words() {
    let read = call(
        "animate",
        animate(
            "last",
            json!({"enter": "fromLeft", "exit": "toBottom", "emphasis": "pulse", "sound": " Whoosh "}),
        ),
    );
    assert_eq!(
        read,
        ToolCall::Animate {
            layer: LayerRef::Last,
            enter: Some(Entrance::FromLeft),
            exit: Some(Exit::ToBottom),
            emphasis: Some(Emphasis::Pulse),
            sound: Some("Whoosh".into()),
        }
    );
    let by_ref = call("animate", animate("i2", json!({"enter": "none"})));
    assert_eq!(
        by_ref,
        ToolCall::Animate {
            layer: LayerRef::One("i2".into()),
            enter: Some(Entrance::None),
            exit: None,
            emphasis: None,
            sound: None,
        }
    );
    assert!(by_ref.names_layer() && !by_ref.names_timeline());
    assert!(!read.names_layer() && !read.names_timeline());
    // Nothing to change.
    let err = parse("animate", animate("last", json!({}))).unwrap_err();
    assert!(matches!(err.problem, Problem::NoChange { .. }), "{err:?}");
    // A word it doesn't take.
    let err = parse("animate", animate("last", json!({"enter": "spin"}))).unwrap_err();
    assert_eq!(err.field.as_deref(), Some("enter"));
}

#[test]
fn animate_the_last_picture_pops_it_in_with_a_sound_at_its_start() {
    let p = project();
    let out = run(
        &p,
        "add a heart at 10 s that pops in with a pop sound",
        &[
            (
                "addImage",
                json!({"file": "Heart", "size": null, "position": null, "atSec": 10.0, "durationSec": null}),
            ),
            (
                "animate",
                animate(
                    "last",
                    json!({"enter": "pop", "emphasis": "pulse", "sound": "Pop"}),
                ),
            ),
        ],
    );
    let after = applied(&p, &out);
    let picture = &layers(&after, TrackKind::Overlay)[0];
    let motion = picture.layout.motion.clone().expect("a motion");
    assert_eq!(preset(&motion.enter), named("pop"));
    assert_eq!(preset(&motion.looping), named("pulse"));
    assert_eq!(motion.exit, None);
    // The sound plays as it comes in, tied to the same footage.
    let sounds = layers(&after, TrackKind::Audio);
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].start_us, picture.start_us);
    assert!(
        matches!(&sounds[0].content, ItemContent::Audio { asset_id, .. } if asset_id == "pack-pop")
    );
    assert_eq!(
        sounds[0].attach.as_ref().map(|a| a.source_us),
        picture.attach.as_ref().map(|a| a.source_us)
    );
    // One undo step.
    let tx = out.additions.as_ref().unwrap();
    assert_eq!(tx.label, "chat.andMore");
}

#[test]
fn animate_by_ref_keeps_what_it_does_not_set() {
    let p = with_words_and_picture();
    let out = run(
        &p,
        "make the title slide in from the left",
        &[("animate", animate("t1", json!({"enter": "fromLeft"})))],
    );
    let after = applied(&p, &out);
    let words = &layers(&after, TrackKind::Overlay)[0];
    let motion = words.layout.motion.clone().unwrap();
    assert_eq!(
        preset(&motion.enter),
        Some(("slide".into(), Some("left".into())))
    );
    // Its exit stays the fade it had.
    assert_eq!(preset(&motion.exit), named("fade"));
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.animate");
    // None takes one away; everything none leaves no motion at all.
    let out = run(
        &p,
        "no animation on the title",
        &[(
            "animate",
            animate(
                "t1",
                json!({"enter": "none", "exit": "none", "emphasis": "none"}),
            ),
        )],
    );
    let after = applied(&p, &out);
    assert_eq!(layers(&after, TrackKind::Overlay)[0].layout.motion, None);
}

#[test]
fn every_entrance_exit_and_emphasis_is_a_built_in_motion_within_the_layer() {
    let p = with_words_and_picture();
    for (enter, want) in [
        ("pop", named("pop")),
        ("fade", named("fade")),
        ("zoom", named("zoom")),
        ("bounce", named("bounce")),
        ("blur", named("blur-in")),
        ("typewriter", named("typewriter")),
        ("fromLeft", Some(("slide".into(), Some("left".into())))),
        ("fromRight", Some(("slide".into(), Some("right".into())))),
        ("fromTop", Some(("slide".into(), Some("top".into())))),
        ("fromBottom", Some(("slide".into(), Some("bottom".into())))),
    ] {
        let out = run(
            &p,
            "animate the title",
            &[("animate", animate("t1", json!({"enter": enter})))],
        );
        let after = applied(&p, &out);
        let item = &layers(&after, TrackKind::Overlay)[0];
        let motion = item.layout.motion.clone().unwrap();
        assert_eq!(preset(&motion.enter), want, "{enter}");
        assert!(
            motion.enter.unwrap().duration_us <= item.duration_us,
            "{enter}"
        );
    }
    for (exit, want) in [
        ("pop", named("pop")),
        ("fade", named("fade")),
        ("zoom", named("zoom")),
        ("toLeft", Some(("slide".into(), Some("left".into())))),
        ("toRight", Some(("slide".into(), Some("right".into())))),
        ("toTop", Some(("slide".into(), Some("top".into())))),
        ("toBottom", Some(("slide".into(), Some("bottom".into())))),
    ] {
        let out = run(
            &p,
            "animate the heart",
            &[("animate", animate("i1", json!({"exit": exit})))],
        );
        let after = applied(&p, &out);
        let motion = layers(&after, TrackKind::Overlay)[1]
            .layout
            .motion
            .clone()
            .unwrap();
        assert_eq!(preset(&motion.exit), want, "{exit}");
    }
    for emphasis in ["pulse", "shake", "wiggle"] {
        let out = run(
            &p,
            "animate the heart",
            &[("animate", animate("i1", json!({"emphasis": emphasis})))],
        );
        let after = applied(&p, &out);
        let motion = layers(&after, TrackKind::Overlay)[1]
            .layout
            .motion
            .clone()
            .unwrap();
        assert_eq!(preset(&motion.looping), named(emphasis), "{emphasis}");
        assert!(motion.looping.unwrap().params.contains_key("strength"));
    }
}

#[test]
fn animate_refuses_what_it_cannot_name_or_do() {
    let p = with_words_and_picture();
    // Nothing added before it in the answer.
    let out = run(
        &p,
        "make it pop",
        &[("animate", animate("last", json!({"enter": "pop"})))],
    );
    assert!(
        refused_to_model(&out, 0).contains("no layer was added before it"),
        "{:?}",
        out.refusals
    );
    // No such ref.
    let out = run(
        &p,
        "make it pop",
        &[("animate", animate("i5", json!({"enter": "pop"})))],
    );
    assert_eq!(refused_to_model(&out, 0), "animate: there is no layer i5.");
    // Typewriter is for words.
    let out = run(
        &p,
        "type the heart in",
        &[("animate", animate("i1", json!({"enter": "typewriter"})))],
    );
    assert_eq!(
        refused_to_model(&out, 0),
        "animate: typewriter is for words. Use another enter for i1."
    );
    // A sound the library doesn't have.
    let out = run(
        &p,
        "pop it",
        &[("animate", animate("i1", json!({"sound": "Bell"})))],
    );
    assert!(refused_to_model(&out, 0).contains("no sound in Library is called \"Bell\""));
}

#[test]
fn an_exit_sound_plays_as_it_goes_out() {
    let p = with_words_and_picture();
    let out = run(
        &p,
        "whoosh the heart out",
        &[(
            "animate",
            animate("i1", json!({"exit": "toRight", "sound": "Whoosh"})),
        )],
    );
    let after = applied(&p, &out);
    let picture = &layers(&after, TrackKind::Overlay)[1];
    let exit = picture.layout.motion.clone().unwrap().exit.unwrap();
    let sound = &layers(&after, TrackKind::Audio)[0];
    assert_eq!(sound.start_us, picture.end_us() - exit.duration_us);
}

#[test]
fn animate_the_last_sticker_rides_with_its_job() {
    let p = project();
    let out = run(
        &p,
        "a paper plane sticker at 10 s that pops in with a whoosh",
        &[
            (
                "makeSticker",
                json!({"prompt": "paper plane", "position": null, "atSec": 10.0, "durationSec": null}),
            ),
            (
                "animate",
                animate("last", json!({"enter": "pop", "sound": "Whoosh"})),
            ),
        ],
    );
    assert_eq!(out.refusals, vec![]);
    let [Job::MakeSticker(job)] = &out.jobs[..] else {
        panic!("{out:?}");
    };
    assert!(job.motion.is_some() && job.sound.is_some());
    // Once drawn, it is placed popping in, with its sound at its start.
    let drawn = asset("drawn", "image", "paper plane.png", 5.0, (512, 512), false);
    let ops = sticker_ops(&p, job, &drawn).expect("placed");
    let after = apply_ops(&p, &ops).unwrap();
    let picture = &layers(&after, TrackKind::Overlay)[0];
    assert_eq!(
        preset(&picture.layout.motion.as_ref().unwrap().enter),
        named("pop")
    );
    let sound = &layers(&after, TrackKind::Audio)[0];
    assert_eq!(sound.start_us, picture.start_us);
    assert!(
        matches!(&sound.content, ItemContent::Audio { asset_id, .. } if asset_id == "pack-whoosh")
    );
}

#[test]
fn add_text_comes_in_with_its_animation() {
    let p = project();
    let text = |animation: Value| json!({"text": "Day one", "preset": "title", "atSec": 0.5, "durationSec": null, "animation": animation});
    let motion_of = |animation: Value| -> Option<Motion> {
        let out = run(
            &p,
            "a title Day one at the start",
            &[("addText", text(animation))],
        );
        let after = applied(&p, &out);
        layers(&after, TrackKind::Overlay)[0].layout.motion.clone()
    };
    // Null: fading in and out, as before.
    let plain = motion_of(Value::Null).unwrap();
    assert_eq!(
        (preset(&plain.enter), preset(&plain.exit)),
        (named("fade"), named("fade"))
    );
    let popped = motion_of(json!("pop")).unwrap();
    assert_eq!(
        (preset(&popped.enter), preset(&popped.exit)),
        (named("pop"), named("fade"))
    );
    let typed = motion_of(json!("typewriter")).unwrap();
    assert_eq!(preset(&typed.enter), named("typewriter"));
    let blurred = motion_of(json!("blur")).unwrap();
    assert_eq!(preset(&blurred.enter), named("blur-in"));
    // None: no animation in.
    let none = motion_of(json!("none")).unwrap();
    assert_eq!((none.enter, preset(&none.exit)), (None, named("fade")));
}

#[test]
fn a_hook_title_may_quote_words_said_in_the_video() {
    let p = project();
    let out = run(
        &p,
        "turn this into a short with a hook title",
        &[(
            "addText",
            json!({"text": "we went flying", "preset": "title", "atSec": 0.0, "durationSec": 2.0, "animation": "pop"}),
        )],
    );
    let after = applied(&p, &out);
    let ItemContent::Text { text, .. } = &layers(&after, TrackKind::Overlay)[0].content else {
        panic!("words");
    };
    assert_eq!(text, "we went flying");
    // Words neither the person wrote nor said in the video are refused back.
    let out = run(
        &p,
        "turn this into a short with a hook title",
        &[(
            "addText",
            json!({"text": "Best trip ever", "preset": "title", "atSec": 0.0, "durationSec": 2.0, "animation": null}),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("said in the video"));
}

#[test]
fn a_short_planned_in_one_answer_applies_its_look_and_proposes_its_cut() {
    // As the recipe plans it: the best part kept, vertical, word pop captions, a hook title and a popping sticker with a
    // sound inside the part kept, music under it.
    let p = project();
    let message =
        "turn this into a 15 second short with nice animations, sound effects and text animations";
    let out = run(
        &p,
        message,
        &[
            (
                "findMoments",
                json!({"query": "best part", "ranges": [{"startSec": 9.85, "endSec": 13.0, "label": "the flight"}]}),
            ),
            (
                "keepOnly",
                json!({"ranges": [{"startSec": 0.5, "endSec": 2.0}, {"startSec": 9.85, "endSec": 13.0}]}),
            ),
            (
                "setFormat",
                json!({"format": "portrait", "keepOthers": false}),
            ),
            ("setCaptions", json!({"on": true, "style": "word-pop"})),
            (
                "addText",
                json!({"text": "we went flying", "preset": "title", "atSec": 0.5, "durationSec": 1.5, "animation": "pop"}),
            ),
            (
                "addImage",
                json!({"file": "Heart", "size": "small", "position": "top-right", "atSec": 10.0, "durationSec": 2.0}),
            ),
            (
                "animate",
                animate(
                    "last",
                    json!({"enter": "bounce", "emphasis": "pulse", "sound": "Pop"}),
                ),
            ),
            (
                "addMusic",
                json!({"file": "Upbeat loop", "volume": null, "duck": null, "atSec": null}),
            ),
        ],
    );
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    // The look applies at once as one step; the cut waits for the person.
    let tx = out.additions.as_ref().expect("additions");
    assert_eq!(tx.label, "chat.andMore");
    // The step lists each change it made, in the order asked, for the reply to show.
    let changes: Vec<&str> = out.changes.iter().map(|c| c.code.as_str()).collect();
    assert_eq!(
        changes,
        [
            "chat.setFormat",
            "chat.setCaptions",
            "chat.addText",
            "chat.addImage",
            "chat.animate",
            "chat.addMusic"
        ]
    );
    assert_eq!(out.proposed_calls, vec![1]);
    let proposal = out.proposal.as_ref().expect("the cut is proposed");
    // Applied, the short is the two parts kept (each snapped to the words it stands on), its layers on them.
    let short = proposal.review.after.clone();
    let length = klipolo_model::query::video_duration(&short.timeline);
    assert!((secs(4.5)..secs(6.0)).contains(&length), "{length}");
    assert_eq!(
        short.settings.formats,
        vec![klipolo_model::project::FormatKind::Portrait]
    );
    assert!(short.captions.enabled);
    let overlays = layers(&short, TrackKind::Overlay);
    assert_eq!(overlays.len(), 2, "{overlays:#?}");
    // The hook title opens the short; the sticker plays where its moment now plays (in the second part), with its sound.
    assert!(overlays[0].start_us < secs(0.5), "{:?}", overlays[0]);
    let sticker = &overlays[1];
    let second = short.timeline.main[0].duration_us();
    assert!((second..length).contains(&sticker.start_us), "{sticker:?}");
    let sounds: Vec<Item> = layers(&short, TrackKind::Audio)
        .into_iter()
        .filter(
            |i| matches!(&i.content, ItemContent::Audio { asset_id, .. } if asset_id == "pack-pop"),
        )
        .collect();
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].start_us, sticker.start_us);
}

#[test]
fn kinetic_words_are_the_kinetic_title_template() {
    let p = project();
    let out = run(
        &p,
        "a title Day one at the start",
        &[(
            "addText",
            json!({"text": "Day one", "preset": "title", "atSec": 0.5, "durationSec": null, "animation": "kinetic"}),
        )],
    );
    let after = applied(&p, &out);
    let layer = &layers(&after, TrackKind::Overlay)[0];
    let ItemContent::Generator { source, params, .. } = &layer.content else {
        panic!("a template: {layer:?}");
    };
    assert_eq!(source.as_str(), "klipolo.builtin/scene.kinetic-title");
    assert_eq!(
        params.get("title"),
        Some(&ParamValue::Text("Day one".into()))
    );
    assert_eq!((layer.start_us, layer.duration_us), (secs(0.5), secs(3.0)));
    // It fills the frame, as every template does, and the context names it as a title.
    assert_eq!(layer.layout.rect, klipolo_model::layout::BoxN::FULL);
    let ctx = build_context(
        &after,
        &Speech::default(),
        &HashMap::new(),
        &HashMap::new(),
        &View::default(),
        &Library::default(),
    );
    assert!(
        ctx.text.contains("t1 0.5-3.5 \"Day one\" (title)"),
        "{}",
        ctx.text
    );
    // A layer already there can't become one.
    let p = with_words_and_picture();
    let out = run(
        &p,
        "make the title kinetic",
        &[("animate", animate("t1", json!({"enter": "kinetic"})))],
    );
    assert!(refused_to_model(&out, 0).starts_with("animate: kinetic is a title of its own"));
}

#[test]
fn animate_last_never_reaches_past_a_layer_that_was_not_added() {
    let p = project();
    let out = run(
        &p,
        "a heart at 10 s, then a missing star, popping in",
        &[
            (
                "addImage",
                json!({"file": "Heart", "size": null, "position": null, "atSec": 10.0, "durationSec": null}),
            ),
            (
                "addImage",
                json!({"file": "Star", "size": null, "position": null, "atSec": 20.0, "durationSec": null}),
            ),
            ("animate", animate("last", json!({"enter": "pop"}))),
        ],
    );
    assert_eq!(out.refusals.len(), 2, "{:#?}", out.refusals);
    assert_eq!(out.refusals[1].call_index, 2);
    assert_eq!(
        out.refusals[1].reason,
        "animate: addImage was not done, so there is no layer to animate."
    );
    // The heart keeps the motion it came with.
    let after = apply_ops(&p, &out.additions.unwrap().ops).unwrap();
    assert_eq!(layers(&after, TrackKind::Overlay)[0].layout.motion, None);
}

/// `p` in 9:16 only.
fn vertical(mut p: Project) -> Project {
    p.settings.formats = vec![klipolo_model::project::FormatKind::Portrait];
    p.settings.primary_format = klipolo_model::project::FormatKind::Portrait;
    p
}

/// The 9:16 box of the one picture on `p` after `out`'s additions.
fn sticker_box(p: &Project, out: &Compiled) -> klipolo_model::layout::BoxN {
    let after = applied(p, out);
    let picture = (layers(&after, TrackKind::Overlay).into_iter())
        .find(|i| matches!(i.content, ItemContent::Image { .. }))
        .expect("a picture");
    picture.layout.rect
}

fn heart_at(position: &str) -> Value {
    json!({"file": "Heart", "size": "medium", "position": position, "atSec": 10.0, "durationSec": null})
}

#[test]
fn a_shorts_sticker_without_follow_data_goes_to_the_far_top_corner_small() {
    let p = vertical(project());
    let out = run(
        &p,
        "a heart at 10 s",
        &[("addImage", heart_at("top-right"))],
    );
    let b = sticker_box(&p, &out);
    assert!((b.w - 0.28).abs() < 1e-6, "{b:?}");
    assert!(b.x + b.w > 0.95, "pushed to the frame's edge: {b:?}");
    assert!(b.y < 0.2, "at the top: {b:?}");
    let out = run(&p, "a heart at 10 s", &[("addImage", heart_at("top-left"))]);
    let b = sticker_box(&p, &out);
    assert!(b.x < 0.05 && (b.w - 0.28).abs() < 1e-6, "{b:?}");
}

#[test]
fn a_shorts_sticker_goes_to_the_corner_away_from_the_followed_speaker() {
    use klipolo_model::fill_focus::{FillFocus, FocusKey, FocusPath};
    use klipolo_model::project::FormatKind;
    let mut p = vertical(project());
    // The speaker stands at the far left of the 16:9 picture: in 9:16 the crop can't centre them, so they sit left.
    let at = |at_us, x| FocusKey {
        at_us,
        x,
        y: 0.4,
        cut: false,
    };
    let path = FocusPath {
        keys: vec![at(0, 0.05), at(secs(30.0), 0.05)],
    };
    p.timeline.main[0]
        .fill_focus
        .insert(FormatKind::Portrait, FillFocus::Speaker { path });
    let out = run(&p, "a heart at 10 s", &[("addImage", heart_at("top-left"))]);
    let b = sticker_box(&p, &out);
    // The right corner is clear of them: there at its size.
    assert!(b.x + b.w > 0.8 && (b.w - 0.35).abs() < 1e-6, "{b:?}");
    // Followed in the middle, no corner is clear: the asked one, pushed to the edge, small.
    let path = FocusPath {
        keys: vec![at(0, 0.5), at(secs(30.0), 0.5)],
    };
    p.timeline.main[0]
        .fill_focus
        .insert(FormatKind::Portrait, FillFocus::Speaker { path });
    let out = run(&p, "a heart at 10 s", &[("addImage", heart_at("top-left"))]);
    let b = sticker_box(&p, &out);
    assert!(b.x < 0.05 && (b.w - 0.28).abs() < 1e-6, "{b:?}");
}

#[test]
fn a_landscape_picture_keeps_its_corner_and_size() {
    let p = project();
    let out = run(
        &p,
        "a heart at 10 s",
        &[("addImage", heart_at("top-right"))],
    );
    let b = sticker_box(&p, &out);
    assert!((b.w - 0.35).abs() < 1e-6, "{b:?}");
}

#[test]
fn the_text_null_reads_as_null_where_a_field_may_be_null() {
    // As Muse Spark sometimes writes it.
    let read = call(
        "animate",
        json!({"layer": "last", "enter": "pop", "exit": "null", "emphasis": " NULL ", "sound": "null"}),
    );
    assert_eq!(
        read,
        ToolCall::Animate {
            layer: LayerRef::Last,
            enter: Some(Entrance::Pop),
            exit: None,
            emphasis: None,
            sound: None,
        }
    );
    let read = call(
        "addText",
        json!({"text": "Day one", "preset": "title", "atSec": "null", "durationSec": null, "animation": "null",
               "font": "null"}),
    );
    assert!(matches!(
        read,
        ToolCall::AddText {
            at_us: None,
            animation: None,
            font: None,
            ..
        }
    ));
    // Words that must be given stay as written.
    let read = call(
        "addText",
        json!({"text": "null", "preset": "title", "atSec": null, "durationSec": null, "animation": null, "font": null}),
    );
    assert!(matches!(read, ToolCall::AddText { ref text, .. } if text == "null"));
}
