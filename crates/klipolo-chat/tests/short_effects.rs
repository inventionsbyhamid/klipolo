//! A short's stickers and sound effects, placed and timed by Klipolo on what the short shows (`compile::cues`): the
//! model picks the word and the sticker, the payoff line and the kinds of sound, never the place or time.
//!
//! The project is made here: `panel.mov`, 120 s, 1920 x 1080, played whole. A word is said every 0.5 s ("w0" at 0.5 s
//! to 0.8 s, "w1" at 1.0 s and so on). The scene log says who speaks: the man on the left to 30 s, the woman on the
//! right to 90 s, the man again to the end. The short keeps four parts of about 14 s (0.5 to 14.8, 32.0 to 46.8, 62.0
//! to 76.8, 92.0 to 106.8 s): its cuts go to the woman, to her again, and back to the man.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use klipolo_chat::{
    AnalysisState, Compiled, FunctionCall, Inputs, Job, Library, LibraryItem, MediaKind, Refs,
    SceneEntry, SceneLog, ToolCall, View, compile, sticker_ops,
};
use klipolo_model::Project;
use klipolo_model::fill_focus::{FillFocus, FocusKey, FocusPath, Keep};
use klipolo_model::layout::BoxN;
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, FormatKind};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall::parse(&FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
    .unwrap_or_else(|e| panic!("{name} {arguments}: {e}"))
}

fn asset(id: &str, kind: &str, name: &str, length: f64, size: (u32, u32)) -> Asset {
    serde_json::from_value(json!({
        "id": id,
        "kind": kind,
        "path": format!("/media/{name}"),
        "name": name,
        "durationUs": secs(length),
        "width": size.0,
        "height": size.1,
        "fps": if kind == "video" { 30.0 } else { 0.0 },
        "hasAudio": kind != "image",
    }))
    .unwrap()
}

fn library() -> Library {
    let sound = |name: &str, tags: &[&str], length: f64| LibraryItem {
        name: name.into(),
        kind: MediaKind::Sound,
        asset: asset(
            &format!("pack-{name}"),
            "audio",
            &format!("{name}.wav"),
            length,
            (0, 0),
        ),
        pack: true,
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
    };
    let sticker = |name: &str| LibraryItem {
        name: name.into(),
        kind: MediaKind::Image,
        asset: asset(
            &format!("pack-{name}"),
            "image",
            &format!("{name}.png"),
            5.0,
            (1024, 1024),
        ),
        pack: true,
        tags: vec![name.to_lowercase()],
    };
    Library {
        items: vec![
            sound("Whoosh", &["whoosh", "transition"], 0.43),
            sound("Pop", &["pop", "appear"], 0.25),
            sound("Rise", &["riser", "build up"], 2.76),
            sound("Chime", &["chime", "magic"], 2.7),
            sound("Impact", &["impact", "hit"], 2.8),
            sticker("Heart"),
            sticker("Rocket"),
            sticker("Star"),
            sticker("Fire"),
            sticker("Check"),
            sticker("Sparkles"),
        ],
    }
}

/// The word said at `0.5 + 0.5 n` s.
fn w(n: usize) -> String {
    format!("w{n}")
}

fn speech() -> Speech {
    let words = (0..239)
        .map(|n| {
            let start = 0.5 + 0.5 * n as f64;
            Word {
                text: w(n),
                start_us: secs(start),
                end_us: secs(start + 0.3),
                confidence: 0.9,
                kind: WordKind::Word,
            }
        })
        .collect();
    let mut speech = Speech::default();
    speech.transcripts.insert(
        "panel".into(),
        Arc::new(Transcript {
            key: "key".into(),
            language: "en".into(),
            model: "test".into(),
            covered_us: secs(120.0),
            complete: true,
            words,
        }),
    );
    speech
}

fn scenes() -> HashMap<String, SceneLog> {
    let item = |kind: &str, start: f64, end: f64, who: &str| SceneEntry {
        start_us: secs(start),
        end_us: secs(end),
        kind: kind.into(),
        who: who.into(),
        text: "talks".into(),
        on_screen_text: None,
    };
    HashMap::from([(
        "panel".to_owned(),
        SceneLog {
            items: vec![
                item("shot", 0.0, 120.0, ""),
                item("speech", 0.0, 30.0, "man on the left"),
                item("speech", 30.0, 60.0, "woman on the right"),
                item("speech", 60.0, 90.0, "woman on the right"),
                item("speech", 90.0, 120.0, "man on the left"),
            ],
        },
    )])
}

