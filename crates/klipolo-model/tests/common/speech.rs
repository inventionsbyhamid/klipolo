//! Projects with one voice file `v` and a synthetic transcript, for the caption tests.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_model::apply_ops;
use klipolo_model::contrib::ContribId;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::secs_to_us as secs;
use klipolo_model::timeline::{Clip, ClipSource};

pub fn w(text: &str, start: f64, end: f64) -> Word {
    Word {
        text: text.into(),
        start_us: secs(start),
        end_us: secs(end),
        confidence: 0.9,
        kind: WordKind::Word,
    }
}

pub fn pause(start: f64, end: f64) -> Word {
    Word {
        kind: WordKind::Pause,
        ..w("", start, end)
    }
}

fn video() -> Asset {
    Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "/media/v".into(),
        name: "v".into(),
        duration_us: secs(60.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }
}

fn speech(words: Vec<Word>) -> Speech {
    let t = Transcript {
        key: "k".into(),
        language: "en".into(),
        model: "m".into(),
        covered_us: secs(60.0),
        complete: true,
        words,
    };
    Speech {
        transcripts: HashMap::from([("v".to_owned(), Arc::new(t))]),
        ..Speech::default()
    }
}

/// `p` with captions on in the built-in `style` (`caption.bold`), checked.
fn captioned(mut p: Project, style: &str) -> Project {
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin(style));
    apply_ops(&p, &[]).unwrap()
}

/// One 60 s clip of voice file `v`, captions on in `style` and every format, and a transcript of `words` (text, start
/// s, end s).
pub fn project_with_words(style: &str, words: &[(&str, f64, f64)]) -> (Project, Speech) {
    let mut p = Project::new("Pages");
    p.settings.formats = vec![
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    p.assets = vec![video()];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "v".into() },
        0,
        secs(60.0),
    )];
    let words = words.iter().map(|&(t, s, e)| w(t, s, e)).collect();
    (captioned(p, style), speech(words))
}

/// A sentence's words from `start` s, 0.3 s apart, each 0.25 s long.
pub fn words_at(start: f64, sentence: &str) -> Vec<(&str, f64, f64)> {
    sentence
        .split_whitespace()
        .enumerate()
        .map(|(i, t)| {
            let at = start + i as f64 * 0.3;
            (t, at, at + 0.25)
        })
        .collect()
}

/// Voice file `v` (60 s) as c1 (0–20 s) and c2 (30–60 s), captions on in `style`, landscape and portrait.
pub fn two_clips(style: &str, words: Vec<Word>) -> (Project, Speech) {
    clips_from(style, words, 30.0, 1.0)
}

/// `two_clips` with c2 playing the file from `second_from` s (20: a split left in place) at `speed`.
pub fn clips_from(
    style: &str,
    words: Vec<Word>,
    second_from: f64,
    speed: f64,
) -> (Project, Speech) {
    let mut p = Project::new("Captions");
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p.assets = vec![video()];
    let mut c1 = Clip::new(ClipSource::Asset { id: "v".into() }, 0, secs(20.0));
    c1.id = "c1".into();
    let mut c2 = Clip::new(
        ClipSource::Asset { id: "v".into() },
        secs(second_from),
        secs(60.0),
    );
    c2.id = "c2".into();
    c2.speed = speed;
    p.timeline.main = vec![c1, c2];
    (captioned(p, style), speech(words))
}
