//! The media tools (music, sound effects, words and pictures on screen, Muse Image stickers and title cards): their
//! calls read, and compiled against a small project into additions applied at once, or into jobs for the pictures Muse
//! Image draws first. The project is made here:
//! - `talk.mov`, 30 s, played whole, landscape. Words are said from 0.5 s to 13.0 s ("flying" from 12.0 to 12.4 s);
//!   one word ends at 10.15 s, near the topic change at 10.0 s.
//! - Its scene log: a shot from 0 to 9.9 s, topics from 10.0 and from 20.0 s, and a gesture from 14.0 to 16.0 s.
//! - The library: the person's own music `lofi-beat.mp3` (120 s) and picture `logo.png` (800 x 400), in the project;
//!   Klipolo's pack: the bed `Soft ambient`, the sound `Whoosh` and the sticker `Heart`, not in the project yet.

use std::collections::HashMap;

use klipolo_chat::tools::{PictureSize, Position, TextPreset};
use klipolo_chat::{
    AnalysisState, Audience, Compiled, FunctionCall, Inputs, Job, Library, LibraryItem,
    MakeSticker, MediaKind, Problem, Refs, SceneEntry, SceneLog, ToolCall, View, compile,
    sticker_ops, sticker_placement,
};
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::ContribId;
use klipolo_model::layout::{Anchor, Layout, resolve_rect};
use klipolo_model::media::image::{Spot, add_image_ops, add_image_ops_at, image_layout};
use klipolo_model::media::music::is_music;
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::media::text::{
    PORTRAIT_CLEAR, TextPreset as ModelPreset, TitleTemplate, preset_rect, text_item,
};
use klipolo_model::ops::Op;
use klipolo_model::params::ParamValue;
use klipolo_model::project::{Asset, FormatKind};
use klipolo_model::query::video_duration;
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Attach, Clip, ClipSource, Item, ItemContent, TrackKind};
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

/// The file every clip here plays.
fn talk_file() -> ClipSource {
    ClipSource::Asset { id: "talk".into() }
}

/// The anchor of clip "t" at `s` seconds of the file.
fn talk_at(s: f64) -> Attach {
    Attach {
        clip_id: "t".into(),
        source: talk_file(),
        source_us: secs(s),
    }
}

/// The one job of a 5 s paper plane sticker the person asked for at `at` seconds on `p`.
fn sticker_job(p: &Project, at: f64) -> MakeSticker {
    let i = inputs(p, &format!("a paper plane sticker at {at} s"));
    let out = compile(
        p,
        &i,
        &[call(
            "makeSticker",
            json!({"prompt": "paper plane", "position": null, "atSec": at, "durationSec": 5.0}),
        )],
    );
    match &out.jobs[..] {
        [Job::MakeSticker(job)] => job.clone(),
        _ => panic!("{out:?}"),
    }
}

/// `p` with `ranges` (timeline seconds) cut out, as the person might while the picture is drawn.
fn cut_out(p: &Project, ranges: &[(f64, f64)]) -> Project {
    let ranges = (ranges.iter())
        .map(|&(a, b)| klipolo_model::time::RangeUs {
            start: secs(a),
            end: secs(b),
        })
        .collect();
    apply_ops(p, &[Op::DeleteRanges { ranges }]).unwrap()
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
    // Two times on edges 0.1 s apart ("we" ends at 10.5 s, "went" starts at 10.6 s) are one moment too: one sound, at
    // the first.
    let close = compile(
        &p,
        &i,
        &[call(
            "addSound",
            json!({"name": "whoosh", "atSec": [10.5, 10.6]}),
        )],
    );
    let sounds = layers(&applied(&p, &close), TrackKind::Audio);
    assert_eq!(
        sounds.iter().map(|s| s.start_us).collect::<Vec<_>>(),
        [secs(10.5)]
    );
    assert_eq!(
        close.additions.unwrap().params.unwrap()["count"],
        ErrorParam::from(1usize)
    );
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

/// The one overlay layer of `p` once `out`'s additions are applied.
fn the_layer(p: &Project, out: &Compiled) -> Item {
    let overlays = layers(&applied(p, out), TrackKind::Overlay);
    assert_eq!(overlays.len(), 1, "{overlays:#?}");
    overlays[0].clone()
}

#[test]
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
        let title = the_layer(&p, &out);
        assert_eq!((title.start_us, title.duration_us), (0, secs(3.0)));
        assert_eq!(words_on(&title), "Day 1 in Lisbon");
        // Exactly the layer the canvas's Add title makes.
        let made = text_item(
            ModelPreset::Title,
            "Day 1 in Lisbon",
            FormatKind::Landscape,
            0,
        );
        assert_eq!(
            (&title.content, &title.layout),
            (&made.content, &made.layout)
        );
    }
}