/// "panel.mov" played whole, landscape only.
fn project() -> Project {
    let mut p = Project::new("Panel");
    p.assets = vec![asset("panel", "video", "panel.mov", 120.0, (1920, 1080))];
    let mut clip = Clip::new(ClipSource::Asset { id: "panel".into() }, 0, secs(120.0));
    clip.id = "c".into();
    p.timeline.main = vec![clip];
    p
}

fn inputs(p: &Project, message: &str) -> Inputs {
    Inputs {
        turns: Default::default(),
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

const MESSAGE: &str = "Make this a 60 second short with stickers and sound effects";

/// The short's cut, format, follow, captions and hook, as the recipe makes them.
fn base() -> Vec<ToolCall> {
    vec![
        call(
            "keepOnly",
            json!({"ranges": [
                {"startSec": 0.5, "endSec": 14.8},
                {"startSec": 32.0, "endSec": 46.8},
                {"startSec": 62.0, "endSec": 76.8},
                {"startSec": 92.0, "endSec": 106.8},
            ]}),
        ),
        call(
            "setFormat",
            json!({"format": "portrait", "keepOthers": true}),
        ),
        call(
            "followSubject",
            json!({"description": "speaker", "focus": null}),
        ),
        call(
            "setCaptions",
            json!({"on": true, "style": "word-pop", "set": null}),
        ),
        call(
            "addText",
            json!({"text": "w1 w2 w3", "preset": "title", "atSec": null, "atWords": "w1 w2 w3",
                   "durationSec": 3.0, "animation": "kinetic", "font": null}),
        ),
    ]
}

fn sticker(file: &str, word: usize) -> [ToolCall; 2] {
    [
        call(
            "addImage",
            json!({"file": file, "size": "medium", "position": "top-right", "atSec": null,
                   "atWords": w(word), "durationSec": 2.0}),
        ),
        call(
            "animate",
            json!({"layer": "last", "enter": "pop", "exit": null, "emphasis": null, "sound": "Pop"}),
        ),
    ]
}

fn sound(name: &str, word: usize) -> ToolCall {
    call(
        "addSound",
        json!({"name": name, "atSec": null, "atWords": [w(word)]}),
    )
}

/// The full plan: six sticker moments and five sounds, as a model might send them.
fn plan() -> Vec<ToolCall> {
    let mut calls = base();
    // w4 (2.5 s) is under the hook; w10 (5.5 s) is kept; w20 (10.5 s) is 5 s after it; w27 (14.0 s) is too near its
    // part's end; w70 (35.5 s) is kept; w210 (105.5 s) is in the short's last seconds.
    for (file, word) in [
        ("Heart", 4),
        ("Rocket", 10),
        ("Star", 20),
        ("Check", 27),
        ("Fire", 70),
        ("Sparkles", 210),
    ] {
        calls.extend(sticker(file, word));
    }
    calls.push(sound("Rise", 1));
    calls.push(sound("Whoosh", 40));
    calls.push(sound("Impact", 150));
    calls.push(sound("Chime", 130));
    calls.push(sound("Pop", 10));
    calls
}

/// The short as Apply leaves it: the additions, then the proposed cut.
fn short(out: &Compiled) -> Project {
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    out.proposal.as_ref().expect("the cut").review.after.clone()
}

fn named(p: &Project, item: &Item) -> String {
    let id = match &item.content {
        ItemContent::Image { asset_id, .. } | ItemContent::Audio { asset_id, .. } => asset_id,
        _ => return String::new(),
    };
    (p.assets.iter())
        .find(|a| &a.id == id)
        .map(|a| {
            a.name
                .trim_end_matches(".png")
                .trim_end_matches(".wav")
                .to_owned()
        })
        .unwrap_or_default()
}

/// A picture on the timeline: its name, start and length in seconds, and the layer.
type Shown = (String, f64, f64, Item);

/// Every picture and every sound effect (no music, `(name, start s)`), by start.
fn layers(p: &Project) -> (Vec<Shown>, Vec<(String, f64)>) {
    let mut pictures = Vec::new();
    let mut sounds = Vec::new();
    for item in p.timeline.tracks.iter().flat_map(|t| &t.items) {
        let name = named(p, item);
        let (at, len) = (item.start_us as f64 / 1e6, item.duration_us as f64 / 1e6);
        match &item.content {
            ItemContent::Image { .. } => pictures.push((name, at, len, item.clone())),
            ItemContent::Audio { .. } => sounds.push((name, at)),
            _ => {}
        }
    }
    pictures.sort_by(|a, b| a.1.total_cmp(&b.1));
    sounds.sort_by(|a, b| a.1.total_cmp(&b.1));
    (pictures, sounds)
}

/// The picture's box in 9:16.
fn portrait_box(p: &Project, item: &Item) -> BoxN {
    if p.settings.primary_format == FormatKind::Portrait {
        item.layout.rect
    } else {
        item.layout.overrides[&FormatKind::Portrait]
    }
}

fn overlaps(a: BoxN, b: BoxN) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.04
}

/// Where the short plays `source` seconds of the talk.
fn in_short(p: &Project, source: f64) -> f64 {
    let t = secs(source);
    let mut start = 0;
    for clip in &p.timeline.main {
        if clip.source_in_us <= t && t < clip.source_out_us {
            return (start + t - clip.source_in_us) as f64 / 1e6;
        }
        start += clip.duration_us();
    }
    panic!("{source} s is cut");
}

/// Where each of the short's parts starts in it.
fn cuts(p: &Project) -> Vec<f64> {
    let mut start = 0;
    let mut out = Vec::new();
    for clip in &p.timeline.main {
        out.push(start as f64 / 1e6);
        start += clip.duration_us();
    }
    out
}

/// Where the follow keeps the speaker in 9:16 (the middle), their head `HEAD_HALF_H` of the height either side.
const HEAD: BoxN = BoxN {
    x: 0.28,
    y: 0.08,
    w: 0.44,
    h: 0.38,
};
/// Where the hook and the captions sit in 9:16: from the hook's band down past word pop's captions.
const WORDS: BoxN = BoxN {
    x: 0.0,
    y: 0.465,
    w: 1.0,
    h: 0.4,
};

#[test]
fn stickers_sit_beside_the_speakers_head_at_a_size_that_reads() {
    let p = project();
    let out = compile(&p, &inputs(&p, MESSAGE), &plan());
    let after = short(&out);
    let (pictures, _) = layers(&after);
    assert_eq!(pictures.len(), 2, "{pictures:#?}");
    let mut sides = Vec::new();
    for (name, _, _, item) in &pictures {
        let b = portrait_box(&after, item);
        // 18 to 24 % of the frame's width, beside the head at its height, never on it.
        assert!((0.18..=0.24).contains(&b.w), "{name}: {b:?}");
        assert!(!overlaps(b, HEAD), "{name} on the head: {b:?}");
        assert!(
            b.x + b.w <= HEAD.x + 0.001 || b.x >= HEAD.x + HEAD.w - 0.001,
            "{b:?}"
        );
        let middle = b.y + b.h / 2.0;
        assert!(
            (0.15..=0.3).contains(&middle),
            "{name} not at the head's height: {b:?}"
        );
        // As near the head as the frame's edge allows.
        let from_head = if b.x < 0.5 {
            HEAD.x - (b.x + b.w)
        } else {
            b.x - (HEAD.x + HEAD.w)
        };
        assert!(from_head < 0.05, "{name} far from the head: {b:?}");
        sides.push(b.x < 0.5);
    }
    // Both sides have the same room around a speaker the follow keeps in the middle: they take turns.
    assert_eq!(sides, vec![false, true]);
}

#[test]
fn a_sticker_takes_the_side_with_more_room_where_the_follow_put_the_speaker() {
    // The clip already follows a speaker near the picture's left edge: in 9:16 they sit left of the middle.
    let mut p = project();
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    let path = FocusPath {
        keys: vec![FocusKey {
            at_us: 0,
            x: 0.1,
            y: 0.5,
            cut: false,
        }],
    };
    p.timeline.main[0].fill_focus.insert(
        FormatKind::Portrait,
        FillFocus::Speaker {
            path,
            keep: Keep::Speaker,
            people: Vec::new(),
        },
    );
    let mut calls = base();
    calls.retain(|c| {
        !matches!(
            c,
            ToolCall::SetFormat { .. } | ToolCall::FollowSubject { .. }
        )
    });
    calls.extend(sticker("Rocket", 10));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    let after = short(&out);
    let (pictures, _) = layers(&after);
    let [(_, _, _, item)] = &pictures[..] else {
        panic!("{pictures:#?}");
    };
    let b = portrait_box(&after, item);
    // The speaker is at 0.1 / 0.316 of the frame (the crop stops at the picture's edge): their head ends at 0.54.
    let head_right = 0.1 / (9.0 / 16.0 / (16.0 / 9.0)) + 0.22;
    assert!(b.x >= head_right, "on the head: {b:?}");
    assert!(b.x - head_right < 0.05, "far from the head: {b:?}");
    assert!((0.18..=0.24).contains(&b.w), "{b:?}");
}

#[test]
fn stickers_never_cover_the_face_zone_or_the_words_where_no_one_is_followed() {
    // No follow: the face may be anywhere across the middle (`FACE_ZONE`).
    let p = project();
    let mut calls = base();
    calls.retain(|c| !matches!(c, ToolCall::FollowSubject { .. }));
    calls.extend(sticker("Rocket", 10));
    calls.extend(sticker("Fire", 70));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    let after = short(&out);
    let (pictures, _) = layers(&after);
    assert_eq!(pictures.len(), 2, "{pictures:#?}");
    for (name, _, _, item) in &pictures {
        let b = portrait_box(&after, item);
        assert!(
            !overlaps(b, klipolo_chat::compile::FACE_ZONE),
            "{name} on the face zone: {b:?}"
        );
        assert!(!overlaps(b, WORDS), "{name} on the hook or captions: {b:?}");
        assert!(b.w >= 0.17, "{name} too small: {b:?}");
        assert!(b.x >= 0.0 && b.x + b.w <= 1.0, "{b:?}");
    }
}

#[test]
fn stickers_show_from_their_word_spaced_out_and_never_under_the_hook_or_at_the_end() {
    let p = project();
    let out = compile(&p, &inputs(&p, MESSAGE), &plan());
    let after = short(&out);
    let (pictures, _) = layers(&after);
    let shown: Vec<(&str, f64, f64)> = (pictures.iter())
        .map(|(name, at, len, _)| (name.as_str(), *at, *len))
        .collect();
    // Rocket from 0.1 s before "w10" (5.5 s of the talk), Fire before "w70" (35.5 s), in the short's own time. Heart
    // (under the hook, which shows 3 s from "w1"), Star (5 s after Rocket), Check (its word 0.8 s before its part
    // ends) and Sparkles (in the last 1.5 s) are left out.
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_eq!(shown[0].0, "Rocket");
    assert!(near(shown[0].1, in_short(&after, 5.4)), "{shown:?}");
    assert_eq!(shown[1].0, "Fire");
    assert!(near(shown[1].1, in_short(&after, 35.4)), "{shown:?}");
    for (name, _, len) in &shown {
        assert!((1.2..=1.8).contains(len), "{name} shows {len} s");
    }
    assert!(shown[1].1 - shown[0].1 >= 10.0);
    // In with a pop, out with a fade.
    for (name, _, _, item) in &pictures {
        let m = item.layout.motion.as_ref().expect("motion");
        let enter = m.enter.as_ref().map(|u| u.preset.as_str());
        let exit = m.exit.as_ref().map(|u| u.preset.as_str());
        assert!(
            enter.is_some_and(|e| e.ends_with("motion.pop")),
            "{name}: {enter:?}"
        );
        assert!(
            exit.is_some_and(|e| e.ends_with("motion.fade")),
            "{name}: {exit:?}"
        );
    }
    // What the model is told says what Klipolo did, so the reply claims nothing that isn't there.
    let told: HashMap<usize, &str> = (out.told.iter()).map(|(i, s)| (*i, s.as_str())).collect();
    assert!(told[&5].starts_with("Left out"), "{told:?}");
    assert!(told[&7].starts_with("done"), "{told:?}");
    assert!(told[&8].starts_with("done"), "{told:?}");
    assert!(told[&9].starts_with("Left out"), "{told:?}");
    // The animate of a sticker left out goes with it.
    assert!(told[&6].starts_with("Left out"), "{told:?}");
    assert!(told.values().all(|s| !s.contains(['\u{2013}', '\u{2014}'])));
}

#[test]
fn sounds_follow_what_is_seen() {
    let p = project();
    let out = compile(&p, &inputs(&p, MESSAGE), &plan());
    let after = short(&out);
    let (pictures, sounds) = layers(&after);
    let at = |name: &str| -> Vec<f64> {
        (sounds.iter())
            .filter(|(n, _)| n == name)
            .map(|(_, t)| *t)
            .collect()
    };
    // A rise under the hook, as it comes in ("w1", 1.0 s of the talk).
    assert_eq!(at("Rise").len(), 1, "{sounds:?}");
    assert!(near(at("Rise")[0], in_short(&after, 1.0)), "{sounds:?}");
    // A pop with each sticker, as it pops in; the model's own Pop on a word adds none.
    let pops = at("Pop");
    assert_eq!(pops.len(), pictures.len(), "{sounds:?}");
    for ((_, start, _, _), pop) in pictures.iter().zip(&pops) {
        assert!(near(*start, *pop), "{sounds:?}");
    }
    // One chime on the payoff line: "w130", said at 65.5 s of the talk.
    assert_eq!(at("Chime").len(), 1, "{sounds:?}");
    assert!(near(at("Chime")[0], in_short(&after, 65.5)), "{sounds:?}");
    // Nothing else: no impact the person didn't ask for.
    assert!(at("Impact").is_empty(), "{sounds:?}");
    assert_eq!(
        sounds.len(),
        1 + pops.len() + 1 + at("Whoosh").len(),
        "{sounds:?}"
    );
}

#[test]
fn a_whoosh_plays_only_at_a_cut_where_the_speaker_changes() {
    let p = project();
    let out = compile(&p, &inputs(&p, MESSAGE), &plan());
    let after = short(&out);
    let (_, sounds) = layers(&after);
    let whooshes: Vec<f64> = (sounds.iter())
        .filter(|(n, _)| n == "Whoosh")
        .map(|(_, t)| *t)
        .collect();
    // The short cuts to the woman, to her again, and back to the man: a whoosh as the first and the last cut come, none
    // where she goes on, none on the word the model gave ("w40").
    let cuts = cuts(&after);
    assert_eq!(cuts.len(), 4, "{cuts:?}");
    assert_eq!(whooshes.len(), 2, "{sounds:?}");
    assert!(near(whooshes[0], cuts[1]), "{sounds:?} {cuts:?}");
    assert!(near(whooshes[1], cuts[3]), "{sounds:?} {cuts:?}");
}

#[test]
fn a_sound_the_person_names_goes_where_the_model_put_it() {
    let p = project();
    let mut calls = base();
    calls.push(sound("Impact", 70));
    let out = compile(
        &p,
        &inputs(&p, "Make a short with an impact sound on the big line"),
        &calls,
    );
    let after = short(&out);
    let (_, sounds) = layers(&after);
    // On "w70", said at 35.5 s of the talk.
    let impacts: Vec<f64> = (sounds.iter())
        .filter(|(n, _)| n == "Impact")
        .map(|(_, t)| *t)
        .collect();
    assert_eq!(impacts.len(), 1, "{sounds:?}");
    assert!(near(impacts[0], in_short(&after, 35.5)), "{sounds:?}");
}

#[test]
fn klipolo_plays_the_rise_and_the_whooshes_where_the_model_marks_only_the_payoff() {
    let p = project();
    let mut calls = base();
    calls.push(sound("Chime", 130));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    let after = short(&out);
    let (_, sounds) = layers(&after);
    let names: Vec<&str> = sounds.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["Rise", "Whoosh", "Chime", "Whoosh"],
        "{sounds:?}"
    );
    let cuts = cuts(&after);
    let want = [
        in_short(&after, 1.0),
        cuts[1],
        in_short(&after, 65.5),
        cuts[3],
    ];
    for ((_, got), want) in sounds.iter().zip(want) {
        assert!(near(*got, want), "{sounds:?} {want}");
    }
}

