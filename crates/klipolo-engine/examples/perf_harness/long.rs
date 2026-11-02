//! `… -- long [<video>] [--clips <n>] [--words <n>] [--runs <n>] [--write <file>]`: a 2-hour talk's project at full
//! size, on the CPU only (nothing is decoded or drawn). `--clips` (600) main clips cut from one 2-hour file, Word pop
//! captions over `--words` (18,000) words, 200 layers (100 titles that enter and leave, 100 images) and 4
//! picture-in-picture video layers, each cropped and moving on motion keys. `<video>` is the 2-hour file: only its path
//! goes into the project.
//!
//! Each figure is the median of `--runs` (5), in ms: the project's save (its JSON and size) and open (parse and
//! check); each edit through the document (split, ripple delete, keep only 50 ranges, roll, slip, volume keys, motion
//! keys, undo and redo), then what the engine makes again for the revision (the caption pages when they may differ,
//! where speech ducks, the plan context, the sound plan and the media it wants); a layer drag's dry run (one apply and
//! plan context per mouse move); and one frame's plan. Anything over 50 ms is marked "!" (a finding).
//!
//! `--write <file>` also writes the project there (its video at `<video>`), and its words beside it
//! (`<file>.words.json`), for opening in the app.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use klipolo_engine::audio::plan::{audio_plan, ducking_speech};
use klipolo_engine::media;
use klipolo_engine::plan::captions::pages_may_differ;
use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_model::captions::captions;
use klipolo_model::contrib::ContribId;
use klipolo_model::crop::Crop;
use klipolo_model::history::Document;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::load::load_project;
use klipolo_model::motion::{Key, Motion, MotionKeys};
use klipolo_model::ops::{ClipPatch, ItemPatch, Op, Transaction};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, TextStyle, Track, TrackKind};

use super::measure::allocs;
use super::project::{preset, secs};

const HOURS_S: f64 = 7_200.0;
/// Main clips unless `--clips` says otherwise.
const CLIPS: usize = 600;
/// An over-50 ms step is a finding.
const FINDING_MS: f64 = 50.0;

/// The words of a 2-hour talk: `n` spread over the file, sentences of six to twelve words with a pause between them.
pub fn talk_words(n: usize) -> Vec<Word> {
    const SAID: [&str; 12] = [
        "so", "today", "we", "build", "a", "faster", "editor", "that", "keeps", "every", "frame",
        "smooth",
    ];
    let per = HOURS_S / n as f64;
    (0..n)
        .map(|i| {
            let at = i as f64 * per;
            let last = i % 9 == 8;
            let mut text = SAID[i % SAID.len()].to_owned();
            if last {
                // Each sentence ends with its number, so no two pages are alike.
                text = format!("{text}{}.", i / 9);
            }
            Word {
                text,
                start_us: secs(at),
                end_us: secs(at + per * if last { 0.5 } else { 0.85 }),
                confidence: 0.9,
                kind: WordKind::Word,
            }
        })
        .collect()
}

fn item(id: String, start: f64, duration: f64, rect: BoxN, content: ItemContent) -> Item {
    Item {
        id,
        start_us: secs(start),
        duration_us: secs(duration),
        attach: None,
        layout: Layout::new(Anchor::TopLeft, rect),
        content,
        effects: Vec::new(),
    }
}

fn keys(values: &[(f64, f64)]) -> Vec<Key> {
    values
        .iter()
        .map(|&(t, v)| Key {
            t_us: secs(t),
            v,
            ease: None,
        })
        .collect()
}

