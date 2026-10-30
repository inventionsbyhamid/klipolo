mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{rec_source, refused, secs, sorted_keys};
use klipolo_model::ops::{op_risk, Edge, Op, OpRisk};
use klipolo_model::patch::{apply, diff};
use klipolo_model::project::{
    Asset, AssetKind, Project, Recording, RecordingKind, SourceRole, SourceRun,
};
use klipolo_model::speech::{
    run_spans, spoken, voice_runs, word_cut, Speech, SpeechSpan, Transcript, VoiceRun, Word,
    WordKind,
};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_model::{apply_ops, ModelError};

fn media(id: &str, kind: AssetKind, seconds: f64, has_audio: bool) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs(seconds),
        width: 1920,
        height: 1080,
        fps: if kind == AssetKind::Video { 30.0 } else { 0.0 },
        rotation: None,
        has_audio,
        audio_lead_us: None,
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

fn pause(start: f64, end: f64) -> Word {
    Word {
        text: String::new(),
        kind: WordKind::Pause,
        ..word("", start, end)
    }
}

fn transcript(words: Vec<Word>) -> Arc<Transcript> {
    Arc::new(Transcript {
        key: "k".into(),
        language: "en".into(),
        model: "speech-whisper-small-q5_1".into(),
        covered_us: secs(300.0),
        complete: true,
        words,
    })
}

fn clip(id: &str, source: ClipSource, from: f64, to: f64, speed: f64) -> Clip {
    let mut c = Clip::new(source, secs(from), secs(to));
    c.id = id.into();
    c.speed = speed;
    c
}

/// A recording whose mic runs 0.5 s behind the screen (mic time = screen time − 0.5 s), cut into c1 (screen 10–20 s)
/// and c2 (screen 40–50 s at 2×).
fn recorded() -> (Project, Speech) {
    let mut p = Project::new("Speech");
    p.assets = vec![
        media("screen", AssetKind::Video, 300.0, false),
        media("mic", AssetKind::Audio, 300.0, true),
    ];
    p.recordings = vec![Recording {
        id: "r1".into(),
        kind: RecordingKind::Screen,
        sources: vec![
            rec_source(
                "screen",
                SourceRole::Screen,
                "capture.source.screen",
                0,
                300.0,
            ),
            rec_source(
                "mic",
                SourceRole::Mic,
                "capture.source.mic",
                secs(0.5),
                300.0,
            ),
        ],
        primary: "screen".into(),
        cursor: None,
        markers: vec![],
        game: None,
    }];
    let rec = || ClipSource::Recording { id: "r1".into() };
    p.timeline.main = vec![
        clip("c1", rec(), 10.0, 20.0, 1.0),
        clip("c2", rec(), 40.0, 50.0, 2.0),
    ];
    let p = apply_ops(&p, &[]).unwrap();
    // Mic times; on the screen's clock each is 0.5 s later.
    let words = vec![
        word("one", 9.4, 9.6),     // screen 9.9–10.1: its middle is c1's first moment
        word("two", 15.0, 15.4),   // screen 15.5–15.9
        word("three", 19.5, 19.8), // screen 20.0–20.3: past c1's end
        word("four", 39.6, 40.0),  // screen 40.1–40.5: c2
        word("five", 49.4, 49.8),  // screen 49.9–50.3: its middle is past c2's end
    ];
    let speech = Speech {
        transcripts: HashMap::from([("mic".to_owned(), transcript(words))]),
        ..Speech::default()
    };
    (p, speech)
}

/// One imported video `v` (60 s, with sound) as one clip from 0 s, and its words.
fn imported(words: Vec<Word>) -> (Project, Speech) {
    let mut p = Project::new("Import");
    p.assets = vec![media("v", AssetKind::Video, 60.0, true)];
    p.timeline.main = vec![clip(
        "c1",
        ClipSource::Asset { id: "v".into() },
        0.0,
        60.0,
        1.0,
    )];
    let speech = Speech {
        transcripts: HashMap::from([("v".to_owned(), transcript(words))]),
        ..Speech::default()
    };
    (apply_ops(&p, &[]).unwrap(), speech)
}