#[test]
fn a_drawn_sticker_in_a_short_is_timed_by_its_word_and_placed_beside_the_speaker_when_it_comes() {
    let p = project();
    let mut calls = base();
    calls.push(call(
        "makeSticker",
        json!({"prompt": "a rocket", "position": "top-left", "atSec": null, "atWords": w(10), "durationSec": 3.0}),
    ));
    calls.push(sound("Chime", 130));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let job = (out.jobs.iter())
        .find_map(|j| match j {
            Job::MakeSticker(job) => Some(job.clone()),
            _ => None,
        })
        .expect("the sticker's job");
    // From 0.1 s before "w10" (5.5 s) for 1.5 s, popping in and fading out with the pop.
    assert_eq!((job.at_us, job.duration_us), (secs(5.4), secs(1.5)));
    let m = job.motion.as_ref().expect("motion");
    assert!(
        m.enter
            .as_ref()
            .is_some_and(|u| u.preset.as_str().ends_with("motion.pop"))
    );
    assert!(
        m.exit
            .as_ref()
            .is_some_and(|u| u.preset.as_str().ends_with("motion.fade"))
    );
    assert_eq!(job.sound.as_ref().map(|a| a.name.as_str()), Some("Pop.wav"));
    assert!(job.beside.is_some());
    // Once drawn: beside the speaker the follow keeps in the middle, not in the corner the model named.
    let with =
        klipolo_model::apply_ops(&p, &out.additions.as_ref().expect("additions").ops).unwrap();
    let drawn = asset("drawn-rocket", "image", "rocket.png", 5.0, (1024, 1024));
    let ops = sticker_ops(&with, &job, &drawn).expect("placed");
    let item = (ops.iter())
        .find_map(|op| match op {
            Op::AddItem { item, .. } if matches!(item.content, ItemContent::Image { .. }) => {
                Some(item.clone())
            }
            _ => None,
        })
        .expect("the picture");
    let b = portrait_box(&with, &item);
    assert!((0.18..=0.24).contains(&b.w), "{b:?}");
    assert!(!overlaps(b, HEAD), "{b:?}");
    assert!(!overlaps(b, WORDS), "{b:?}");
}