/// The 2-hour project and its speech (`video` the file's path).
pub fn two_hour_project(video: &str, clips: usize, words: usize) -> (Project, Speech) {
    let mut p = Project::new("Two hours");
    p.id = "long".into();
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p.assets = vec![Asset {
        id: "talk".into(),
        kind: AssetKind::Video,
        path: video.into(),
        name: "talk".into(),
        duration_us: secs(HOURS_S),
        width: 1920,
        height: 1080,
        fps: 30.0,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }];
    for k in 0..10 {
        p.assets.push(Asset {
            id: format!("logo-{k}"),
            kind: AssetKind::Image,
            path: format!("{video}.logo-{k}.png"),
            name: format!("logo-{k}"),
            duration_us: 0,
            width: 800,
            height: 600,
            fps: 0.0,
            has_audio: false,
            audio_lead_us: None,
            made_with: None,
            rotation: None,
        });
    }
    // Each clip its stretch of the talk but its last half second (600: 11.5 s of each 12 s).
    let stretch = HOURS_S / clips as f64;
    p.timeline.main = (0..clips)
        .map(|k| {
            let from = k as f64 * stretch;
            let mut c = Clip::new(
                ClipSource::Asset { id: "talk".into() },
                secs(from),
                secs(from + stretch - 0.5),
            );
            c.id = format!("clip-{k}");
            c
        })
        .collect();
    let main_s = clips as f64 * (stretch - 0.5);
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    let mut images = Track::new(TrackKind::Overlay, "Images");
    for j in 0..100 {
        let t = main_s * j as f64 / 100.0;
        let mut title = item(
            format!("title-{j}"),
            t + 2.0,
            4.0,
            BoxN {
                x: 0.1,
                y: 0.05,
                w: 0.8,
                h: 0.2,
            },
            ItemContent::Text {
                text: format!("Part {j}: what we build next"),
                style: TextStyle {
                    size: 48.0 + (j % 6) as f64 * 8.0,
                    ..TextStyle::default()
                },
            },
        );
        title.layout.motion = Some(Motion {
            enter: Some(preset("fade")),
            exit: Some(preset("slide")),
            ..Motion::default()
        });
        titles.items.push(title);
        images.items.push(item(
            format!("image-{j}"),
            t + 20.0,
            5.0,
            BoxN {
                x: 0.72,
                y: 0.66,
                w: 0.24,
                h: 0.3,
            },
            ItemContent::Image {
                asset_id: format!("logo-{}", j % 10),
            },
        ));
    }
    // Four picture-in-picture layers over the whole talk, one per corner, each from its own stretch of the file.
    let mut tracks = vec![titles, images];
    for v in 0..4 {
        let mut track = Track::new(TrackKind::Overlay, format!("Video {v}"));
        let (x, y) = [(0.02, 0.02), (0.74, 0.02), (0.02, 0.72), (0.74, 0.72)][v];
        let mut layer = item(
            format!("pip-{v}"),
            0.0,
            main_s - 1.0,
            BoxN {
                x,
                y,
                w: 0.24,
                h: 0.26,
            },
            ItemContent::Video {
                asset_id: "talk".into(),
                source_in_us: secs(60.0 * v as f64),
                speed: 1.0,
                volume: 1.0,
                muted: true,
                mirror: false,
                fill: false,
                crop: Some(Crop {
                    left: 0.1,
                    top: 0.05,
                    right: 0.1,
                    bottom: 0.05,
                }),
                tie: None,
            },
        );
        // A slow drift and a breath of scale every 2 minutes, over the layer's whole length (58 keys of 64).
        let steps = 0..=((main_s - 1.0) / 120.0) as usize;
        let swing = |a: f64, b: f64| -> Vec<(f64, f64)> {
            steps
                .clone()
                .map(|k| (k as f64 * 120.0, if k.is_multiple_of(2) { a } else { b }))
                .collect()
        };
        layer.layout.motion = Some(Motion {
            keys: Some(MotionKeys {
                x: keys(&swing(0.0, 0.01)),
                scale: keys(&swing(1.0, 1.05)),
                ..MotionKeys::default()
            }),
            ..Motion::default()
        });
        track.items.push(layer);
        tracks.push(track);
    }
    p.timeline.tracks = tracks;
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin("caption.word-pop"));
    let p = klipolo_model::apply_ops(&p, &[]).expect("a checked project");
    let transcript = Transcript {
        key: "talk".into(),
        language: "en".into(),
        model: "m".into(),
        covered_us: secs(HOURS_S),
        complete: true,
        words: talk_words(words),
    };
    let speech = Speech {
        transcripts: HashMap::from([("talk".to_owned(), Arc::new(transcript))]),
        ..Speech::default()
    };
    (p, speech)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn flag(ms: f64) -> &'static str {
    if ms > FINDING_MS { " !" } else { "" }
}

/// Times `f` `runs` times: the median ms and the last result.
fn timed<T>(runs: usize, mut f: impl FnMut() -> T) -> (f64, T) {
    let mut ms = Vec::new();
    let mut last = None;
    for _ in 0..runs {
        let started = Instant::now();
        let out = f();
        ms.push(started.elapsed().as_secs_f64() * 1e3);
        last = Some(out);
    }
    (median(ms), last.expect("one run"))
}

/// What the engine makes again for a revision, as its feed does: (pages, ducking, context, sound, wants) ms.
struct Engine {
    ctx: PlanContext,
    project: Arc<Project>,
    pages: Arc<klipolo_model::captions::Captions>,
}

impl Engine {
    fn new(project: Arc<Project>, speech: &Speech) -> Self {
        let pages = Arc::new(captions(&project, speech));
        let ctx = PlanContext::new(project.clone()).with_captions(pages.clone());
        Self {
            ctx,
            project,
            pages,
        }
    }

