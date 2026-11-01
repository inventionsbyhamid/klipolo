//! What the chat crate tells other apps that edit through its tools: one section of the context by its heading, the
//! transcript's lines whole however long the project, a key that says whether a context's times and clip refs still
//! hold, and which tool calls name the timeline.
//!
//! The project is clip A (the fixtures `tests/context.rs` describes) as two pieces around a cut, or many copies of it;
//! the 30-minute talk is made here, so its context has to tell the transcript briefly.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use klipolo_chat::context::QUOTED_NOTE;
use klipolo_chat::tools::{
    BubbleChange, ClipRef, PictureSize, Position, Sound, TextPreset, TransitionKind,
};
use klipolo_chat::{
    ChatContext, FullTranscript, Library, LibraryItem, MediaKind, Moment, SceneLog, SpeakerTurn,
    ToolCall, View, build_context, full_transcript, timeline_key, tools,
};
use klipolo_model::captions::styles::BuiltinCaptionStyle;
use klipolo_model::layout::Anchor;
use klipolo_model::ops::Edge;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Settings};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{CameraMode, Clip, ClipSource};
use klipolo_model::{Op, Project, apply_ops, load_project};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

/// What `build_context` and `full_transcript` read besides the view.
struct Inputs {
    project: Project,
    speech: Speech,
    turns: HashMap<String, Vec<SpeakerTurn>>,
    scenes: HashMap<String, SceneLog>,
}

impl Inputs {
    fn context(&self, view: &View) -> ChatContext {
        build_context(
            &self.project,
            &self.speech,
            &self.turns,
            &self.scenes,
            view,
            &Library::default(),
        )
    }

    fn full(&self) -> FullTranscript {
        full_transcript(&self.project, &self.speech, &self.turns)
    }
}

/// Clip A as two pieces around 13.0 to 24.0 s of the file, with its transcript, speaker turns and scene log.
fn two_clips() -> Inputs {
    let project =
        load_project(&fixture("project_two_clips.json")).expect("the fixture project loads");
    let transcript: Transcript = serde_json::from_str(&fixture("transcript_clip_a.json")).unwrap();
    let turns: Vec<SpeakerTurn> = serde_json::from_str(&fixture("turns_clip_a.json")).unwrap();
    let scenes: SceneLog = serde_json::from_str(&fixture("scenes_clip_a.json")).unwrap();
    let mut speech = Speech::default();
    speech
        .transcripts
        .insert("clip-a".into(), Arc::new(transcript));
    Inputs {
        project,
        speech,
        turns: HashMap::from([("clip-a".to_owned(), turns)]),
        scenes: HashMap::from([("clip-a".to_owned(), scenes)]),
    }
}

/// Clip A `copies` times over, each copy the whole file: about 2.6 minutes of speech and scenes a copy.
fn repeated(copies: usize) -> Inputs {
    let mut inputs = two_clips();
    let piece = inputs.project.timeline.main[0].clone();
    let whole = inputs.project.assets[0].duration_us;
    inputs.project.timeline.main = (0..copies)
        .map(|i| Clip {
            id: format!("copy-{i}"),
            source_in_us: 0,
            source_out_us: whole,
            ..piece.clone()
        })
        .collect();
    inputs
}

fn view_at(playhead: f64) -> View {
    View {
        playhead_us: secs(playhead),
        selection: None,
    }
}

fn late_view() -> View {
    View {
        playhead_us: secs(1000.0),
        selection: Some(RangeUs {
            start: secs(990.0),
            end: secs(995.5),
        }),
    }
}

/// A line's `[a-b]` in seconds.
fn times_of(line: &str) -> (f64, f64) {
    let inside = &line[1..line.find(']').unwrap()];
    let (a, b) = inside.split_once('-').unwrap();
    (a.parse().unwrap(), b.parse().unwrap())
}

/// `us` as the context shows it: seconds to the nearest tenth.
fn shown(us: Us) -> f64 {
    ((us + 50_000) / 100_000) as f64 / 10.0
}

/// The Transcript section the context would write with every line of `full` whole.
fn as_context(full: &FullTranscript) -> String {
    let mut out = String::from("## Transcript\n");
    for line in &full.lines {
        out.push_str(&line.text);
        out.push('\n');
    }
    out.push_str(&full.notes);
    out.push('\n');
    out
}