#[test]
fn a_layer_tool_on_a_sticker_left_out_goes_with_it() {
    // The model removes or moves a sticker Klipolo left out (here, one under the hook): nothing is refused, and it is
    // told the sticker and the call went together.
    let p = project();
    let mut calls = base();
    calls.push(sticker("Heart", 4)[0].clone());
    calls.push(call(
        "setLayer",
        json!({"layer": "last", "set": [{"key": "remove", "value": true}]}),
    ));
    calls.push(call(
        "animate",
        json!({"layer": "i1", "enter": "pop", "exit": null, "emphasis": null, "sound": null}),
    ));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let told: HashMap<usize, &str> = (out.told.iter()).map(|(i, s)| (*i, s.as_str())).collect();
    assert!(told[&5].starts_with("Left out"), "{told:?}");
    assert!(
        told[&6].starts_with("Left out with its sticker"),
        "{told:?}"
    );
    assert!(
        told[&7].starts_with("Left out with its sticker"),
        "{told:?}"
    );
}

#[test]
fn a_short_hook_holds_at_most_2_5_s_and_no_caption_word_is_hidden() {
    // The hook ("w1 w2 w3", asked for 3 s) shows 2.5 s at most; the 9:16 captions wait while it shows
    // (`captions::hook_holds_captions`, drawn from the layer), so no word is hidden in the project, its caption files or
    // its other versions.
    let p = project();
    let out = compile(&p, &inputs(&p, MESSAGE), &plan());
    let ops = &out.additions.as_ref().expect("additions").ops;
    assert!(
        !ops.iter().any(|op| matches!(op, Op::SetWordLook { .. })),
        "{ops:#?}"
    );
    let after = short(&out);
    let hook = (after.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| klipolo_model::captions::in_hook_band(&after, i))
        .expect("the hook in its band");
    assert_eq!(hook.duration_us, secs(2.5));
    // The captions wait in 9:16 while it shows, and come back as it ends.
    assert!(klipolo_model::captions::hook_holds_captions(
        &after,
        hook.start_us + secs(2.4)
    ));
    assert!(!klipolo_model::captions::hook_holds_captions(
        &after,
        hook.end_us()
    ));
}

