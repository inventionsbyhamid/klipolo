//! The media tools (music, sound effects, words and pictures on screen, Muse Image stickers and title cards): their
//! calls read, and compiled against a small project into additions applied at once, or into jobs for the pictures Muse
//! Image draws first. The project is made here:
//! - `talk.mov`, 30 s, played whole, landscape. Words are said from 0.5 s to 13.0 s ("flying" from 12.0 to 12.4 s);
//!   one word ends at 10.15 s, near the topic change at 10.0 s.
//! - Its scene log: a shot from 0 to 9.9 s, topics from 10.0 and from 20.0 s, and a gesture from 14.0 to 16.0 s.
//! - The library: the person's own music `lofi-beat.mp3` (120 s) and picture `logo.png` (800 x 400), in the project;
//!   Klipolo's pack: the bed `Soft ambient`, the sound `Whoosh` and the sticker `Heart`, not in the project yet.

use std::collections::HashMap;

use klipolo_chat::compile::{TEXT_COMING_SOON, TITLE_CARD};
use klipolo_chat::tools::{PictureSize, Position, TextPreset};
use klipolo_chat::{
    AnalysisState, Audience, Compiled, FunctionCall, Inputs, Job, Library, LibraryItem, MediaKind,
    Problem, Refs, Refusal, SceneEntry, SceneLog, ToolCall, View, compile,
};
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::ContribId;
use klipolo_model::layout::{Anchor, Layout};
use klipolo_model::media::music::is_music;
use klipolo_model::ops::Op;
use klipolo_model::params::ParamValue;
use klipolo_model::project::{Asset, FormatKind};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, TrackKind};
use klipolo_model::{Project, apply_ops};
use serde_json::json;
use std::sync::Arc;

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

/// One call as the model wrote it, read by the tools' own reader.
fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall::parse(&FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
    .unwrap_or_else(|e| panic!("{name} {arguments}: {e}"))
}

/// An asset as the project stores it (read from its JSON, so a field added later keeps its default).
fn asset(
    id: &str,
    kind: &str,
    name: &str,
    length: f64,
    size: (u32, u32),
    has_audio: bool,
) -> Asset {
    serde_json::from_value(json!({
        "id": id,
        "kind": kind,
        "path": format!("/media/{name}"),
        "name": name,
        "durationUs": secs(length),
        "width": size.0,
        "height": size.1,
        "fps": if kind == "video" { 30.0 } else { 0.0 },
        "hasAudio": has_audio,
    }))
    .unwrap()
}

fn lofi() -> Asset {
    asset("lofi", "audio", "lofi-beat.mp3", 120.0, (0, 0), true)
}

fn rain() -> Asset {
    asset("rain", "audio", "rain.mp3", 90.0, (0, 0), true)
}

fn logo() -> Asset {
    asset("logo", "image", "logo.png", 5.0, (800, 400), false)
}

fn soft_ambient() -> Asset {
    asset(
        "pack-soft-ambient",
        "audio",
        "soft-ambient.wav",
        60.0,
        (0, 0),
        true,
    )
}

fn whoosh() -> Asset {
    asset("pack-whoosh", "audio", "whoosh.wav", 0.9, (0, 0), true)
}

fn heart() -> Asset {
    asset("pack-heart", "image", "heart.png", 5.0, (512, 512), false)
}

fn item(name: &str, kind: MediaKind, asset: Asset, pack: bool) -> LibraryItem {
    LibraryItem {
        name: name.into(),
        kind,
        asset,
        pack,
    }
}