#[test]
fn section_returns_one_heading_block() {
    // An hour of clip A: Transcript keeps its sentence starts, Scenes their kind and who.
    let inputs = repeated(24);
    let ctx = inputs.context(&late_view());
    let project = ctx.section("Project").expect("Project");
    assert!(
        project.starts_with("## Project\nName: Space station Q&A\n"),
        "{project}"
    );
    assert!(
        project.ends_with(&format!("{QUOTED_NOTE}\n\n")),
        "{project}"
    );
    let transcript = ctx.section("Transcript").expect("Transcript");
    assert!(
        transcript.starts_with("## Transcript (sentence starts)\n["),
        "{transcript}"
    );
    let scenes = ctx.section("Scenes").expect("Scenes");
    assert!(
        scenes.starts_with("## Scenes (kind and who only)\n["),
        "{scenes}"
    );
    let now = ctx.section("Now").expect("Now");
    assert_eq!(
        now,
        "## Now\nPlayhead: 1000.0 in c7\nSelection: 990.0-995.5\n"
    );
    // Each block runs to the next heading: in order, they are the whole text.
    for block in [project, transcript, scenes, now] {
        assert!(!block[1..].contains("\n## "), "one block: {block}");
    }
    assert_eq!(format!("{project}{transcript}{scenes}{now}"), ctx.text);
    // A heading the context doesn't have, a part of one, or one in another case.
    for missing in [
        "Library",
        "Proj",
        "Scene",
        "now",
        "Transcript (sentence",
        "",
    ] {
        assert_eq!(ctx.section(missing), None, "{missing:?}");
    }

    // With nothing shortened, the headings are bare, and Library sits before Now.
    let short = two_clips();
    let library = Library {
        items: vec![LibraryItem {
            name: "lofi.mp3".into(),
            kind: MediaKind::Music,
            asset: Asset {
                id: "lofi".into(),
                kind: AssetKind::Audio,
                path: "/media/lofi.mp3".into(),
                name: "lofi.mp3".into(),
                duration_us: secs(60.0),
                width: 0,
                height: 0,
                fps: 0.0,
                rotation: None,
                has_audio: true,
                audio_lead_us: None,
                made_with: None,
            },
            pack: false,
            tags: Vec::new(),
        }],
    };
    let ctx = build_context(
        &short.project,
        &short.speech,
        &short.turns,
        &short.scenes,
        &view_at(0.0),
        &library,
    );
    assert!(
        ctx.section("Transcript")
            .unwrap()
            .starts_with("## Transcript\n[")
    );
    assert!(ctx.section("Scenes").unwrap().starts_with("## Scenes\n["));
    assert_eq!(
        ctx.section("Library").unwrap(),
        "## Library\nMusic: lofi.mp3\n\n"
    );
    assert!(ctx.section("Scenes").unwrap().ends_with("\n\n"));
    assert_eq!(
        ctx.section("Now").unwrap(),
        "## Now\nPlayhead: 0.0 in c1\nSelection: none\n"
    );
}