#[test]
fn a_short_plays_its_sound_effects_without_a_sound_call() {
    // The model sends no sound call at all: the rise, each sticker's pop and the whooshes still play, and the hook's
    // call says so.
    let p = project();
    let mut calls = base();
    calls.extend(sticker("Rocket", 10));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    let after = short(&out);
    let (_, sounds) = layers(&after);
    let names: Vec<&str> = sounds.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["Rise", "Pop", "Whoosh", "Whoosh"], "{sounds:?}");
    let told: HashMap<usize, &str> = (out.told.iter()).map(|(i, s)| (*i, s.as_str())).collect();
    let hook = told[&4];
    assert!(
        hook.contains("a rise under it") && hook.contains("whooshes") && hook.contains("pop"),
        "{hook}"
    );
    // Each is its own change on the card.
    let changes: Vec<String> = (out.changes.iter())
        .filter(|c| c.code == "chat.addSound")
        .map(|c| format!("{:?}", c.params.get("name")))
        .collect();
    assert_eq!(changes.len(), 2, "{changes:?}");
}

#[test]
fn a_short_the_person_wants_without_sound_effects_gets_none() {
    let p = project();
    let mut calls = base();
    calls.extend(sticker("Rocket", 10));
    calls.push(sound("Chime", 130));
    let out = compile(
        &p,
        &inputs(&p, "Make a 60 second short, no sound effects"),
        &calls,
    );
    let after = short(&out);
    let (pictures, sounds) = layers(&after);
    assert_eq!(pictures.len(), 1);
    assert!(sounds.is_empty(), "{sounds:?}");
}

