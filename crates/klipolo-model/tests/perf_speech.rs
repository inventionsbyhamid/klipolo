//! Speech mapping at real scale: a 2-hour recording's 18,000 words over 400 clips, with word fixes, and its caption
//! pages. `cargo test -p klipolo-model --release --test perf_speech -- --ignored --nocapture`

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::secs;
use klipolo_model::apply_ops;
use klipolo_model::captions::looks::{Look, WordLook};
use klipolo_model::captions::{captions, captions_in, Captions};
use klipolo_model::contrib::ContribId;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::{spoken, Speech, Transcript, Word, WordFix, WordKind};
use klipolo_model::timeline::{Clip, ClipSource};

pub fn two_hours() -> (Project, Speech) {
    let mut p = Project::new("Two hours");
    p.assets = vec![Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "/media/v".into(),
        name: "v".into(),
        duration_us: secs(7_200.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }];
    // 400 clips: every 18 s keeps 15 s, as after silence removal.
    p.timeline.main = (0..400)
        .map(|i| {
            let mut c = Clip::new(
                ClipSource::Asset { id: "v".into() },
                secs(i as f64 * 18.0),
                secs(i as f64 * 18.0 + 15.0),
            );
            c.id = format!("c{i}");
            c
        })
        .collect();
    // Fixes as a person makes them: one word in 60, and one long fix over 1,000–1,500 s that the
    // cuts put in 28 pieces (its uses are checked per piece).
    let fix = |from: f64, to: f64, text: String| WordFix {
        asset_id: "v".into(),
        start_us: secs(from),
        end_us: secs(to),
        text,
    };
    let mut fixes: Vec<WordFix> = (0..18_000)
        .step_by(60)
        .filter(|i| !(2_490..3_760).contains(i))
        .map(|i| fix(i as f64 * 0.4, i as f64 * 0.4 + 0.3, format!("fixed{i}")))
        .chain([fix(1_000.0, 1_500.0, "A long fix".into())])
        .collect();
    fixes.sort_by_key(|f| f.start_us);
    p.word_fixes = fixes.into();
    let words = (0..18_000)
        .map(|i| Word {
            text: format!("w{i}"),
            start_us: secs(i as f64 * 0.4),
            end_us: secs(i as f64 * 0.4 + 0.3),
            confidence: 0.9,
            kind: if i % 30 == 0 {
                WordKind::Filler
            } else {
                WordKind::Word
            },
        })
        .collect();
    let t = Transcript {
        key: "k".into(),
        language: "en".into(),
        model: "m".into(),
        covered_us: secs(7_200.0),
        complete: true,
        words,
    };
    (
        apply_ops(&p, &[]).unwrap(),
        Speech {
            transcripts: HashMap::from([("v".to_owned(), Arc::new(t))]),
            ..Speech::default()
        },
    )
}

#[test]
#[ignore]
fn two_hours_of_words_map_onto_400_clips_in_under_5_ms() {
    let (p, speech) = two_hours();
    let runs = 20;
    let started = Instant::now();
    let mut words = 0;
    for _ in 0..runs {
        words = spoken(&p, &speech).words.len();
    }
    let each = started.elapsed() / runs;
    println!("spoken: {words} words over 400 clips in {each:?} per call");
    assert!(each.as_micros() < 5_000, "{each:?}");
}

/// `two_hours` at the caps a person can reach: 20,000 word fixes (most words fixed, so most read a fix's
/// text) and 2,000 looks, every ninth word's: colours, sizes, emphasis both ways, hidden words and line breaks.
fn two_hours_at_the_caps() -> (Project, Speech) {
    let (mut p, speech) = two_hours();
    let fixes: Vec<WordFix> = (0..20_000)
        .map(|i| WordFix {
            asset_id: "v".into(),
            start_us: secs(i as f64 * 0.36),
            end_us: secs(i as f64 * 0.36 + 0.3),
            // Every tenth ends a sentence.
            text: if i % 10 == 9 {
                format!("fix{i}.")
            } else {
                format!("fix{i}")
            },
        })
        .collect();
    p.word_fixes = fixes.into();
    let looks: Vec<WordLook> = (0..2_000)
        .map(|i| {
            let word = i as f64 * 9.0 * 0.4;
            let look = match i % 5 {
                0 => Look {
                    color: Some("#39E508".into()),
                    ..Look::default()
                },
                1 => Look {
                    size: Some(2),
                    ..Look::default()
                },
                2 => Look {
                    emphasis: Some(i % 2 == 0),
                    ..Look::default()
                },
                3 => Look {
                    hidden: Some(true),
                    ..Look::default()
                },
                _ => Look {
                    line_break: Some(true),
                    ..Look::default()
                },
            };
            WordLook {
                asset_id: "v".into(),
                start_us: secs(word),
                end_us: secs(word + 0.3),
                look,
            }
        })
        .collect();
    p.word_looks = looks.into();
    p.captions.enabled = true;
    p.settings.formats = vec![
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    (apply_ops(&p, &[]).unwrap(), speech)
}

/// Pages for three formats from two hours of words at the caps, in Bold (keywords, upper case), Karaoke and Clean
/// (reading speed): each style's fastest of 5 warm runs (a shared host only adds time) within 8 ms.
/// It also prints the previewed format's pages alone, what a dry run that moves words makes per drag frame, and prints
/// every figure before it asserts.
#[test]
#[ignore]
fn perf_pages() {
    let (mut p, speech) = two_hours_at_the_caps();
    let fastest = |make: &dyn Fn() -> Captions| {
        (0..5)
            .map(|_| {
                let started = Instant::now();
                std::hint::black_box(make());
                started.elapsed()
            })
            .min()
            .unwrap()
    };
    let mut over = Vec::new();
    for style in ["caption.bold", "caption.karaoke", "caption.clean"] {
        p.captions.style = Some(ContribId::builtin(style));
        let started = Instant::now();
        let c = captions(&p, &speech);
        let first = started.elapsed();
        let pages: usize = c.pages.values().map(|f| f.pages.len()).sum();
        let three = fastest(&|| captions(&p, &speech));
        let one = fastest(&|| captions_in(&p, &speech, &[FormatKind::Portrait]));
        println!(
            "{style}: {pages} pages in 3 formats in {three:?}; the previewed format alone (a dry run) {one:?} (fastest \
             of 5 warm runs; first {first:?})"
        );
        if three > Duration::from_millis(8) {
            over.push(format!("{style}: {three:?}"));
        }
    }
    assert!(over.is_empty(), "over 8 ms: {over:?}");
}