#[test]
fn full_transcript_equals_the_context_lines_for_a_short_project() {
    let inputs = two_clips();
    let ctx = inputs.context(&view_at(0.0));
    let full = inputs.full();
    assert!(full.lines.len() > 10, "{} lines", full.lines.len());
    assert_eq!(full.notes, "");
    assert_eq!(ctx.section("Transcript").unwrap(), as_context(&full));
    // Each line's range is its words' times on the timeline, which its `[a-b]` shows to a tenth; in timeline order.
    for line in &full.lines {
        assert!(!line.text.contains('\n'), "{:?}", line.text);
        assert_eq!(
            times_of(&line.text),
            (shown(line.range.start), shown(line.range.end)),
            "{}",
            line.text
        );
    }
    assert!(
        full.lines
            .windows(2)
            .all(|w| w[0].range.end <= w[1].range.start)
    );

    // Clips with no words yet: no lines, and the notes are what the context says after its lines.
    let none = full_transcript(&inputs.project, &Speech::default(), &HashMap::new());
    assert!(none.lines.is_empty());
    assert_eq!(none.notes, "Not transcribed yet: c1, c2.\n");
    let ctx = build_context(
        &inputs.project,
        &Speech::default(),
        &HashMap::new(),
        &HashMap::new(),
        &view_at(0.0),
        &Library::default(),
    );
    assert_eq!(ctx.section("Transcript").unwrap(), as_context(&none));

    // A transcript still being made: the lines heard so far, then how far it reaches.
    let mut heard = two_clips();
    let mut transcript = (*heard.speech.transcripts["clip-a"]).clone();
    transcript.complete = false;
    transcript.covered_us = secs(60.0);
    transcript.words.retain(|w| w.end_us <= secs(60.0));
    heard
        .speech
        .transcripts
        .insert("clip-a".into(), Arc::new(transcript));
    let full = heard.full();
    assert_eq!(full.notes, "Still transcribing: c2 (up to 49.0).\n");
    assert!(!full.lines.is_empty());
    assert_eq!(
        heard.context(&view_at(0.0)).section("Transcript").unwrap(),
        as_context(&full)
    );
}

/// One file, "talk.mov", 30 minutes of one speaker: a 27-word sentence after another, 5 words a second.
fn talk_30_minutes() -> Inputs {
    let length = secs(1800.0);
    let mut words = Vec::new();
    let mut at = secs(0.5);
    let mut part = 1;
    'talk: loop {
        let sentence = format!(
            "In part {part} the crew of the station talked about the new module, how it was designed, built, tested \
             and finally flown up on a heavy rocket."
        );
        for token in sentence.split_whitespace() {
            if at + secs(0.15) > length {
                break 'talk;
            }
            words.push(Word {
                text: token.into(),
                start_us: at,
                end_us: at + secs(0.15),
                confidence: 0.9,
                kind: WordKind::Word,
            });
            at += secs(0.2);
        }
        part += 1;
    }
    let mut project = Project::new("Talk");
    project.assets.push(Asset {
        id: "talk".into(),
        kind: AssetKind::Video,
        path: "/media/talk.mov".into(),
        name: "talk.mov".into(),
        duration_us: length,
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    });
    project.timeline.main.push(Clip::new(
        ClipSource::Asset { id: "talk".into() },
        0,
        length,
    ));
    let mut speech = Speech::default();
    speech.transcripts.insert(
        "talk".into(),
        Arc::new(Transcript {
            key: "talk".into(),
            language: "en".into(),
            model: "test".into(),
            covered_us: length,
            complete: true,
            words,
        }),
    );
    Inputs {
        project,
        speech,
        turns: HashMap::new(),
        scenes: HashMap::from([("talk".to_owned(), SceneLog::default())]),
    }
}

#[test]
fn full_transcript_keeps_every_line_of_a_30_minute_project() {
    let inputs = talk_30_minutes();
    let said = inputs.speech.transcripts["talk"].words.len();
    let ctx = inputs.context(&view_at(0.0));
    let transcript = ctx.section("Transcript").unwrap();
    // The context tells it briefly: each line's first eight words.
    assert!(
        transcript.starts_with("## Transcript (sentence starts"),
        "{}",
        transcript.lines().next().unwrap()
    );
    let full = inputs.full();
    assert_eq!(full.notes, "");
    // Every sentence, every word of it: 27 words a line, the last one cut short where the talk ends.
    let words: usize = (full.lines.iter())
        .map(|l| l.text.split_whitespace().count() - 1)
        .sum();
    assert_eq!(words, said);
    assert_eq!(full.lines.len(), said.div_ceil(27));
    assert!(
        full.lines[..full.lines.len() - 1]
            .iter()
            .all(|l| l.text.ends_with(" and finally flown up on a heavy rocket.")),
        "whole sentences"
    );
    assert!(full.lines.last().unwrap().range.end > secs(1799.0));
    // Each line the context shows briefly is one of these, its times the same and its words their start.
    for brief in transcript.lines().filter(|l| l.starts_with('[')) {
        let start = brief.trim_end_matches(" ...");
        let times = &brief[..brief.find("] ").unwrap() + 2];
        let line = (full.lines.iter())
            .find(|l| l.text.starts_with(times))
            .unwrap_or_else(|| panic!("no full line for {brief}"));
        assert!(line.text.starts_with(start), "{brief} / {}", line.text);
    }
    assert!(transcript.len() * 2 < as_context(&full).len());
}