    fn revision(&mut self, next: Arc<Project>, speech: &Speech) -> [f64; 5] {
        let t = Instant::now();
        if pages_may_differ(&self.project, &next) {
            self.pages = Arc::new(captions(&next, speech));
        }
        let pages_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let ducking = ducking_speech(&next, speech);
        let ducking_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let (ctx, build) = PlanContext::starting(next.clone(), Some(&self.ctx), &self.ctx.cursors);
        let mut ctx = ctx.with_captions(self.pages.clone());
        if let Some(build) = build {
            ctx.with_zoom(build.finish());
        }
        let ctx_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        std::hint::black_box(audio_plan(&next, &ctx.registry, ducking));
        let sound_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        std::hint::black_box(media::wants(&next, 0));
        let wants_ms = t.elapsed().as_secs_f64() * 1e3;
        self.ctx = ctx;
        self.project = next;
        [pages_ms, ducking_ms, ctx_ms, sound_ms, wants_ms]
    }
}

fn tx(label: &str, ops: Vec<Op>) -> Transaction {
    Transaction {
        label: label.into(),
        ops,
        ..Transaction::default()
    }
}

/// The 51 stretches between 50 kept ranges spread over `main_us`: keep only those 50.
fn keep_only_50(main_us: Us) -> Vec<RangeUs> {
    let step = main_us / 50;
    let mut cut = Vec::new();
    let mut at = 0;
    for k in 0..50 {
        let keep_from = k * step + step / 3;
        let keep_to = keep_from + step / 4;
        cut.push(RangeUs {
            start: at,
            end: keep_from,
        });
        at = keep_to;
    }
    cut.push(RangeUs {
        start: at,
        end: main_us,
    });
    cut
}