fn texts(p: &Project, speech: &Speech) -> Vec<(String, Us, Us)> {
    let s = spoken(p, speech);
    s.words
        .iter()
        .map(|w| (s.text(w, p, speech).to_owned(), w.start_us, w.end_us))
        .collect()
}

fn fix(asset: &str, from: f64, to: f64, text: Option<&str>) -> Op {
    Op::FixWords {
        asset_id: asset.into(),
        start_us: secs(from),
        end_us: secs(to),
        text: text.map(str::to_owned),
    }
}

#[test]
fn voice_runs_prefer_the_mic_then_a_sounding_camera_then_the_screen() {
    let (mut p, _) = recorded();
    let c1 = p.timeline.main[0].clone(); // screen 10–20 s
    let run = |asset: &str, offset: f64| VoiceRun {
        asset: asset.into(),
        rec_from: secs(10.0),
        rec_to: secs(20.0),
        offset_us: secs(offset),
        rate: 1.0,
        file_in_us: 0,
    };
    assert_eq!(voice_runs(&p, &c1), [run("mic", 0.5)]);
    p.assets.push(media("cam", AssetKind::Video, 300.0, true));
    let sources = &mut p.recordings[0].sources;
    sources.push(rec_source(
        "cam",
        SourceRole::Camera,
        "capture.source.camera",
        secs(0.25),
        300.0,
    ));
    sources.retain(|s| s.role != SourceRole::Mic);
    assert_eq!(
        voice_runs(&p, &c1),
        [run("cam", 0.25)],
        "a camera with sound, at its own offset"
    );
    p.recordings[0]
        .sources
        .retain(|s| s.role != SourceRole::Camera);
    assert!(
        voice_runs(&p, &c1).is_empty(),
        "a silent screen has no voice"
    );
    p.assets[0].has_audio = true;
    assert_eq!(voice_runs(&p, &c1), [run("screen", 0.0)]);
}

#[test]
fn a_voice_s_runs_carry_their_drift_and_the_source_s_nudge() {
    let (mut p, _) = recorded();
    let c1 = p.timeline.main[0].clone(); // recording 10–20 s
    p.assets.push(media("mic2", AssetKind::Audio, 300.0, true));
    let mic = &mut p.recordings[0].sources[1];
    // The mic reconnected: its first file 0.5–14.5 s, its second from 16 s, running 100 ppm fast; nudged 40 ms later.
    mic.runs[0].duration_us = secs(14.0);
    mic.runs.push(SourceRun {
        asset_id: "mic2".into(),
        offset_us: secs(16.0),
        duration_us: secs(100.0),
        drift_ppm: 100.0,
        file_in_us: 0,
    });
    mic.nudge_us = 40_000;
    let runs = voice_runs(&p, &c1);
    assert_eq!(
        runs,
        [
            VoiceRun {
                asset: "mic".into(),
                rec_from: secs(10.0),
                rec_to: secs(14.54),
                offset_us: secs(0.54),
                rate: 1.0,
                file_in_us: 0,
            },
            VoiceRun {
                asset: "mic2".into(),
                rec_from: secs(16.04),
                rec_to: secs(20.0),
                offset_us: secs(16.04),
                rate: 1.0001,
                file_in_us: 0,
            },
        ]
    );
    // 1 s into the second file's recording time is 1.0001 s of its own.
    assert_eq!(runs[1].local_us(secs(17.04)), 1_000_100);
}