#[test]
fn add_text_takes_the_persons_own_words_or_asks() {
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
    assert_eq!(words_on(&the_layer(&p, &out)), "day 1 in LISBON");
    // Words of the model's own are refused back, for it to ask: nothing is added.
    let i = inputs(&p, "put her name on screen");
    let out = compile(
        &p,
        &i,
        &[call(
            "addText",
            json!({"text": "Sunita Williams", "preset": "lower-third", "atSec": 0.0, "durationSec": null}),
        )],
    );
    assert_eq!(out.additions, None);
    let reason = refused_to_model(&out, 0);
    assert!(
        reason.starts_with("addText: the words must be the person's own")
            && reason.contains("askUser"),
        "{reason}"
    );
}

#[test]
fn every_text_preset_lands_where_the_canvas_puts_it_in_every_format() {
    // Each of the four presets, with each format the main one: the layer the model's `text_item` makes, in each
    // format's own place, inside the area 9:16 keeps clear.
    let formats = [
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    let presets = [
        ("title", ModelPreset::Title),
        ("lower-third", ModelPreset::LowerThird),
        ("plain", ModelPreset::Plain),
        ("caption", ModelPreset::Caption),
    ];
    for primary in formats {
        let mut p = project();
        p.settings.formats = formats.to_vec();
        p.settings.primary_format = primary;
        let i = inputs(&p, "words on screen saying a day at the studio, at 2 s");
        for (word, preset) in presets {
            let out = compile(
                &p,
                &i,
                &[call(
                    "addText",
                    json!({"text": "A day at the studio", "preset": word, "atSec": 2.0, "durationSec": null}),
                )],
            );
            let layer = the_layer(&p, &out);
            let made = text_item(preset, "a day at the studio", primary, secs(2.0));
            assert_eq!(
                (
                    &layer.content,
                    &layer.layout,
                    layer.start_us,
                    layer.duration_us
                ),
                (&made.content, &made.layout, secs(2.0), secs(3.0)),
                "{word} in {primary:?}"
            );
            assert!(layer.attach.is_some(), "{word}: moves with its clip");
            for format in formats {
                let r = resolve_rect(&layer.layout, primary, format);
                assert_eq!(
                    r,
                    preset_rect(preset, format),
                    "{word} in {primary:?}, {format:?}"
                );
                if format == FormatKind::Portrait {
                    let c = PORTRAIT_CLEAR;
                    let inside = r.x >= c.x - 1e-9
                        && r.y >= c.y - 1e-9
                        && r.x + r.w <= c.x + c.w + 1e-9
                        && r.y + r.h <= c.y + c.h + 1e-9;
                    assert!(inside, "{word} in {primary:?}: {r:?}");
                }
            }
        }
    }
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
fn nothing_is_placed_at_or_past_the_videos_end() {
    let p = project();
    let i = inputs(&p, "add these at the end");
    let at_end = [
        call(
            "addMusic",
            json!({"file": "lofi", "volume": null, "duck": null, "atSec": 30.0}),
        ),
        call("addSound", json!({"name": "whoosh", "atSec": [30.0]})),
        call(
            "addImage",
            json!({"file": "logo", "size": null, "position": null, "atSec": 30.0, "durationSec": null}),
        ),
        call(
            "makeSticker",
            json!({"prompt": "plane", "position": null, "atSec": 30.0, "durationSec": null}),
        ),
        call(
            "addText",
            json!({"text": "the end", "preset": "plain", "atSec": 30.0, "durationSec": null}),
        ),
    ];
    for c in at_end {
        let out = compile(&p, &i, std::slice::from_ref(&c));
        assert_eq!((&out.additions, out.jobs.len()), (&None, 0), "{c:?}");
        let reason = refused_to_model(&out, 0);
        let field = if matches!(c, ToolCall::AddSound { .. }) {
            "atSec[0]"
        } else {
            "atSec"
        };
        assert_eq!(
            reason,
            format!(
                "{}: {field} 30.0 is at or past the video's end, where nothing can be added.",
                c.name()
            )
        );
    }
    // The playhead at the end, no time given: the same.
    let mut end = inputs(&p, "a title card saying the end");
    end.view.playhead_us = secs(30.0);
    let out = compile(
        &p,
        &end,
        &[call(
            "makeTitleCard",
            json!({"title": "the end", "prompt": "sunset", "atSec": null, "durationSec": null}),
        )],
    );
    assert_eq!(
        refused_to_model(&out, 0),
        "makeTitleCard: the playhead (30.0 s) is at or past the video's end, where nothing can be added."
    );
    // Near the end, a layer is cut where the video ends: the video stays 30 s long.
    let near = inputs(
        &p,
        "the logo at 28 s and a title card saying the end at 28 s",
    );
    let out = compile(
        &p,
        &near,
        &[
            call(
                "addImage",
                json!({"file": "logo", "size": null, "position": null, "atSec": 28.0, "durationSec": null}),
            ),
            call(
                "makeTitleCard",
                json!({"title": "the end", "prompt": "sunset", "atSec": 28.0, "durationSec": null}),
            ),
        ],
    );
    let after = applied(&p, &out);
    let overlays = layers(&after, TrackKind::Overlay);
    assert_eq!(
        (overlays.iter())
            .map(|l| (l.start_us, l.duration_us))
            .collect::<Vec<_>>(),
        vec![(secs(28.0), secs(2.0)), (secs(28.0), secs(2.0))]
    );
    assert_eq!(video_duration(&after.timeline), secs(30.0));
    // Words 0.2 s before the end are cut there too, their fades fitted to what is left (each no longer than the layer).
    let mut last = inputs(&p, "add The End here");
    last.view.playhead_us = secs(29.8);
    let out = compile(
        &p,
        &last,
        &[call(
            "addText",
            json!({"text": "The End", "preset": "plain", "atSec": null, "durationSec": null}),
        )],
    );
    let words = the_layer(&p, &out);
    assert_eq!((words.start_us, words.duration_us), (secs(29.8), secs(0.2)));
    let motion = words.layout.motion.as_ref().expect("its fades");
    for fade in [&motion.enter, &motion.exit].into_iter().flatten() {
        assert!(fade.duration_us <= secs(0.2), "{fade:?}");
    }
}

/// The layers `ops` add, without their ids (chat's and the library's are each new).
fn layers_of(ops: &[Op]) -> Vec<Item> {
    (ops.iter())
        .filter_map(|op| match op {
            Op::AddItem { item, .. } => Some(Item {
                id: String::new(),
                ..item.clone()
            }),
            _ => None,
        })
        .collect()
}

/// The layers `out`'s additions add, without their ids.
fn added_layers(out: &Compiled) -> Vec<Item> {
    layers_of(&out.additions.as_ref().expect("additions").ops)
}

#[test]
fn chat_places_sounds_and_pictures_as_the_librarys_plus_does() {
    let p = project();
    let i = inputs(&p, "a whoosh and the logo at 12 s");
    // A sound: the same layer as the library's + makes at that moment.
    let out = compile(
        &p,
        &i,
        &[call("addSound", json!({"name": "whoosh", "atSec": [12.0]}))],
    );
    let plus = add_sound_ops(&p, &whoosh(), secs(12.0)).unwrap();
    assert_eq!(added_layers(&out), layers_of(&plus));
    // A picture with no size or place given: the same, medium and in the middle, for its 5 s.
    let out = compile(
        &p,
        &i,
        &[call(
            "addImage",
            json!({"file": "logo", "size": null, "position": null, "atSec": 12.0, "durationSec": null}),
        )],
    );
    let plus = add_image_ops(&p, &logo(), secs(12.0)).unwrap();
    assert_eq!(added_layers(&out), layers_of(&plus));
    // In a corner, each format has its own: in 9:16, inside the area the apps keep clear.
    let out = compile(
        &p,
        &i,
        &[call(
            "addImage",
            json!({"file": "logo", "size": "small", "position": "top-right", "atSec": 12.0, "durationSec": null}),
        )],
    );
    let layout = &added_layers(&out)[0].layout;
    assert_eq!(
        layout,
        &image_layout(800, 400, 0.2, Spot::TopRight, FormatKind::Landscape)
    );
    let portrait = resolve_rect(layout, FormatKind::Landscape, FormatKind::Portrait);
    let c = PORTRAIT_CLEAR;
    assert!(
        portrait.x >= c.x && portrait.y >= c.y && portrait.x + portrait.w <= c.x + c.w,
        "{portrait:?}"
    );
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
        vec![Job::MakeSticker(MakeSticker {
            prompt: "paper plane".into(),
            at_us: secs(12.0),
            duration_us: secs(5.0),
            position: Position::TopRight,
            size: PictureSize::Medium,
            anchor: Some(talk_at(12.0)),
        })]
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
        matches!(&out.jobs[..], [Job::MakeSticker(MakeSticker { prompt, .. })] if prompt == "paper plane, blue"),
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
fn a_drawn_sticker_is_placed_where_its_job_says_as_a_picture_is() {
    // A 5 s sticker at 12 s, top right; the drawing comes back as a picture not in the project yet.
    let p = project();
    let mut job = sticker_job(&p, 12.0);
    job.position = Position::TopRight;
    let drawn = asset(
        "drawn",
        "image",
        "paper plane sticker.png",
        5.0,
        (1024, 1024),
        false,
    );
    let ops = sticker_ops(&p, &job, &drawn).expect("a place");
    // The same ops the library's picture makes there, for the job's length.
    let mut plus = add_image_ops_at(&p, &drawn, secs(12.0), 0.35, Spot::TopRight).unwrap();
    for op in &mut plus {
        if let Op::AddItem { item, .. } = op {
            item.duration_us = secs(5.0);
        }
    }
    assert_eq!(layers_of(&ops), layers_of(&plus));
    assert_eq!(assets_added(&ops), ["drawn"]);
    let after = apply_ops(&p, &ops).unwrap();
    let sticker = &layers(&after, TrackKind::Overlay)[0];
    assert_eq!(
        (sticker.start_us, sticker.duration_us),
        (secs(12.0), secs(5.0))
    );
    // Its moment cut while it was drawn: nowhere to go.
    assert_eq!(
        sticker_ops(&cut_out(&p, &[(11.0, 13.0)]), &job, &drawn),
        None
    );
}

#[test]
fn a_sticker_whose_footage_is_cut_while_it_is_drawn_is_dropped() {
    let p = project();
    let job = sticker_job(&p, 12.0);
    assert_eq!(job.anchor, Some(talk_at(12.0)));
    // Nothing changed: where it was made.
    assert_eq!(sticker_placement(&p, &job), Some((secs(12.0), secs(5.0))));
    // The moment it shows over is cut: nowhere to go.
    assert_eq!(sticker_placement(&cut_out(&p, &[(11.0, 13.0)]), &job), None);
    // A cut before it moves it earlier with its footage.
    assert_eq!(
        sticker_placement(&cut_out(&p, &[(2.0, 4.0)]), &job),
        Some((secs(10.0), secs(5.0)))
    );
}

#[test]
fn a_cut_after_a_sticker_shortens_it_to_the_new_end() {
    let p = project();
    let job = sticker_job(&p, 12.0);
    // The video now ends at 15.0 s: 3 s are left of the 5.
    let shorter = cut_out(&p, &[(15.0, 30.0)]);
    assert_eq!(
        sticker_placement(&shorter, &job),
        Some((secs(12.0), secs(3.0)))
    );
}

#[test]
fn a_copy_placed_earlier_doesnt_take_the_sticker() {
    let p = project();
    let job = sticker_job(&p, 12.0);
    // A copy of the whole clip goes before it: the sticker stays on its own clip, now 30 s later.
    let mut copied = p.clone();
    let mut copy = copied.timeline.main[0].clone();
    copy.id = "copy".into();
    copied.timeline.main.insert(0, copy);
    assert_eq!(
        sticker_placement(&copied, &job),
        Some((secs(42.0), secs(5.0)))
    );
}

#[test]
fn a_sticker_past_the_main_track_has_no_anchor() {
    // A picture runs to 35 s, past the clips' end at 30 s; a sticker at 32 s has no footage under it.
    let p = project();
    let mut ops = add_image_ops(&p, &logo(), secs(25.0)).unwrap();
    for op in &mut ops {
        if let Op::AddItem { item, .. } = op {
            item.duration_us = secs(10.0);
        }
    }
    let longer = apply_ops(&p, &ops).unwrap();
    assert_eq!(video_duration(&longer.timeline), secs(35.0));
    let job = sticker_job(&longer, 32.0);
    assert_eq!((job.at_us, &job.anchor), (secs(32.0), &None));
    // Placed at its time, cut at the end: 3 s.
    assert_eq!(
        sticker_placement(&longer, &job),
        Some((secs(32.0), secs(3.0)))
    );
    // Past the end once the picture is gone: dropped.
    assert_eq!(sticker_placement(&p, &job), None);
}

#[test]
fn a_sticker_over_a_faster_clip_keeps_to_its_footage() {
    // The clip plays at twice its speed: 30 s of file in 15 s.
    let mut p = project();
    p.timeline.main[0].speed = 2.0;
    let job = sticker_job(&p, 5.0);
    // 5 s on the timeline is 10 s of the file.
    assert_eq!(job.anchor, Some(talk_at(10.0)));
    assert_eq!(sticker_placement(&p, &job), Some((secs(5.0), secs(5.0))));
    // Cutting 1 s of the timeline before it (2 s of the file) moves it 1 s earlier.
    assert_eq!(
        sticker_placement(&cut_out(&p, &[(1.0, 2.0)]), &job),
        Some((secs(4.0), secs(5.0)))
    );
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
    // The template fills the frame in every format, for its own 4 s: the model makes the primary's own place its rect.
    let mut full = Layout::full_frame();
    full.overrides.remove(&p.settings.primary_format);
    assert_eq!(title.layout, full);
    assert_eq!((title.start_us, title.duration_us), (0, secs(4.0)));
    let (source, texts) = generator(title);
    assert_eq!(source, &TitleTemplate::TitleCard.source());
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
            title_id: title.id.clone(),
        }]
    );
}