#[test]
fn outside_a_short_a_sticker_with_a_place_goes_there_and_one_without_goes_beside_the_speaker() {
    // A vertical version and stickers, no cut and no short asked for: not a short. The message names no place and no
    // sound, so only the gate decides.
    let p = project();
    let calls = vec![
        call(
            "setFormat",
            json!({"format": "portrait", "keepOthers": true}),
        ),
        sticker("Rocket", 10)[0].clone(),
        call(
            "addImage",
            json!({"file": "Heart", "size": null, "position": null, "atSec": null, "atWords": w(40), "durationSec": null}),
        ),
        sound("Impact", 70),
    ];
    let out = compile(
        &p,
        &inputs(&p, "add the pictures on w10 and w40 and a sound on w70"),
        &calls,
    );
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    assert!(out.told.is_empty());
    let tx = out.additions.as_ref().expect("additions");
    let after = klipolo_model::apply_ops(&p, &tx.ops).unwrap();
    let (pictures, sounds) = layers(&after);
    // The Rocket where the model put it: its word, 2 s, the top right.
    let rocket = &pictures[0];
    assert!(near(rocket.1, 5.5) && near(rocket.2, 2.0), "{pictures:?}");
    assert!(portrait_box(&after, &rocket.3).x > 0.5);
    // The Heart, given no place: beside the speaker for 1.5 s, popping in, never in the middle on them.
    let heart = &pictures[1];
    assert!(near(heart.2, 1.5), "{pictures:?}");
    let b = portrait_box(&after, &heart.3);
    assert!(!overlaps(b, klipolo_chat::compile::FACE_ZONE), "{b:?}");
    assert!((0.17..=0.24).contains(&b.w), "{b:?}");
    let enter = heart
        .3
        .layout
        .motion
        .as_ref()
        .and_then(|m| m.enter.as_ref());
    assert!(enter.is_some_and(|u| u.preset.as_str().ends_with("motion.pop")));
    // The sound where the model put it.
    assert_eq!(sounds, vec![("Impact".to_owned(), 35.5)]);
}