#[test]
fn an_imported_video_speaks_through_itself_and_a_silent_one_has_no_voice() {
    let mut p = Project::new("Import");
    p.assets = vec![
        media("v", AssetKind::Video, 60.0, true),
        media("quiet", AssetKind::Video, 60.0, false),
    ];
    let c = clip("c", ClipSource::Asset { id: "v".into() }, 5.0, 25.0, 1.0);
    assert_eq!(
        voice_runs(&p, &c),
        [VoiceRun {
            asset: "v".into(),
            rec_from: secs(5.0),
            rec_to: secs(25.0),
            offset_us: 0,
            rate: 1.0,
            file_in_us: 0,
        }]
    );
    let q = clip(
        "q",
        ClipSource::Asset { id: "quiet".into() },
        0.0,
        10.0,
        1.0,
    );
    assert!(voice_runs(&p, &q).is_empty());
}

/// A transcript is in its file's sound time, and a picture file's time is its picture's. With the
/// sound starting 40 ms after the picture, a word at 1.000 s of the transcript plays at 1.040 s, through an imported
/// clip and through a camera-only recording whose picture source is the voice; the timeline's 1.040 s reads back as
/// the transcript's 1.000 s (what a word fix is keyed by).
#[test]
fn words_play_where_their_file_s_sound_does() {
    let (mut p, speech) = imported(vec![word("hello", 1.0, 1.2)]);
    p.assets[0].audio_lead_us = Some(40_000);
    let heard = vec![("hello".to_owned(), secs(1.04), secs(1.24))];
    assert_eq!(texts(&p, &speech), heard, "an imported clip");
    assert_eq!(run_spans(&p)[0].local_of(secs(1.04)), secs(1.0));
    p.recordings = vec![Recording {
        id: "r".into(),
        kind: RecordingKind::Camera,
        sources: vec![rec_source(
            "v",
            SourceRole::Camera,
            "capture.source.camera",
            0,
            60.0,
        )],
        primary: "v".into(),
        cursor: None,
        markers: vec![],
        game: None,
    }];
    p.timeline.main = vec![clip(
        "c1",
        ClipSource::Recording { id: "r".into() },
        0.0,
        60.0,
        1.0,
    )];
    assert_eq!(texts(&p, &speech), heard, "a picture source's voice");
}

/// Format 4's drift: 100 ppm is 0.72 s after two hours, six times a word cut's pad. A span maps file time through
/// its rate, and `local_of` undoes it.
#[test]
fn a_drifting_run_maps_words_through_its_rate() {
    let span = SpeechSpan {
        clip: "c".into(),
        asset: "mic".into(),
        first: 0,
        end: 0,
        start_us: 0,
        source_in_us: 0,
        offset_us: 0,
        rate: 1.0001,
        asset_in_us: 0,
        asset_out_us: secs(7_300.0),
        speed: 1.0,
    };
    let at = span.at(secs(7_200.0));
    assert!((at - secs(7_199.28)).abs() < 1_000, "{at}");
    assert!((span.local_of(at) - secs(7_200.0)).abs() <= 2);
}

#[test]
fn spans_follow_cuts_trims_speed_and_reorders() {
    let (p, speech) = recorded();
    assert_eq!(
        texts(&p, &speech),
        [
            ("one".to_owned(), 0, secs(0.1)), // starts before c1: clamped to the clip
            ("two".to_owned(), secs(5.5), secs(5.9)),
            ("four".to_owned(), secs(10.05), secs(10.25)), // c2 starts at 10 s and plays at 2×
        ]
    );
    let p = apply_ops(
        &p,
        &[Op::MoveClip {
            id: "c2".into(),
            index: 0,
        }],
    )
    .unwrap();
    assert_eq!(
        texts(&p, &speech)[0],
        ("four".to_owned(), secs(0.05), secs(0.25))
    );
    let p = apply_ops(
        &p,
        &[Op::TrimClip {
            id: "c1".into(),
            edge: Edge::Start,
            source_us: secs(15.0),
        }],
    )
    .unwrap();
    let t = texts(&p, &speech);
    assert_eq!(
        t.iter().map(|w| w.0.as_str()).collect::<Vec<_>>(),
        ["four", "two"],
        "the trim took \"one\""
    );
    assert_eq!(
        t[1].1,
        secs(5.5),
        "c1 now starts after c2's 5 s, and \"two\" 0.5 s into it"
    );
}