/// The person's music and picture, then the pack's bed, sound and sticker.
fn library() -> Library {
    Library {
        items: vec![
            item("lofi-beat.mp3", MediaKind::Music, lofi(), false),
            item("logo.png", MediaKind::Image, logo(), false),
            item("Soft ambient", MediaKind::Music, soft_ambient(), true),
            item("Whoosh", MediaKind::Sound, whoosh(), true),
            item("Heart", MediaKind::Image, heart(), true),
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

/// "talk.mov" played whole, with the person's music and logo in the project.
fn project() -> Project {
    let mut p = Project::new("Trip");
    p.assets = vec![
        asset("talk", "video", "talk.mov", 30.0, (1920, 1080), true),
        lofi(),
        logo(),
    ];
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

fn scene(kind: &str, start: f64, end: f64, text: &str) -> SceneEntry {
    SceneEntry {
        start_us: secs(start),
        end_us: secs(end),
        kind: kind.into(),
        who: String::new(),
        text: text.into(),
        on_screen_text: None,
    }
}

fn scenes() -> HashMap<String, SceneLog> {
    HashMap::from([(
        "talk".to_owned(),
        SceneLog {
            items: vec![
                scene("shot", 0.0, 9.9, "a woman at a window"),
                scene("topic", 10.0, 20.0, "the flight"),
                scene("action", 14.0, 16.0, "waves"),
                scene("topic", 20.0, 30.0, "the hotel"),
            ],
        },
    )])
}

/// What `compile` reads besides the project: every file analyzed, the playhead at 0, no selection.
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
        library: library(),
    }
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

/// The assets the ops bring into the project.
fn assets_added(ops: &[Op]) -> Vec<String> {
    (ops.iter())
        .flat_map(|op| match op {
            Op::AddAssets { assets } => assets.iter().map(|a| a.id.clone()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// A scene preset layer's source and its text settings.
fn generator(item: &Item) -> (&ContribId, HashMap<&str, &str>) {
    let ItemContent::Generator { source, params, .. } = &item.content else {
        panic!("not a generator: {item:?}");
    };
    let texts = (params.iter())
        .filter_map(|(k, v)| match v {
            ParamValue::Text(t) => Some((k.as_str(), t.as_str())),
            _ => None,
        })
        .collect();
    (source, texts)
}

#[test]
fn add_music_by_name_uses_defaults_with_ducking() {
    let p = project();
    let i = inputs(&p, "add lofi as background music, quieter when I talk");
    let out = compile(
        &p,
        &i,
        &[call(
            "addMusic",
            json!({"file": "lofi", "volume": null, "duck": null, "atSec": 0.0}),
        )],
    );
    let after = applied(&p, &out);
    let tx = out.additions.as_ref().unwrap();
    assert_eq!(tx.label, "chat.addMusic");
    // The person's file is in the project already: nothing comes in with it.
    assert_eq!(assets_added(&tx.ops), Vec::<String>::new());
    let music = layers(&after, TrackKind::Audio);
    assert!(
        !music.is_empty() && music.iter().all(is_music),
        "{music:#?}"
    );
    for layer in &music {
        let ItemContent::Audio {
            asset_id,
            volume,
            duck,
            ..
        } = &layer.content
        else {
            unreachable!()
        };
        assert_eq!(
            (asset_id.as_str(), *volume, *duck),
            ("lofi", 0.3, Some(-12.0))
        );
        assert!(layer.attach.is_none(), "music stays free");
    }
    // From the start to the video's end, fading in at the start and out at the end.
    let (first, last) = (&music[0], &music[music.len() - 1]);
    assert_eq!((first.start_us, last.end_us()), (0, secs(30.0)));
    let fades = |l: &Item| match &l.content {
        ItemContent::Audio {
            fade_in_us,
            fade_out_us,
            ..
        } => (*fade_in_us, *fade_out_us),
        _ => unreachable!(),
    };
    assert_eq!(fades(first).0, 500_000);
    assert_eq!(fades(last).1, 2_000_000);
    // Ducking off keeps it music, at 0 dB.
    let level = compile(
        &p,
        &i,
        &[call(
            "addMusic",
            json!({"file": "lofi-beat.mp3", "volume": 0.5, "duck": false, "atSec": 0.0}),
        )],
    );
    let after = applied(&p, &level);
    for layer in layers(&after, TrackKind::Audio) {
        let ItemContent::Audio { volume, duck, .. } = layer.content else {
            unreachable!()
        };
        assert_eq!((volume, duck), (0.5, Some(0.0)));
    }
}

#[test]
fn music_starts_at_the_playhead_when_no_time_is_given() {
    let p = project();
    let mut i = inputs(&p, "add lofi here");
    i.view.playhead_us = secs(12.0);
    let out = compile(
        &p,
        &i,
        &[call(
            "addMusic",
            json!({"file": "lofi", "volume": null, "duck": null, "atSec": null}),
        )],
    );
    let music = layers(&applied(&p, &out), TrackKind::Audio);
    assert_eq!(
        (music[0].start_us, music[music.len() - 1].end_us()),
        (secs(12.0), secs(30.0))
    );
}

#[test]
fn music_ducks_even_before_klipolo_has_listened() {
    // No transcript and no loudness yet: the music still goes in with ducking on, and ducks once there is speech to
    // duck under (the engine reads it when it comes).
    let p = project();
    let mut i = inputs(&p, "add some music");
    i.speech = Speech::default();
    i.analysis = HashMap::from([("talk".to_owned(), AnalysisState::NotStarted)]);
    let out = compile(
        &p,
        &i,
        &[call(
            "addMusic",
            json!({"file": "lofi", "volume": null, "duck": null, "atSec": 0.0}),
        )],
    );
    let music = layers(&applied(&p, &out), TrackKind::Audio);
    assert!(
        music
            .iter()
            .all(|m| matches!(m.content, ItemContent::Audio { duck: Some(d), .. } if d == -12.0)),
        "{music:#?}"
    );
}

#[test]
fn the_song_is_the_persons_one_music_file_or_a_question() {
    let p = project();
    let i = inputs(&p, "add the song");
    let song = |file: &str| {
        call(
            "addMusic",
            json!({"file": file, "volume": null, "duck": null, "atSec": 0.0}),
        )
    };
    // One music file of the person's own (the pack's bed doesn't count): the song is it.
    for said in ["the song", "my music", "Song"] {
        let out = compile(&p, &i, &[song(said)]);
        let music = layers(&applied(&p, &out), TrackKind::Audio);
        assert!(
            music
                .iter()
                .all(|m| m.content.played().unwrap().0 == "lofi"),
            "{said}"
        );
    }
    // Two: the model is told to ask which.
    let mut two = p.clone();
    two.assets.push(rain());
    let mut i = inputs(&two, "add the song");
    (i.library.items).push(item("rain.mp3", MediaKind::Music, rain(), false));
    let out = compile(&two, &i, &[song("the song")]);
    assert_eq!(out.additions, None);
    let reason = refused_to_model(&out, 0);
    assert!(
        reason.contains("lofi-beat.mp3")
            && reason.contains("rain.mp3")
            && reason.contains("askUser"),
        "{reason}"
    );
    // None of their own: ask too.
    let mut none = inputs(&p, "add the song");
    none.library.items.retain(|it| it.pack);
    let out = compile(&p, &none, &[song("the song")]);
    assert!(refused_to_model(&out, 0).contains("askUser"), "{out:?}");
}

#[test]
fn a_pack_item_comes_in_with_its_file() {
    let p = project();
    let i = inputs(&p, "add soft ambient music");
    let out = compile(
        &p,
        &i,
        &[call(
            "addMusic",
            json!({"file": "soft-ambient", "volume": null, "duck": null, "atSec": 0.0}),
        )],
    );
    let tx = out.additions.as_ref().expect("additions");
    assert_eq!(assets_added(&tx.ops), ["pack-soft-ambient"]);
    let after = applied(&p, &out);
    assert!(after.asset("pack-soft-ambient").is_some());
}

#[test]
fn add_sound_at_topic_changes_snaps_to_scene_items() {
    let p = project();
    let i = inputs(&p, "a whoosh at every topic change");
    // A little off at both: "So" ends at 10.15 s, nearer 10.1 than the topic change at 10.0 s, which wins anyway.
    let out = compile(
        &p,
        &i,
        &[call(
            "addSound",
            json!({"name": "whoosh", "atSec": [10.1, 19.8]}),
        )],
    );
    let tx = out.additions.as_ref().expect("additions");
    assert_eq!(tx.label, "chat.addSound");
    // The pack's file comes in once for both.
    assert_eq!(assets_added(&tx.ops), ["pack-whoosh"]);
    let after = applied(&p, &out);
    let sounds = layers(&after, TrackKind::Audio);
    assert_eq!(
        (sounds.iter())
            .map(|s| (s.start_us, s.duration_us))
            .collect::<Vec<_>>(),
        vec![(secs(10.0), secs(0.9)), (secs(20.0), secs(0.9))]
    );
    for s in &sounds {
        let ItemContent::Audio {
            asset_id,
            volume,
            fade_in_us,
            fade_out_us,
            duck,
            ..
        } = &s.content
        else {
            unreachable!()
        };
        assert_eq!(
            (asset_id.as_str(), *volume, *fade_in_us, *fade_out_us, *duck),
            ("pack-whoosh", 1.0, 0, 0, None)
        );
        assert!(!is_music(s));
        assert!(s.attach.is_some(), "a sound moves with its moment");
    }
    // Two times on one edge are one sound.
    let twice = compile(
        &p,
        &i,
        &[call(
            "addSound",
            json!({"name": "Whoosh", "atSec": [20.0, 20.1]}),
        )],
    );
    assert_eq!(layers(&applied(&p, &twice), TrackKind::Audio).len(), 1);
}

#[test]
fn invented_time_for_a_sound_is_refused() {
    let p = project();
    let i = inputs(&p, "a whoosh at the good bits");
    // Nothing is said, shown or selected near 25.5 s.
    let out = compile(
        &p,
        &i,
        &[call(
            "addSound",
            json!({"name": "whoosh", "atSec": [10.0, 25.5]}),
        )],
    );
    assert_eq!((&out.additions, out.jobs.len()), (&None, 0));
    let reason = refused_to_model(&out, 0);
    assert!(
        reason.starts_with("addSound: atSec[1] 25.5 is not within 0.3 s of a word"),
        "{reason}"
    );
}

#[test]
fn an_unknown_name_is_refused_with_what_the_library_has() {
    let p = project();
    let i = inputs(&p, "add a boing");
    let out = compile(
        &p,
        &i,
        &[call("addSound", json!({"name": "boing", "atSec": [10.0]}))],
    );
    let reason = refused_to_model(&out, 0);
    assert!(
        reason.starts_with("addSound: no sound in Library is called \"boing\"")
            && reason.contains("Whoosh"),
        "{reason}"
    );
    // A picture isn't music.
    let out = compile(
        &p,
        &i,
        &[call(
            "addMusic",
            json!({"file": "logo", "volume": null, "duck": null, "atSec": 0.0}),
        )],
    );
    assert!(refused_to_model(&out, 0).starts_with("addMusic: no music in Library is called"));
}

/// A text layer's words.
fn words_on(item: &Item) -> &str {
    match &item.content {
        ItemContent::Text { text, .. } => text,
        other => panic!("not a text layer: {other:?}"),
    }
}

#[test]
#[ignore = "addText is a stub until its four presets are built on the model's text layers"]
fn add_text_title_at_start_is_three_seconds() {
    let p = project();
    let i = inputs(
        &p,
        "put a title 'Day 1 in Lisbon' for 3 seconds at the start",
    );
    for duration in [json!(3.0), json!(null)] {
        let out = compile(
            &p,
            &i,
            &[call(
                "addText",
                json!({"text": "Day 1 in Lisbon", "preset": "title", "atSec": 0.0, "durationSec": duration}),
            )],
        );
        assert_eq!(out.additions.as_ref().unwrap().label, "chat.addText");
        let titles = layers(&applied(&p, &out), TrackKind::Overlay);
        assert_eq!(titles.len(), 1);
        let title = &titles[0];
        assert_eq!((title.start_us, title.duration_us), (0, secs(3.0)));
        assert_eq!(words_on(title), "Day 1 in Lisbon");
    }
}

#[test]
#[ignore = "addText is a stub until its four presets are built on the model's text layers"]
fn text_keeps_the_persons_own_words_and_the_models_have_no_dashes() {
    let p = project();
    // The person's spelling wins over the model's.
    let i = inputs(&p, "lower third saying day 1 in LISBON please");
    let out = compile(
        &p,
        &i,
        &[call(
            "addText",
            json!({"text": "Day 1 in Lisbon", "preset": "lower-third", "atSec": 0.0, "durationSec": null}),
        )],
    );
    let layer = &layers(&applied(&p, &out), TrackKind::Overlay)[0];
    assert_eq!(words_on(layer), "day 1 in LISBON");
    // Words the model wrote itself (a name from the transcript) never show a dash.
    let i = inputs(&p, "put her name on screen");
    let out = compile(
        &p,
        &i,
        &[call(
            "addText",
            json!({"text": "Sunita \u{2014} commander", "preset": "lower-third", "atSec": 0.0, "durationSec": null}),
        )],
    );
    let layer = &layers(&applied(&p, &out), TrackKind::Overlay)[0];
    assert_eq!(words_on(layer), "Sunita, commander");
}

#[test]
fn add_text_is_coming_soon() {
    // Offered with its four presets, but placing them comes later: the person is told plainly, nothing is added, and
    // the model is not sent back to try the title card instead.
    let p = project();
    let i = inputs(&p, "put a title 'Day 1 in Lisbon' at the start");
    let out = compile(
        &p,
        &i,
        &[call(
            "addText",
            json!({"text": "Day 1 in Lisbon", "preset": "title", "atSec": 0.0, "durationSec": null}),
        )],
    );
    assert_eq!((&out.additions, out.jobs.len()), (&None, 0));
    assert_eq!(
        out.refusals,
        vec![Refusal {
            call_index: 0,
            reason: TEXT_COMING_SOON.to_owned(),
            audience: Audience::Person,
            clip: None,
        }]
    );
}

#[test]
fn text_presets_are_the_four_presets() {
    let tools = klipolo_chat::tools();
    let add_text = (tools.as_array().unwrap().iter())
        .find(|t| t["name"] == "addText")
        .unwrap();
    assert_eq!(
        add_text["parameters"]["properties"]["preset"]["enum"],
        json!(["title", "lower-third", "plain", "caption"])
    );
    for (word, preset) in [
        ("title", TextPreset::Title),
        ("lower-third", TextPreset::LowerThird),
        ("plain", TextPreset::Plain),
        ("caption", TextPreset::Caption),
    ] {
        assert_eq!(preset.key(), word);
        let read = call(
            "addText",
            json!({"text": "Hi", "preset": word, "atSec": null, "durationSec": null}),
        );
        assert!(matches!(read, ToolCall::AddText { preset: p, .. } if p == preset));
    }
    // The title card template is only makeTitleCard's.
    let e = ToolCall::parse(&FunctionCall {
        name: "addText".into(),
        call_id: "c".into(),
        arguments:
            json!({"text": "Hi", "preset": "title-card", "atSec": null, "durationSec": null})
                .to_string(),
    })
    .unwrap_err();
    assert_eq!(e.field.as_deref(), Some("preset"));
}

#[test]
fn add_image_by_name_places_it_by_size_and_position() {
    let p = project();
    let mut i = inputs(&p, "put the logo small in the top right");
    i.view.playhead_us = secs(2.0);
    let out = compile(
        &p,
        &i,
        &[call(
            "addImage",
            json!({"file": "logo", "size": "small", "position": "top-right", "atSec": null, "durationSec": null}),
        )],
    );
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.addImage");
    let pictures = layers(&applied(&p, &out), TrackKind::Overlay);
    let logo = &pictures[0];
    assert_eq!(
        logo.content,
        ItemContent::Image {
            asset_id: "logo".into()
        }
    );
    // 5 s from the playhead, a fifth of the width, 2:1 like the file, in the corner less the margin.
    assert_eq!((logo.start_us, logo.duration_us), (secs(2.0), secs(5.0)));
    assert_eq!(logo.layout.anchor, Anchor::TopRight);
    let r = logo.layout.rect;
    let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
    let margin = 0.04 * 1080.0;
    assert!(near(r.w, 0.2), "{r:?}");
    assert!(near(r.h * 1080.0, r.w * 1920.0 / 2.0), "{r:?}");
    assert!(near((1.0 - r.x - r.w) * 1920.0, margin), "{r:?}");
    assert!(near(r.y * 1080.0, margin), "{r:?}");
    // Left unsaid: medium (0.35 of the width) in the middle.
    let out = compile(
        &p,
        &i,
        &[call(
            "addImage",
            json!({"file": "Heart", "size": null, "position": null, "atSec": 0.0, "durationSec": 2.0}),
        )],
    );
    assert_eq!(
        assets_added(&out.additions.as_ref().unwrap().ops),
        ["pack-heart"]
    );
    let heart = &layers(&applied(&p, &out), TrackKind::Overlay)[0];
    assert_eq!(heart.layout.anchor, Anchor::Center);
    assert!(near(heart.layout.rect.w, 0.35) && near(heart.layout.rect.x, 0.325));
    assert_eq!(heart.duration_us, secs(2.0));
    // A sound is no picture.
    let out = compile(
        &p,
        &i,
        &[call(
            "addImage",
            json!({"file": "whoosh", "size": null, "position": null, "atSec": 0.0, "durationSec": null}),
        )],
    );
    assert!(refused_to_model(&out, 0).starts_with("addImage: no picture in Library is called"));
}

#[test]
fn make_sticker_becomes_a_job_not_an_op() {
    let p = project();
    let i = inputs(
        &p,
        "add a sticker of a paper plane in the top right when she mentions flying",
    );
    let out = compile(
        &p,
        &i,
        &[call(
            "makeSticker",
            json!({"prompt": "paper plane", "position": "top-right", "atSec": 12.0, "durationSec": null}),
        )],
    );
    assert_eq!(out.refusals, vec![]);
    assert_eq!((&out.additions, &out.proposal), (&None, &None));
    assert_eq!(
        out.jobs,
        vec![Job::MakeSticker {
            prompt: "paper plane".into(),
            at_us: secs(12.0),
            duration_us: secs(5.0),
            position: Position::TopRight,
            size: PictureSize::Medium,
        }]
    );
    // The prompt names the picture in the library too: no dashes.
    let out = compile(
        &p,
        &i,
        &[call(
            "makeSticker",
            json!({"prompt": "paper plane \u{2014} blue", "position": null, "atSec": 12.0, "durationSec": 2.0}),
        )],
    );
    assert!(
        matches!(&out.jobs[..], [Job::MakeSticker { prompt, .. }] if prompt == "paper plane, blue"),
        "{:?}",
        out.jobs
    );
    // An invented time is refused like any other.
    let out = compile(
        &p,
        &i,
        &[call(
            "makeSticker",
            json!({"prompt": "paper plane", "position": null, "atSec": 25.5, "durationSec": null}),
        )],
    );
    assert!(out.jobs.is_empty());
    assert!(refused_to_model(&out, 0).starts_with("makeSticker: atSec 25.5 is not within 0.3 s"));
}

#[test]
fn make_title_card_writes_the_persons_title_over_a_background_job() {
    let p = project();
    let i = inputs(
        &p,
        "make a title card that says day 1 in Lisbon, with trams at sunset behind it",
    );
    let out = compile(
        &p,
        &i,
        &[call(
            "makeTitleCard",
            json!({"title": "Day 1 in Lisbon", "prompt": "Lisbon trams at sunset", "atSec": 0.0, "durationSec": null}),
        )],
    );
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.makeTitleCard");
    let titles = layers(&applied(&p, &out), TrackKind::Overlay);
    assert_eq!(titles.len(), 1);
    let title = &titles[0];
    // The template fills the frame in every format, for its own 4 s.
    assert_eq!(title.layout, Layout::full_frame());
    assert_eq!((title.start_us, title.duration_us), (0, secs(4.0)));
    let (source, texts) = generator(title);
    assert_eq!(source, &ContribId::builtin(TITLE_CARD));
    // Exactly as the person wrote it, and only it: the template's own subtitle is left empty.
    assert_eq!(
        texts,
        HashMap::from([("title", "day 1 in Lisbon"), ("subtitle", "")])
    );
    assert_eq!(
        out.jobs,
        vec![Job::MakeTitleBackground {
            prompt: "Lisbon trams at sunset".into(),
            format: FormatKind::Landscape,
            at_us: 0,
            duration_us: secs(4.0),
            title_id: title.id.clone(),
        }]
    );
}

#[test]
fn a_title_card_keeps_a_length_the_person_gave_and_its_template_holds_its_title() {
    let p = project();
    let long = "word ".repeat(20);
    let i = inputs(&p, &format!("a title card for 3 seconds saying {long}"));
    let card = |title: &str, duration: serde_json::Value| {
        call(
            "makeTitleCard",
            json!({"title": title, "prompt": "trams", "atSec": 0.0, "durationSec": duration}),
        )
    };
    let out = compile(&p, &i, &[card("word word", json!(3.0))]);
    let title = &layers(&applied(&p, &out), TrackKind::Overlay)[0];
    assert_eq!(title.duration_us, secs(3.0));
    // Longer than the template holds: refused back.
    let out = compile(&p, &i, &[card(&long, json!(null))]);
    assert_eq!((&out.additions, out.jobs.len()), (&None, 0));
    assert!(
        refused_to_model(&out, 0)
            .starts_with("makeTitleCard: title is 99 characters; a title card holds at most 60."),
        "{:?}",
        out.refusals
    );
}

#[test]
fn a_title_card_without_the_persons_title_is_refused_to_ask() {
    let p = project();
    let i = inputs(&p, "make a title card with trams behind it");
    let out = compile(
        &p,
        &i,
        &[call(
            "makeTitleCard",
            json!({"title": "Lisbon Days", "prompt": "trams", "atSec": 0.0, "durationSec": null}),
        )],
    );
    assert_eq!((&out.additions, out.jobs.len()), (&None, 0));
    let reason = refused_to_model(&out, 0);
    assert!(
        reason.starts_with("makeTitleCard: ") && reason.contains("askUser"),
        "{reason}"
    );
}

#[test]
fn media_additions_apply_at_once_as_one_step() {
    let p = project();
    let i = inputs(
        &p,
        "add lofi, a whoosh at the topic change, a heart and a paper plane sticker",
    );
    let out = compile(
        &p,
        &i,
        &[
            call(
                "addMusic",
                json!({"file": "lofi", "volume": null, "duck": null, "atSec": 0.0}),
            ),
            call("addSound", json!({"name": "whoosh", "atSec": [10.0]})),
            call(
                "makeSticker",
                json!({"prompt": "paper plane", "position": null, "atSec": 12.0, "durationSec": 2.0}),
            ),
            call(
                "addImage",
                json!({"file": "heart", "size": null, "position": "bottom-left", "atSec": 12.0, "durationSec": null}),
            ),
        ],
    );
    let tx = out.additions.as_ref().expect("additions");
    assert_eq!(tx.label, "chat.andMore");
    let params = tx.params.as_ref().unwrap();
    match &params["first"] {
        ErrorParam::Coded(first) => assert_eq!(first.code, "chat.addMusic"),
        other => panic!("{other:?}"),
    }
    assert_eq!(params["more"], ErrorParam::from(2usize));
    assert_eq!((out.proposal, out.proposed_calls), (None, vec![]));
    assert!(matches!(
        out.jobs.as_slice(),
        [Job::MakeSticker {
            position: Position::Center,
            ..
        }]
    ));
}

#[test]
fn media_tools_read_their_arguments() {
    use ToolCall::*;
    assert_eq!(
        call(
            "addMusic",
            json!({"file": " lofi ", "volume": 0.2, "duck": true, "atSec": 1.5})
        ),
        AddMusic {
            file: "lofi".into(),
            volume: Some(0.2),
            duck: Some(true),
            at_us: Some(secs(1.5)),
        }
    );
    assert_eq!(
        call("addSound", json!({"name": "whoosh", "atSec": [12.8, 23.0]})),
        AddSound {
            name: "whoosh".into(),
            at_us: vec![secs(12.8), secs(23.0)],
        }
    );
    assert_eq!(
        call(
            "addImage",
            json!({"file": "logo", "size": "large", "position": "bottom-right", "atSec": null, "durationSec": 4.0})
        ),
        AddImage {
            file: "logo".into(),
            size: Some(PictureSize::Large),
            position: Some(Position::BottomRight),
            at_us: None,
            duration_us: Some(secs(4.0)),
        }
    );
    assert_eq!(
        call(
            "makeTitleCard",
            json!({"title": "Day 1", "prompt": "trams", "atSec": null, "durationSec": null})
        ),
        MakeTitleCard {
            title: "Day 1".into(),
            prompt: "trams".into(),
            at_us: None,
            duration_us: None,
        }
    );
    let error = |name: &str, args: serde_json::Value| {
        ToolCall::parse(&FunctionCall {
            name: name.into(),
            call_id: "c".into(),
            arguments: args.to_string(),
        })
        .unwrap_err()
    };
    // A place the tool doesn't offer, no time for a sound, and a volume of 30 meaning 30 %.
    let e = error(
        "makeSticker",
        json!({"prompt": "plane", "position": "upper right", "atSec": null, "durationSec": null}),
    );
    assert_eq!(
        e.to_string(),
        "makeSticker: position must be one of top-left, top-right, bottom-left, bottom-right, center or null, \
         not \"upper right\"."
    );
    let e = error("addSound", json!({"name": "whoosh", "atSec": []}));
    assert_eq!(
        (e.field.as_deref(), &e.problem),
        (Some("atSec"), &Problem::Empty)
    );
    let e = error("addSound", json!({"name": "whoosh", "atSec": [3.0, "x"]}));
    assert_eq!(e.field.as_deref(), Some("atSec[1]"));
    let e = error(
        "addMusic",
        json!({"file": "lofi", "volume": 30, "duck": null, "atSec": null}),
    );
    assert_eq!(
        e.to_string(),
        "addMusic: volume must be between 0 and 1, not 30."
    );
    let e = error(
        "addText",
        json!({"text": "Hi", "preset": "title", "atSec": 0, "durationSec": 0}),
    );
    assert_eq!(
        e.to_string(),
        "addText: durationSec must be between 0.5 and 600 seconds, not 0."
    );
}