#[test]
fn a_vertical_project_given_a_plain_trim_is_no_short() {
    // Already vertical; a trim and a sound: placed as asked, as on main.
    let mut p = project();
    p.settings.formats = vec![FormatKind::Portrait];
    p.settings.primary_format = FormatKind::Portrait;
    let calls = vec![
        call(
            "keepOnly",
            json!({"ranges": [{"startSec": 0.5, "endSec": 40.0}]}),
        ),
        sound("Impact", 70),
    ];
    let out = compile(
        &p,
        &inputs(&p, "keep the first 40 seconds and add a sound on w70"),
        &calls,
    );
    assert!(out.told.is_empty(), "{:?}", out.told);
    let after = short(&out);
    let (_, sounds) = layers(&after);
    assert_eq!(sounds.len(), 1, "{sounds:?}");
}

#[test]
fn a_sticker_the_person_names_stays_even_where_the_rules_leave_others_out() {
    // "a heart when he says w4": w4 comes under the hook, where no sticker shows unless the person asked for it.
    let p = project();
    let mut calls = base();
    calls.extend(sticker("Heart", 4));
    let out = compile(
        &p,
        &inputs(&p, "Make a 60 second short and put a heart on w4"),
        &calls,
    );
    let after = short(&out);
    let (pictures, _) = layers(&after);
    assert_eq!(pictures.len(), 1, "{pictures:?}");
    assert!(near(pictures[0].1, in_short(&after, 2.4)), "{pictures:?}");
    let told: HashMap<usize, &str> = (out.told.iter()).map(|(i, s)| (*i, s.as_str())).collect();
    assert!(told[&5].starts_with("done"), "{told:?}");
}

#[test]
fn a_sound_the_person_names_by_what_it_is_is_kept() {
    // Named by a tag that isn't its first: "cha ching" is the Cash register's.
    let p = project();
    let mut i = inputs(&p, "Make a 60 second short with a cha ching on w70");
    i.library.items.push(LibraryItem {
        name: "Cash register".into(),
        kind: MediaKind::Sound,
        asset: asset("pack-cash", "audio", "Cash register.wav", 1.4, (0, 0)),
        pack: true,
        tags: vec![
            "cash register".into(),
            "money".into(),
            "sale".into(),
            "cha ching".into(),
        ],
    });
    let mut calls = base();
    calls.push(sound("Cash register", 70));
    let out = compile(&p, &i, &calls);
    let after = short(&out);
    let (_, sounds) = layers(&after);
    let kept: Vec<f64> = (sounds.iter())
        .filter(|(n, _)| n == "Cash register")
        .map(|(_, t)| *t)
        .collect();
    assert_eq!(kept.len(), 1, "{sounds:?}");
    assert!(near(kept[0], in_short(&after, 35.5)), "{sounds:?}");
}