#[test]
fn a_duplicated_clip_speaks_its_words_twice() {
    let (p, speech) = recorded();
    let mut copy = p.timeline.main[0].clone();
    copy.id = "c1-copy".into();
    let p = apply_ops(
        &p,
        &[Op::InsertClip {
            clip: copy,
            index: Some(1),
        }],
    )
    .unwrap();
    let s = spoken(&p, &speech);
    let clips: Vec<&str> = s
        .words
        .iter()
        .map(|w| s.spans[w.span as usize].clip.as_str())
        .collect();
    assert_eq!(clips, ["c1", "c1", "c1-copy", "c1-copy", "c2"]);
}

#[test]
fn fixes_replace_the_words_they_cover_and_survive_cuts() {
    let (p, speech) = recorded();
    let p = apply_ops(&p, &[fix("mic", 15.0, 15.4, Some("2"))]).unwrap();
    assert_eq!(
        texts(&p, &speech)[1],
        ("2".to_owned(), secs(5.5), secs(5.9))
    );
    // A cut before it moves it on the timeline; the fix is in the mic's own time, so it stays.
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: 0,
                end: secs(3.0),
            }],
        }],
    )
    .unwrap();
    assert_eq!(
        texts(&p, &speech)[0],
        ("2".to_owned(), secs(2.5), secs(2.9))
    );
    // A new fix over the same words replaces it; no text removes every fix in its range.
    let p = apply_ops(&p, &[fix("mic", 14.9, 15.5, Some("two!"))]).unwrap();
    assert_eq!(p.word_fixes.len(), 1);
    let p = apply_ops(&p, &[fix("mic", 0.0, 300.0, None)]).unwrap();
    assert!(p.word_fixes.is_empty());
    assert_eq!(texts(&p, &speech)[0].0, "two");
}

/// A fix that a split, a cut or a removed pause puts into two pieces reads once, from the piece with
/// its first word. The other piece keeps its time, so it can be selected and cut, and reads nothing.
#[test]
fn a_fix_split_into_pieces_reads_once() {
    let (p, speech) = imported(vec![
        word("I'll", 1.0, 1.3),
        word("show", 1.4, 1.7),
        pause(1.7, 2.6),
        word("you", 2.6, 2.9),
        word("how", 3.0, 3.3),
    ]);
    let p = apply_ops(&p, &[fix("v", 0.9, 3.0, Some("I will show you"))]).unwrap();
    assert_eq!(
        texts(&p, &speech),
        [
            ("I will show you".to_owned(), secs(1.0), secs(2.9)),
            ("how".to_owned(), secs(3.0), secs(3.3)),
        ]
    );
    let r = |a: f64, b: f64| RangeUs {
        start: secs(a),
        end: secs(b),
    };
    for (why, op) in [
        ("a split inside it", Op::Split { at_us: secs(1.35) }),
        (
            "a cut that keeps words on both sides",
            Op::DeleteRanges {
                ranges: vec![r(1.35, 1.75)],
            },
        ),
        (
            "its pause removed",
            Op::DeleteRanges {
                ranges: vec![r(1.82, 2.48)],
            },
        ),
    ] {
        let p = apply_ops(&p, &[op]).unwrap();
        let s = spoken(&p, &speech);
        let t = texts(&p, &speech);
        assert_eq!(t.len(), 3, "{why}: {t:?}");
        assert_eq!(
            t[0].0, "I will show you",
            "{why}: the piece with its first word reads it"
        );
        let rest = &s.words[1];
        assert!(
            rest.fix == Some(0) && rest.continued && t[1].0.is_empty(),
            "{why}: the later piece reads nothing: {t:?}"
        );
        assert!(
            rest.end_us > rest.start_us,
            "{why}: the later piece keeps its time"
        );
        assert_eq!(t[2].0, "how", "{why}");
    }
}