#[test]
fn the_title_card_lasts_as_long_as_its_template() {
    // The default length is the template's own `duration`, its words coming in and going out included.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../klipolo-model/presets")
        .join("scene.title-card")
        .join("scene.json");
    let scene = klipolo_model::Scene::from_stored(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(klipolo_chat::tools::media::TEMPLATE_SECS, scene.duration);
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
fn a_title_cards_background_may_be_the_models_own_choice() {
    // The person gave only the words: the background the model chose is drawn, and the title goes in at once.
    let p = project();
    let i = inputs(&p, "make a title card that says Day 1 in Lisbon");
    let out = compile(
        &p,
        &i,
        &[call(
            "makeTitleCard",
            json!({"title": "Day 1 in Lisbon", "prompt": "Lisbon street at golden hour", "atSec": null, "durationSec": null}),
        )],
    );
    assert_eq!(out.refusals, vec![]);
    let title = the_layer(&p, &out);
    assert!(matches!(
        &out.jobs[..],
        [Job::MakeTitleBackground { prompt, title_id, .. }]
            if prompt == "Lisbon street at golden hour" && *title_id == title.id
    ));
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

/// "talk.mov" as two pieces: c1 plays its 0 to 10 s, c2 its 20 to 30 s, at 10 to 20 s on the timeline.
fn two_pieces() -> Project {
    let mut p = project();
    let piece = |id: &str, from: f64, to: f64| {
        let mut c = Clip::new(
            ClipSource::Asset { id: "talk".into() },
            secs(from),
            secs(to),
        );
        c.id = id.into();
        c
    };
    p.timeline.main = vec![piece("a", 0.0, 10.0), piece("b", 20.0, 30.0)];
    p
}

/// c1's end moved out to 12 s: c2 starts 2 s later.
fn longer_c1() -> ToolCall {
    call(
        "trimClip",
        json!({"clip": "c1", "edge": "end", "toSec": 12.0}),
    )
}

/// The answer's calls in their order and the other way round.
fn both_orders(calls: [ToolCall; 2]) -> [[ToolCall; 2]; 2] {
    let [x, y] = calls;
    [[x.clone(), y.clone()], [y, x]]
}

#[test]
fn a_sticker_keeps_to_its_footage_whatever_the_answer_does_before_it() {
    let p = two_pieces();
    let i = inputs(
        &p,
        "a paper plane sticker at 15 s, and the first clip to end at 12 s",
    );
    let sticker = call(
        "makeSticker",
        json!({"prompt": "paper plane", "position": null, "atSec": 15.0, "durationSec": null}),
    );
    for calls in both_orders([sticker, longer_c1()]) {
        let out = compile(&p, &i, &calls);
        let with_additions = applied(&p, &out);
        // 15 s was c2's 25 s; c1 is 2 s longer now, so that footage plays at 17 s.
        let [Job::MakeSticker(job)] = &out.jobs[..] else {
            panic!("{:?}", out.jobs);
        };
        let anchor = Attach {
            clip_id: "b".into(),
            source: talk_file(),
            source_us: secs(25.0),
        };
        assert_eq!(
            (job.at_us, &job.anchor),
            (secs(17.0), &Some(anchor)),
            "{calls:?}"
        );
        assert_eq!(
            sticker_placement(&with_additions, job),
            Some((secs(17.0), secs(5.0)))
        );
    }
}

#[test]
fn a_title_card_moves_with_its_footage_whatever_the_answer_does_before_it() {
    let p = two_pieces();
    let i = inputs(
        &p,
        "a title card saying Day one at 15 s, and the first clip to end at 12 s",
    );
    let card = call(
        "makeTitleCard",
        json!({"title": "Day one", "prompt": "trams", "atSec": 15.0, "durationSec": null}),
    );
    for calls in both_orders([card, longer_c1()]) {
        let out = compile(&p, &i, &calls);
        let titles = layers(&applied(&p, &out), TrackKind::Overlay);
        assert_eq!(titles[0].start_us, secs(17.0), "{calls:?}");
        assert!(
            matches!(&out.jobs[..], [Job::MakeTitleBackground { title_id, .. }] if *title_id == titles[0].id)
        );
    }
}

#[test]
fn a_title_cards_background_takes_the_format_the_answer_leaves() {
    let p = project();
    let i = inputs(
        &p,
        "make it vertical, with a title card saying Day 1 and a plane sticker when I say flying",
    );
    let card = call(
        "makeTitleCard",
        json!({"title": "Day 1", "prompt": "trams", "atSec": 0.0, "durationSec": null}),
    );
    let portrait = call(
        "setFormat",
        json!({"format": "portrait", "keepOthers": true}),
    );
    let sticker = call(
        "makeSticker",
        json!({"prompt": "plane", "position": "top-right", "atSec": 12.0, "durationSec": null}),
    );
    let mut seen = Vec::new();
    for calls in both_orders([card, portrait]) {
        let calls = [calls[0].clone(), calls[1].clone(), sticker.clone()];
        let out = compile(&p, &i, &calls);
        assert!(
            matches!(
                &out.jobs[..],
                [
                    Job::MakeTitleBackground {
                        format: FormatKind::Portrait,
                        ..
                    },
                    Job::MakeSticker(_)
                ]
            ),
            "{calls:?}: {:?}",
            out.jobs
        );
        seen.push(out.jobs[1].clone());
    }
    // A new format moves no footage: the sticker is the same either way.
    assert_eq!(seen[0], seen[1]);
}

#[test]
fn a_stickers_footage_is_found_again_once_a_removal_is_applied() {
    let p = project();
    let i = inputs(
        &p,
        "a plane sticker when I say flying, and cut the first words",
    );
    let sticker = call(
        "makeSticker",
        json!({"prompt": "plane", "position": null, "atSec": 12.0, "durationSec": null}),
    );
    let cut = call("cutRange", json!({"startSec": 0.5, "endSec": 2.0}));
    for calls in both_orders([sticker, cut]) {
        let out = compile(&p, &i, &calls);
        // The removal is only proposed: the sticker is placed on the project as the additions leave it.
        let [Job::MakeSticker(job)] = &out.jobs[..] else {
            panic!("{:?}", out.jobs);
        };
        assert_eq!(job.at_us, secs(12.0), "{calls:?}");
        let proposal = out.proposal.as_ref().expect("the cut, proposed");
        let after = apply_ops(&p, &proposal.tx.ops).unwrap();
        // Applied, the cut moves "flying" earlier by what it took away; the sticker follows it there.
        assert_eq!(
            sticker_placement(&after, job),
            Some((secs(12.0) - proposal.removed_us, secs(5.0))),
            "{calls:?}"
        );
    }
}

#[test]
fn pictures_near_the_end_are_as_long_whatever_the_answer_does_after_them() {
    // The clip plays 26 s of its 30 s file; in the same answer it grows to 28 s. A picture or a title card at 24 s gets
    // the 4 s before the new end in either order, not the 2 s before the old one.
    let mut p = project();
    p.timeline.main[0].source_out_us = secs(26.0);
    let i = inputs(
        &p,
        "the logo and a title card saying the end at 24 s, and let the clip run to 28 s",
    );
    let logo = call(
        "addImage",
        json!({"file": "logo", "size": null, "position": null, "atSec": 24.0, "durationSec": null}),
    );
    let card = call(
        "makeTitleCard",
        json!({"title": "the end", "prompt": "sunset", "atSec": 24.0, "durationSec": null}),
    );
    let longer = call(
        "trimClip",
        json!({"clip": "c1", "edge": "end", "toSec": 28.0}),
    );
    let answers =
        (both_orders([logo, longer.clone()]).into_iter()).chain(both_orders([card, longer]));
    for calls in answers {
        let out = compile(&p, &i, &calls);
        let after = applied(&p, &out);
        let overlays = layers(&after, TrackKind::Overlay);
        assert_eq!(
            (overlays[0].start_us, overlays[0].duration_us),
            (secs(24.0), secs(4.0)),
            "{calls:?}"
        );
        assert_eq!(video_duration(&after.timeline), secs(28.0), "{calls:?}");
    }
}

#[test]
fn a_title_card_with_a_removal_is_the_same_in_either_order() {
    let p = project();
    let i = inputs(
        &p,
        "a title card saying flying high when I say flying, and cut the first words",
    );
    let card = call(
        "makeTitleCard",
        json!({"title": "flying high", "prompt": "clouds", "atSec": 12.0, "durationSec": null}),
    );
    let cut = call("cutRange", json!({"startSec": 0.5, "endSec": 2.0}));
    for calls in both_orders([card, cut]) {
        let out = compile(&p, &i, &calls);
        // The title is placed on the project as the additions leave it; the cut waits for the person.
        let with_additions = applied(&p, &out);
        let title = layers(&with_additions, TrackKind::Overlay)[0].clone();
        assert_eq!(title.start_us, secs(12.0), "{calls:?}");
        assert!(
            matches!(&out.jobs[..], [Job::MakeTitleBackground { title_id, .. }] if *title_id == title.id),
            "{calls:?}"
        );
        // Applied, the cut takes the title along with its footage: the job's layer is still there, earlier.
        let proposal = out.proposal.as_ref().expect("the cut, proposed");
        let after = apply_ops(&with_additions, &proposal.tx.ops).unwrap();
        let moved = layers(&after, TrackKind::Overlay);
        assert_eq!(
            (moved[0].id.as_str(), moved[0].start_us),
            (title.id.as_str(), secs(12.0) - proposal.removed_us),
            "{calls:?}"
        );
    }
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
        [Job::MakeSticker(MakeSticker {
            position: Position::Center,
            ..
        })]
    ));
}

#[test]
fn the_undo_step_is_named_for_the_answers_first_call() {
    // The music is asked for first; the clip's split goes in before it (layers go on what the edits make), yet the one
    // undo step is named for the music, then how many more.
    let p = project();
    let i = inputs(&p, "add lofi, and split the clip at 12 s");
    let out = compile(
        &p,
        &i,
        &[
            call(
                "addMusic",
                json!({"file": "lofi", "volume": null, "duck": null, "atSec": 0.0}),
            ),
            call("splitAt", json!({"atSec": 12.0})),
        ],
    );
    let tx = out.additions.as_ref().expect("additions");
    assert_eq!(tx.label, "chat.andMore");
    let params = tx.params.as_ref().unwrap();
    match &params["first"] {
        ErrorParam::Coded(first) => assert_eq!(first.code, "chat.addMusic"),
        other => panic!("{other:?}"),
    }
    assert_eq!(params["more"], ErrorParam::from(1usize));
    // A format the video already has changes nothing, and is noted so.
    let out = compile(
        &p,
        &i,
        &[call(
            "setFormat",
            json!({"format": "landscape", "keepOthers": true}),
        )],
    );
    assert_eq!((out.additions, out.unchanged), (None, vec![0]));
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