pub fn run(args: &[String]) {
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let video = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "/media/talk-1080p-2h.mp4".into());
    let words: usize = value("--words")
        .and_then(|s| s.parse().ok())
        .unwrap_or(18_000);
    let runs: usize = value("--runs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5)
        .max(1);

    let started = Instant::now();
    let clips: usize = value("--clips")
        .and_then(|s| s.parse().ok())
        .unwrap_or(CLIPS)
        .max(400);
    let (project, speech) = two_hour_project(&video, clips, words);
    let layers: usize = project.timeline.tracks.iter().map(|t| t.items.len()).sum();
    println!(
        "2-hour project: {} clips, {layers} layers, {words} words (built in {:.0} ms); median of {runs} runs; \
         \"!\" over {FINDING_MS} ms",
        project.timeline.main.len(),
        started.elapsed().as_secs_f64() * 1e3,
    );

    // Save and open.
    let (save_ms, json) = timed(runs, || serde_json::to_vec(&project).expect("JSON"));
    println!(
        "save: {save_ms:.1} ms{} ({:.2} MB of JSON)",
        flag(save_ms),
        json.len() as f64 / 1e6
    );
    let text = String::from_utf8(json.clone()).expect("UTF-8");
    let (open_ms, opened) = timed(runs, || load_project(&text).expect("opens"));
    assert_eq!(opened, project, "opens as saved");
    let (doc_ms, _) = timed(runs, || Document::new(project.clone()).expect("a document"));
    println!(
        "open: parse and check {open_ms:.1} ms{} | history {doc_ms:.1} ms{}",
        flag(open_ms),
        flag(doc_ms)
    );
    if let Some(file) = value("--write") {
        std::fs::write(&file, &json).expect("writes the project");
        let words = serde_json::to_vec(&speech.transcripts["talk"]).expect("JSON");
        std::fs::write(format!("{file}.words.json"), words).expect("writes the words");
        println!("wrote {file} and {file}.words.json");
    }

    // Engine state for the opened revision.
    let shared = Arc::new(project.clone());
    let (pages_ms, pages) = timed(runs, || captions(&shared, &speech));
    let page_count: usize = pages.pages.values().map(|f| f.pages.len()).sum();
    let (ctx_ms, ctx) = timed(runs, || PlanContext::new(shared.clone()));
    let ctx = ctx.with_captions(Arc::new(pages));
    println!(
        "first revision: caption pages {pages_ms:.1} ms{} ({page_count} pages over {} formats) | plan context \
         {ctx_ms:.1} ms{}",
        flag(pages_ms),
        project.settings.formats.len(),
        flag(ctx_ms)
    );
    let (a0, _) = allocs();
    let frames = 2_000;
    let started = Instant::now();
    let main_us = ctx.main_duration_us;
    for k in 0..frames {
        let t = main_us / frames as i64 * k as i64;
        std::hint::black_box(frame_plan(&ctx, t, FormatKind::Landscape));
    }
    println!(
        "frame plan: {:.1} µs, {:.1} allocs each (2,000 frames over the video)",
        started.elapsed().as_secs_f64() * 1e6 / frames as f64,
        (allocs().0 - a0) as f64 / frames as f64
    );
    drop(ctx);

    // Edits through the document, each followed by the engine's revision.
    let mid = main_us / 2;
    let clip = |k: usize| project.timeline.main[k].id.clone();
    // A dip over the middle half of clip 250, whatever its length.
    let dip = {
        let c = &project.timeline.main[250];
        let len = (c.source_out_us - c.source_in_us) as f64 / 1e6;
        keys(&[
            (0.0, 1.0),
            (len * 0.25, 0.3),
            (len * 0.65, 0.3),
            (len * 0.9, 1.0),
        ])
    };
    let mut motion = project.timeline.tracks[2].items[0].layout.clone();
    if let Some(keys) = motion.motion.as_mut().and_then(|m| m.keys.as_mut()) {
        keys.opacity = (0..20)
            .map(|k| Key {
                t_us: secs(k as f64 * 3.0),
                v: if k % 2 == 0 { 1.0 } else { 0.6 },
                ease: None,
            })
            .collect();
    }
    let edits: Vec<(&str, Transaction)> = vec![
        (
            "split",
            tx(
                "Split",
                vec![Op::Split {
                    at_us: mid + secs(5.0),
                }],
            ),
        ),
        (
            "ripple delete",
            tx("Delete", vec![Op::RemoveClip { id: clip(300) }]),
        ),
        (
            "keep only 50 ranges",
            tx(
                "Keep",
                vec![Op::DeleteRanges {
                    ranges: keep_only_50(main_us),
                }],
            ),
        ),
        (
            "roll",
            tx(
                "Roll",
                vec![Op::Roll {
                    clip_id: clip(200),
                    by_us: secs(0.2),
                }],
            ),
        ),
        (
            "slip",
            tx(
                "Slip",
                vec![Op::Slip {
                    clip_id: clip(201),
                    by_us: secs(0.2),
                }],
            ),
        ),
        (
            "volume keys",
            tx(
                "Volume",
                vec![Op::UpdateClip {
                    id: clip(250),
                    patch: ClipPatch {
                        volume_keys: Some(dip),
                        ..ClipPatch::default()
                    },
                }],
            ),
        ),
        (
            "motion keys",
            tx(
                "Motion",
                vec![Op::UpdateItem {
                    id: "pip-0".into(),
                    patch: ItemPatch {
                        layout: Some(motion),
                        ..ItemPatch::default()
                    },
                }],
            ),
        ),
    ];
    println!(
        "  {:<20} {:>9} {:>9} | engine: {:>7} {:>7} {:>7} {:>7} {:>7} {:>8}",
        "edit", "apply", "undo+redo", "pages", "duck", "context", "sound", "wants", "total"
    );
    for (name, t) in &edits {
        let mut apply = Vec::new();
        let mut back = Vec::new();
        let mut engine_ms: Vec<[f64; 5]> = Vec::new();
        for _ in 0..runs {
            let mut doc = Document::new(project.clone()).expect("a document");
            let mut engine = Engine::new(doc.shared(), &speech);
            let started = Instant::now();
            doc.apply_change(t).expect("applies");
            apply.push(started.elapsed().as_secs_f64() * 1e3);
            engine_ms.push(engine.revision(doc.shared(), &speech));
            let started = Instant::now();
            doc.undo_change().expect("undoes");
            doc.redo_change().expect("redoes");
            back.push(started.elapsed().as_secs_f64() * 1e3 / 2.0);
        }
        let parts: Vec<f64> = (0..5)
            .map(|i| median(engine_ms.iter().map(|e| e[i]).collect()))
            .collect();
        let total = median(engine_ms.iter().map(|e| e.iter().sum()).collect());
        let (apply, back) = (median(apply), median(back));
        println!(
            "  {name:<20} {apply:>7.1}{:<2} {back:>7.1}{:<2} | {:>15.1} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {total:>6.1}{}",
            flag(apply),
            flag(back),
            parts[0],
            parts[1],
            parts[2],
            parts[3],
            parts[4],
            flag(total)
        );
    }

    // A title dragged along the timeline: each mouse move applies the move to the committed revision and plans it.
    let base = Arc::new(project.clone());
    let base_ctx = PlanContext::new(base.clone());
    let mut drag = Vec::new();
    for k in 0..(runs * 6) {
        let ops = [Op::MoveItem {
            id: "title-50".into(),
            track_id: None,
            start_us: base.timeline.tracks[0].items[50].start_us + secs(0.1 * k as f64),
        }];
        let started = Instant::now();
        let moved = Arc::new(klipolo_model::apply_ops(&base, &ops).expect("moves"));
        let (ctx, _) = PlanContext::starting(moved, Some(&base_ctx), &base_ctx.cursors);
        std::hint::black_box(ctx);
        drag.push(started.elapsed().as_secs_f64() * 1e3);
    }
    let drag = median(drag);
    println!(
        "drag a title: {drag:.1} ms a mouse move{} (apply and plan context)",
        flag(drag)
    );
}