/// Within one use of a fix's words, the first piece still on the timeline reads it,
/// whichever of its words it holds; a copy is another use and reads it again. So the person's correction shows once
/// per time its words play: never twice for one use, and never not at all while a word of it plays.
#[test]
fn a_fix_reads_once_on_its_first_piece_still_on_the_timeline() {
    let (p, speech) = imported(vec![
        word("I'll", 1.0, 1.3),
        word("show", 1.4, 1.7),
        pause(1.7, 2.6),
        word("you", 2.6, 2.9),
        word("how", 3.0, 3.3),
    ]);
    let p = apply_ops(&p, &[fix("v", 0.9, 3.0, Some("I will show you"))]).unwrap();
    let r = |a: f64, b: f64| RangeUs {
        start: secs(a),
        end: secs(b),
    };
    let read =
        |p: &Project| -> Vec<String> { texts(p, &speech).into_iter().map(|w| w.0).collect() };
    // Its first word cut, and its pause: the first piece kept ("show") reads it, the next ("you") nothing.
    let cut = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![r(0.95, 1.35), r(1.82, 2.48)],
        }],
    )
    .unwrap();
    assert_eq!(read(&cut), ["I will show you", "", "how"]);
    assert_eq!(
        texts(&cut, &speech)[0].1,
        secs(1.0),
        "on \"show\", 0.05 s into the piece kept from 1.35 s, which now starts at 0.95 s"
    );
    // Every word it covers cut: nothing reads it.
    let gone = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![r(0.95, 2.95)],
        }],
    )
    .unwrap();
    assert_eq!(read(&gone), ["how"]);
    // First by the timeline: a later piece moved to the front reads it.
    let split = apply_ops(&p, &[Op::Split { at_us: secs(1.35) }]).unwrap();
    let second = split.timeline.main[1].id.clone();
    let moved = apply_ops(
        &split,
        &[Op::MoveClip {
            id: second,
            index: 0,
        }],
    )
    .unwrap();
    assert_eq!(read(&moved), ["I will show you", "how", ""]);
    // A copy of the clip (a repeated intro) plays its words again and reads the fix there too.
    let mut copy = p.timeline.main[0].clone();
    copy.id = "copy".into();
    let twice = apply_ops(
        &p,
        &[Op::InsertClip {
            clip: copy.clone(),
            index: None,
        }],
    )
    .unwrap();
    assert_eq!(
        read(&twice),
        ["I will show you", "how", "I will show you", "how"]
    );
    // And after the first use is split: its two pieces read once, the copy again.
    let split_twice = apply_ops(
        &split,
        &[Op::InsertClip {
            clip: copy,
            index: None,
        }],
    )
    .unwrap();
    assert_eq!(
        read(&split_twice),
        ["I will show you", "", "how", "I will show you", "how"]
    );
}

/// Undo and redo go through the history's patches (`patch::diff` and `apply`), which carry the fixes.
#[test]
fn undo_and_redo_bring_fixes_back() {
    let (before, speech) = recorded();
    let after = apply_ops(&before, &[fix("mic", 15.0, 15.4, Some("2"))]).unwrap();
    let (undo, redo) = (diff(&after, &before), diff(&before, &after));
    let undone = apply(&after, &undo).unwrap();
    assert_eq!(texts(&undone, &speech)[1].0, "two");
    assert_eq!(apply(&undone, &redo).unwrap(), after);
}