#[test]
fn timeline_key_ignores_format_captions_and_word_fixes_but_not_a_cut_or_split() {
    let inputs = two_clips();
    let p = &inputs.project;
    let key = timeline_key(p);
    assert_eq!(
        timeline_key(&p.clone()),
        key,
        "the same project, the same key"
    );
    let after = |op: Op| {
        let next = apply_ops(p, &[op]).expect("the op applies");
        assert_ne!(next, *p, "the op changed the project");
        timeline_key(&next)
    };
    // What leaves every clip where it was: the format, captions, a word read differently.
    let landscape = Settings {
        formats: vec![FormatKind::Landscape],
        primary_format: FormatKind::Landscape,
        ..p.settings.clone()
    };
    assert_eq!(
        after(Op::SetSettings {
            settings: landscape
        }),
        key
    );
    let mut captions = p.captions.clone();
    captions.enabled = false;
    assert_eq!(after(Op::SetCaptions { captions }), key);
    let first = &inputs.speech.transcripts["clip-a"].words[0];
    assert_eq!(
        after(Op::FixWords {
            asset_id: "clip-a".into(),
            start_us: first.start_us,
            end_us: first.end_us,
            text: Some("And".into()),
        }),
        key
    );
    // What moves or renames the clips: a cut, a split.
    let cut = after(Op::DeleteRanges {
        ranges: vec![RangeUs {
            start: secs(1.0),
            end: secs(2.0),
        }],
    });
    assert_ne!(cut, key);
    let split = after(Op::Split { at_us: secs(5.0) });
    assert_ne!(split, key);
    assert_ne!(split, cut);
    // A cut in the last clip moves no clip's start, only its end.
    let late = after(Op::DeleteRanges {
        ranges: vec![RangeUs {
            start: secs(100.0),
            end: secs(101.0),
        }],
    });
    assert_ne!(late, key);
}