#[test]
fn a_later_call_never_takes_out_a_sticker_an_earlier_one_placed() {
    // Fire (w70, 35.5 s) is called first; Star (w60, 30.5 s, earlier in the short but within 10 s of Fire) after it,
    // as a later round would retry it. Fire stays; Star is left out.
    let p = project();
    let mut calls = base();
    calls.extend(sticker("Fire", 70));
    calls.extend(sticker("Star", 66));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    let after = short(&out);
    let (pictures, _) = layers(&after);
    let names: Vec<&str> = pictures.iter().map(|(n, ..)| n.as_str()).collect();
    assert_eq!(names, vec!["Fire"], "{pictures:?}");
}

#[test]
fn whooshes_follow_the_transcripts_speakers_over_the_scene_logs_words() {
    // The scene log names one man two ways; the transcript's turns say one voice speaks to 60 s and another after.
    let p = project();
    let mut i = inputs(&p, MESSAGE);
    let scene = |start: f64, end: f64, who: &str| SceneEntry {
        start_us: secs(start),
        end_us: secs(end),
        kind: "speech".into(),
        who: who.into(),
        text: "talks".into(),
        on_screen_text: None,
    };
    i.scenes.insert(
        "panel".into(),
        SceneLog {
            items: vec![
                scene(0.0, 40.0, "man in a blue shirt"),
                scene(40.0, 120.0, "the man"),
            ],
        },
    );
    let turn = |start: f64, end: f64, who: &str| klipolo_chat::SpeakerTurn {
        start_us: secs(start),
        end_us: secs(end),
        speaker: Some(who.into()),
    };
    i.turns.insert(
        "panel".into(),
        vec![turn(0.0, 60.0, "A"), turn(60.0, 120.0, "B")],
    );
    let mut calls = base();
    calls.push(sound("Chime", 130));
    let out = compile(&p, &i, &calls);
    let after = short(&out);
    let (_, sounds) = layers(&after);
    let whooshes: Vec<f64> = (sounds.iter())
        .filter(|(n, _)| n == "Whoosh")
        .map(|(_, t)| *t)
        .collect();
    // The cuts: at 32 s (A again), 62 s (A to B) and 92 s (B again): one whoosh, at the second.
    let cuts = cuts(&after);
    assert_eq!(whooshes.len(), 1, "{sounds:?}");
    assert!(near(whooshes[0], cuts[2]), "{sounds:?} {cuts:?}");
}

#[test]
fn a_sticker_placed_before_the_follow_moves_beside_the_head_where_the_path_puts_it() {
    // At compile time the follow hasn't run: the sticker goes beside the middle. Once the path lands with the speaker
    // near the picture's left edge (the crop meets it, so in 9:16 they sit left of the middle), it moves beside them.
    let p = project();
    let mut calls = base();
    calls.extend(sticker("Rocket", 10));
    calls.extend(sticker("Fire", 70));
    let out = compile(&p, &inputs(&p, MESSAGE), &calls);
    assert_eq!(out.beside.len(), 2, "{:?}", out.beside);
    let mut after = short(&out);
    let path = FocusPath {
        keys: vec![FocusKey {
            at_us: 0,
            x: 0.1,
            y: 0.5,
            cut: false,
        }],
    };
    for clip in &mut after.timeline.main {
        clip.fill_focus.insert(
            FormatKind::Portrait,
            FillFocus::Speaker {
                path: path.clone(),
                keep: Keep::Speaker,
                people: Vec::new(),
            },
        );
    }
    let mut ops = out.additions.as_ref().expect("additions").ops.clone();
    klipolo_chat::place_beside_again(&after, &mut ops, &out.beside);
    let head_right = 0.1 / (9.0 / 16.0 / (16.0 / 9.0)) + 0.22;
    for (id, _) in &out.beside {
        let item = (ops.iter())
            .find_map(|op| match op {
                Op::AddItem { item, .. } if &item.id == id => Some(item.clone()),
                _ => None,
            })
            .expect("the sticker");
        let b = portrait_box(&after, &item);
        // Both on the right of the off-centre head, near it: the left (Fire's turn) has no room.
        assert!(b.x >= head_right - 1e-6, "on the head: {b:?}");
        assert!(b.x - head_right < 0.05, "far from the head: {b:?}");
    }
}