#[test]
fn a_fix_needs_media_a_range_and_one_short_line() {
    let (p, _) = recorded();
    assert_eq!(
        apply_ops(&p, &[fix("nope", 1.0, 2.0, Some("x"))]).unwrap_err(),
        ModelError::NotFound {
            kind: "media",
            id: "nope".into()
        }
    );
    assert_eq!(
        apply_ops(&p, &[fix("mic", 2.0, 2.0, Some("x"))]).unwrap_err(),
        refused("A word fix needs a start before its end.")
    );
    let long = "x".repeat(201);
    let marked = ["tab\there", "two\u{2028}lines", "\u{202E}reversed"];
    for text in ["", "two\nlines", long.as_str()].into_iter().chain(marked) {
        assert_eq!(
            apply_ops(&p, &[fix("mic", 1.0, 2.0, Some(text))]).unwrap_err(),
            refused("A word fix is one line of 1 to 200 characters."),
            "{text:?}"
        );
    }
    // Nothing to see: spaces, zero-width and other invisible marks get their own message.
    for text in [" ", "\u{200B}", " \u{2060}\u{FEFF} ", "\u{3164}"] {
        assert_eq!(
            apply_ops(&p, &[fix("mic", 1.0, 2.0, Some(text))]).unwrap_err(),
            refused("A word fix needs text you can see."),
            "{text:?}"
        );
    }
    assert!(
        apply_ops(&p, &[fix("mic", 1.0, 2.0, Some("Notion\u{200B}"))]).is_ok(),
        "an invisible mark beside words is kept"
    );
}

/// Tag characters draw nothing: a correction is stored without them, so it can't carry text the person never sees
///; a correction of tags only is one nobody could see.
#[test]
fn a_fix_is_stored_without_tag_characters() {
    let (p, _) = recorded();
    let hidden = "Notion\u{E0001}\u{E0069}\u{E0067}\u{E006E}";
    let p = apply_ops(&p, &[fix("mic", 1.0, 2.0, Some(hidden))]).unwrap();
    assert_eq!(p.word_fixes[0].text, "Notion");
    assert_eq!(
        apply_ops(&p, &[fix("mic", 3.0, 4.0, Some("\u{E0041}\u{E0042}"))]).unwrap_err(),
        refused("A word fix needs text you can see.")
    );
}

#[test]
fn removing_unused_media_drops_its_word_fixes() {
    let (mut p, _) = recorded();
    p.assets.push(media("spare", AssetKind::Audio, 10.0, true));
    let p = apply_ops(&p, &[fix("spare", 1.0, 2.0, Some("hi"))]).unwrap();
    let p = apply_ops(&p, &[Op::RemoveAsset { id: "spare".into() }]).unwrap();
    assert!(p.word_fixes.is_empty());
}

#[test]
fn fixing_words_is_additive() {
    assert_eq!(op_risk(&fix("mic", 1.0, 2.0, Some("x"))), OpRisk::Additive);
    assert_eq!(
        op_risk(&fix("mic", 1.0, 2.0, None)),
        OpRisk::Additive,
        "the words heard come back"
    );
}

#[test]
fn word_cut_never_takes_more_than_half_a_gap() {
    let (p, speech) = imported(vec![
        word("a", 1.0, 1.3),
        word("b", 1.5, 1.8),
        pause(1.8, 2.6),
        word("c", 2.6, 2.9),
    ]);
    let s = spoken(&p, &speech);
    let cut = |from, to| word_cut(&s, from, to, 50); // 20 ms frames: every expected time is on one
    let r = |a: f64, b: f64| RangeUs {
        start: secs(a),
        end: secs(b),
    };
    assert_eq!(
        cut(1, 1),
        r(1.4, 1.92),
        "half the 200 ms before b; 120 ms into the pause after it"
    );
    assert_eq!(cut(2, 2), r(1.92, 2.48), "a pause keeps 120 ms at each end");
    assert_eq!(cut(1, 3), r(1.4, 3.02), "the last word gets the whole pad");
    assert_eq!(
        cut(0, 0),
        r(0.88, 1.4),
        "the first word gets the whole pad before it"
    );
}