/// One call of every tool, with and without times where a time is optional, and whether it names the timeline.
fn every_call() -> Vec<(ToolCall, bool)> {
    let span = RangeUs {
        start: secs(1.0),
        end: secs(2.0),
    };
    vec![
        (ToolCall::CutRange { range: span }, true),
        (ToolCall::KeepOnly { ranges: vec![span] }, true),
        (ToolCall::RemoveSilences { min_pause_us: None }, false),
        (
            ToolCall::RemoveSilences {
                min_pause_us: Some(secs(1.0)),
            },
            false,
        ),
        (ToolCall::RemoveFillers, false),
        (
            ToolCall::TrimClip {
                clip: "c2".into(),
                edge: Edge::Start,
                to_us: secs(20.0),
            },
            true,
        ),
        (ToolCall::SplitAt { at_us: secs(5.0) }, true),
        (
            ToolCall::SetClip {
                clip: ClipRef::One("c2".into()),
                speed: Some(2.0),
                volume: None,
                muted: None,
                framing: None,
            },
            true,
        ),
        (
            ToolCall::SetClip {
                clip: ClipRef::All,
                speed: None,
                volume: None,
                muted: Some(true),
                framing: None,
            },
            false,
        ),
        (
            ToolCall::SetTransition {
                after: ClipRef::One("c1".into()),
                kind: Some(TransitionKind::Dissolve),
                duration_us: None,
            },
            true,
        ),
        (
            ToolCall::SetTransition {
                after: ClipRef::All,
                kind: None,
                duration_us: None,
            },
            false,
        ),
        (
            ToolCall::SetFormat {
                format: FormatKind::Square,
                keep_others: true,
            },
            false,
        ),
        (
            ToolCall::SetCaptions {
                on: true,
                style: Some(BuiltinCaptionStyle::Karaoke),
            },
            false,
        ),
        (
            ToolCall::FixCaptionWords {
                find: "Caden".into(),
                replace: "Kaiden".into(),
            },
            false,
        ),
        (
            ToolCall::FindMoments {
                query: "a question".into(),
                moments: vec![Moment {
                    range: span,
                    label: "the first".into(),
                }],
            },
            true,
        ),
        (
            ToolCall::FindMoments {
                query: "nothing".into(),
                moments: Vec::new(),
            },
            true,
        ),
        (
            ToolCall::FollowSubject {
                description: "woman astronaut".into(),
            },
            false,
        ),
        (
            ToolCall::BlurObjects {
                description: "face".into(),
            },
            false,
        ),
        (
            ToolCall::AddMusic {
                file: "lofi.mp3".into(),
                volume: None,
                duck: None,
                at_us: None,
            },
            false,
        ),
        (
            ToolCall::AddMusic {
                file: "lofi.mp3".into(),
                volume: None,
                duck: None,
                at_us: Some(secs(3.0)),
            },
            true,
        ),
        (
            ToolCall::AddSound {
                name: "whoosh".into(),
                at_us: vec![secs(3.0)],
            },
            true,
        ),
        (
            ToolCall::AddText {
                text: "Hello".into(),
                preset: TextPreset::Title,
                at_us: None,
                duration_us: None,
            },
            false,
        ),
        (
            ToolCall::AddText {
                text: "Hello".into(),
                preset: TextPreset::Title,
                at_us: Some(secs(3.0)),
                duration_us: Some(secs(2.0)),
            },
            true,
        ),
        (
            ToolCall::AddImage {
                file: "logo.png".into(),
                size: Some(PictureSize::Small),
                position: Some(Position::TopRight),
                at_us: None,
                duration_us: None,
            },
            false,
        ),
        (
            ToolCall::AddImage {
                file: "logo.png".into(),
                size: None,
                position: None,
                at_us: Some(secs(3.0)),
                duration_us: None,
            },
            true,
        ),
        (
            ToolCall::MakeSticker {
                prompt: "a rocket".into(),
                position: None,
                at_us: None,
                duration_us: None,
            },
            false,
        ),
        (
            ToolCall::MakeSticker {
                prompt: "a rocket".into(),
                position: None,
                at_us: Some(secs(3.0)),
                duration_us: None,
            },
            true,
        ),
        (
            ToolCall::MakeTitleCard {
                title: "Space".into(),
                prompt: "stars".into(),
                at_us: None,
                duration_us: None,
            },
            false,
        ),
        (
            ToolCall::MakeTitleCard {
                title: "Space".into(),
                prompt: "stars".into(),
                at_us: Some(secs(0.0)),
                duration_us: None,
            },
            true,
        ),
        (
            ToolCall::SetCameraMode {
                clips: None,
                mode: CameraMode::Hidden,
                format: None,
            },
            false,
        ),
        (
            ToolCall::AdjustCamera {
                clips: Some(vec!["c1".into()]),
                change: BubbleChange {
                    position: Some(Anchor::TopLeft),
                    ..BubbleChange::default()
                },
            },
            true,
        ),
        (ToolCall::DetachCamera { clips: None }, false),
        (
            ToolCall::MuteSource {
                source: Sound::Mic,
                muted: true,
                which: None,
                clips: Some(vec!["c3".into()]),
            },
            true,
        ),
        (
            ToolCall::SetSourceVolume {
                source: Sound::ComputerSound,
                volume: 0.5,
                which: None,
                clips: None,
            },
            false,
        ),
        (
            ToolCall::SeparateSound {
                source: Sound::Mic,
                which: Some("2".into()),
                clips: None,
            },
            false,
        ),
        (ToolCall::Undo, false),
        (
            ToolCall::AskUser {
                question: "Which part?".into(),
            },
            false,
        ),
        (
            ToolCall::CannotDo {
                reason: "No tool fits.".into(),
            },
            false,
        ),
    ]
}

#[test]
fn names_timeline_for_every_tool_with_times_or_refs() {
    let calls = every_call();
    // Every tool the model is offered is here.
    let listed: BTreeSet<String> = (tools().as_array().unwrap().iter())
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    let covered: BTreeSet<String> = calls.iter().map(|(c, _)| c.name().to_owned()).collect();
    assert_eq!(covered, listed);
    for (call, names) in &calls {
        assert_eq!(call.names_timeline(), *names, "{call:?}");
    }
}