/// The fixture the Script view's TS mapping (`apps/desktop/src/lib/speech.ts`) is checked against, like
/// `layout-parity.json`: a fix split in two pieces by a split (only the first reads it), a fix the drifting copy plays
/// again (another use: it reads in c2 and in the copy), two more copies of the split fix (c1 unsplit, and c1's second
/// piece alone: each covers words the use before already covered, so each reads it again), and a word
/// whose start lands half-way between two microseconds.
#[test]
fn speech_parity_json_is_current() {
    let (p, mut speech) = recorded();
    // At odd microseconds in c2 (2×): both sides round its half-way times away from zero.
    let mut words = speech.transcripts["mic"].words.clone();
    words.insert(
        4,
        Word {
            start_us: 44_000_001,
            end_us: 44_300_001,
            ..word("and", 0.0, 0.0)
        },
    );
    speech
        .transcripts
        .insert("mic".into(), transcript(words.clone()));
    let p = apply_ops(
        &p,
        &[
            fix("mic", 9.0, 16.0, Some("ONE TWO")),
            fix("mic", 39.6, 40.0, Some("4")),
            Op::Split { at_us: secs(3.0) },
        ],
    )
    .unwrap();
    let mut spans = klipolo_model::speech::spans(&p, &speech);
    // A drifting run (format 4: rate ≠ 1) the TS mapping must follow too: c2's stretch of the recording again, 20 s
    // later, through a 100 ppm clock, with its file times and words as `run_spans` and `spans` would choose them.
    let c2 = spans.last().unwrap().clone();
    let run = VoiceRun {
        asset: "mic".into(),
        rec_from: secs(40.0),
        rec_to: secs(50.0),
        offset_us: secs(0.5),
        rate: 1.0001,
        file_in_us: 0,
    };
    let chosen =
        |at: Us| words.partition_point(|w| w.start_us + (w.end_us - w.start_us) / 2 < at) as u32;
    let (asset_in_us, asset_out_us) = (run.local_us(run.rec_from), run.local_us(run.rec_to));
    spans.push(SpeechSpan {
        clip: "c2-drift".into(),
        start_us: c2.start_us + secs(20.0),
        rate: run.rate,
        first: chosen(asset_in_us),
        end: chosen(asset_out_us),
        asset_in_us,
        asset_out_us,
        ..c2
    });
    // c1 placed again whole (screen 10–20 s: "one" and "two", one piece that clears the use split before it), then c1's
    // second piece alone ("two" again: it reads on "two").
    let (c1, c1_2) = (spans[0].clone(), spans[1].clone());
    spans.push(SpeechSpan {
        clip: "c1-copy".into(),
        start_us: secs(40.0),
        first: chosen(c1.asset_in_us),
        end: chosen(c1_2.asset_out_us),
        asset_out_us: c1_2.asset_out_us,
        ..c1
    });
    spans.push(SpeechSpan {
        clip: "c1-2-copy".into(),
        start_us: secs(55.0),
        ..c1_2
    });
    let s = klipolo_model::speech::spoken_from(spans, &p.word_fixes, &speech);
    for (fixed, uses) in [("ONE TWO", 3), ("4", 2)] {
        assert_eq!(
            s.words
                .iter()
                .filter(|w| s.text(w, &p, &speech) == fixed)
                .count(),
            uses,
            "{fixed} reads once per use"
        );
    }
    let fixture = serde_json::json!({
        "spans": s.spans,
        "transcripts": { "mic": speech.transcripts["mic"].as_ref() },
        "fixes": p.word_fixes,
        "words": s.words.iter().map(|w| serde_json::json!({
            "span": w.span, "word": w.word, "fix": w.fix, "text": s.text(w, &p, &speech),
            "startUs": w.start_us, "endUs": w.end_us, "kind": w.kind,
        })).collect::<Vec<_>>(),
    });
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/lib/speech-parity.json");
    // Sorted keys: the same file whether or not klipolo-ai's `preserve_order` builds alongside.
    let text = serde_json::to_string_pretty(&sorted_keys(fixture)).unwrap() + "\n";
    if std::env::var_os("KLIPOLO_UPDATE_PARITY").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_default(),
        text,
        "stale: run with KLIPOLO_UPDATE_PARITY=1"
    );
}
