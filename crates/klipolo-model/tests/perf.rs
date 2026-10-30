//! Edit timings on 1- and 2-hour projects. Not part of the default suite, and timed one test at a time (run together,
//! they slow each other down by half):
//! `cargo test -p klipolo-model --release --test perf -- --ignored --nocapture --test-threads=1`

mod common;
use std::time::{Duration, Instant};

use common::*;
use klipolo_model::history::{ByApp, Change, Document, Share, Whose};
use klipolo_model::ops::*;
use klipolo_model::project::*;
use klipolo_model::query::anchor_at;
use klipolo_model::timeline::*;
use klipolo_model::{apply_ops, validate, Project, RangeUs};

const CLIPS: usize = 1_500;

fn media(id: &str, kind: AssetKind, has_audio: bool) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs(3_600.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        has_audio,
        audio_lead_us: None,
        rotation: None,
    }
}

/// Recording "rec": one 1-hour run per source, each file named as its source.
fn one_hour_take(sources: &[(&str, SourceRole)]) -> Recording {
    Recording {
        id: "rec".into(),
        kind: RecordingKind::Screen,
        sources: sources
            .iter()
            .map(|&(id, role)| rec_source(id, role, id, 0, 3_600.0))
            .collect(),
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }
}

/// A 1-hour recording with a separated camera layer, as one clip "a".
fn one_hour_recording() -> Project {
    let mut p = Project::new("Perf");
    p.assets = vec![
        media("screen", AssetKind::Video, false),
        media("camera", AssetKind::Video, false),
        media("mic", AssetKind::Audio, true),
        media("logo", AssetKind::Image, false),
    ];
    p.recordings = vec![one_hour_take(&[
        ("screen", SourceRole::Screen),
        ("camera", SourceRole::Camera),
        ("mic", SourceRole::Mic),
    ])];
    let mut a = Clip::new(ClipSource::Recording { id: "rec".into() }, 0, secs(3_600.0));
    a.id = "a".into();
    a.camera = Some(CameraLayout::default());
    p.timeline.main = vec![a];
    apply_ops(
        &p,
        &[Op::DetachSource {
            source_id: "camera".into(),
            clip_id: "a".into(),
        }],
    )
    .unwrap()
}

/// Silence removal: cut 0.2 s after every 2.2 s of speech.
fn silences() -> Vec<RangeUs> {
    (0..CLIPS)
        .map(|k| {
            let start = secs(2.4 * k as f64 + 2.2);
            RangeUs {
                start,
                end: start + secs(0.2),
            }
        })
        .collect()
}

/// An hour-long project: 1,500 clips cut from one recording (ids `a`, `a-2`, …) and 1,800
/// attached layers (600 titles, 600 images, 600 zooms).
fn one_hour_project() -> Project {
    let mut p = Project::new("Perf");
    p.assets = vec![
        media("screen", AssetKind::Video, false),
        media("mic", AssetKind::Audio, true),
        media("logo", AssetKind::Image, false),
    ];
    p.recordings = vec![one_hour_take(&[
        ("screen", SourceRole::Screen),
        ("mic", SourceRole::Mic),
    ])];
    p.timeline.main = (0..CLIPS)
        .map(|k| {
            let start = secs(2.4 * k as f64);
            let mut c = Clip::new(
                ClipSource::Recording { id: "rec".into() },
                start,
                start + secs(2.2),
            );
            c.id = if k == 0 {
                "a".into()
            } else {
                format!("a-{}", k + 1)
            };
            c
        })
        .collect();
    let layer = |id: String, at: f64, content: ItemContent| {
        // A title fades in and out; other layers keep still (an audio or zoom layer can't move).
        let mut item = match content {
            ItemContent::Text { .. } => text_item(&id, at, 1.0),
            _ => still_item(&id, at, 1.0),
        };
        item.content = content;
        item.attach = anchor_at(&p.timeline.main, item.start_us);
        item
    };
    let mut tracks = vec![
        Track::new(TrackKind::Overlay, "Overlay 1"),
        Track::new(TrackKind::Overlay, "Overlay 2"),
        Track::new(TrackKind::Zoom, "Zoom"),
    ];
    for j in 0..600 {
        let t = 5.5 * j as f64;
        let title = text_item("x", 0.0, 1.0).content;
        tracks[0].items.push(layer(format!("title-{j}"), t, title));
        let image = ItemContent::Image {
            asset_id: "logo".into(),
        };
        tracks[1]
            .items
            .push(layer(format!("image-{j}"), t + 2.0, image));
        let zoom = ItemContent::Zoom {
            target: ZoomTarget::Auto { using: vec![] },
            scale: 1.5,
        };
        tracks[2]
            .items
            .push(layer(format!("zoom-{j}"), t + 3.5, zoom));
    }
    p.timeline.tracks = tracks;
    validate(&p).unwrap();
    p
}

/// Fastest of a few runs of `ops` on `p`.
fn time(p: &Project, ops: &[Op]) -> Duration {
    (0..5)
        .map(|_| {
            let started = Instant::now();
            apply_ops(p, ops).unwrap();
            started.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
#[ignore = "timing; run in release with --ignored --nocapture"]
fn one_hour_project_edit_timings() {
    let p = one_hour_project();
    let mid = secs(2.2 * 750.0 + 1.0);
    let timings = [
        ("Split mid-timeline", vec![Op::Split { at_us: mid }]),
        ("Split clip `a`", vec![Op::Split { at_us: secs(1.0) }]),
        (
            "MoveItem",
            vec![Op::MoveItem {
                id: "title-300".into(),
                track_id: None,
                start_us: secs(1_000.0),
            }],
        ),
        (
            "DeleteRanges (1 range)",
            vec![Op::DeleteRanges {
                ranges: vec![RangeUs {
                    start: mid,
                    end: mid + secs(0.5),
                }],
            }],
        ),
    ];
    for (name, ops) in &timings {
        println!("{name}: {:?}", time(&p, ops));
    }
    let recording = one_hour_recording();
    let cut = [Op::DeleteRanges { ranges: silences() }];
    println!(
        "Silence removal (1 clip + camera -> 1,500 pieces each): {:?}",
        time(&recording, &cut)
    );
}

/// A pause removal on a 2-hour recording with 300 layers (titles attached to the clip, so they ripple,
/// beside the separated camera's hour).
fn two_hours_with_300_layers() -> Project {
    let mut p = one_hour_recording();
    for a in &mut p.assets {
        a.duration_us = secs(7_200.0);
    }
    // The runs too: the clip can't play past its recording.
    for run in p.recordings[0].sources.iter_mut().flat_map(|s| &mut s.runs) {
        run.duration_us = secs(7_200.0);
    }
    p.timeline.main[0].source_out_us = secs(7_200.0);
    let items = (0..300)
        .map(|i| {
            let mut item = text_item(&format!("t{i}"), i as f64 * 24.0, 3.0);
            item.attach = anchor_at(&p.timeline.main, item.start_us);
            item
        })
        .collect();
    p.timeline.tracks.push(overlay_track("texts", items));
    apply_ops(&p, &[]).unwrap()
}

#[test]
#[ignore = "timing; run in release with --ignored --nocapture"]
fn five_thousand_ranges_ripple_two_hours_in_under_15_ms() {
    let p = two_hours_with_300_layers();
    let ranges: Vec<RangeUs> = (0..5_000)
        .map(|i| RangeUs {
            start: secs(i as f64 * 1.44),
            end: secs(i as f64 * 1.44 + 0.4),
        })
        .collect();
    let tx = vec![Op::DeleteRanges { ranges }];
    // The first run also pays for this process's first touch of the memory a cut fills (5,000 clips of 808 B), which
    // the app's warm allocator doesn't; on a shared machine it varies by several ms. The budget is the median of the
    // runs after it, each timed before its result is dropped; the first run has a loose ceiling, so a cold regression
    // is still caught.
    let started = Instant::now();
    let after = apply_ops(&p, &tx).unwrap();
    let first = started.elapsed();
    let mut warm: Vec<Duration> = (0..5)
        .map(|_| {
            let started = Instant::now();
            let next = apply_ops(&p, &tx).unwrap();
            let took = started.elapsed();
            drop(next);
            took
        })
        .collect();
    warm.sort();
    let median = warm[warm.len() / 2];
    let layers: usize = after.timeline.tracks.iter().map(|t| t.items.len()).sum();
    println!(
        "DeleteRanges × 5,000 on a 2-hour clip with 300 layers: {median:?} (median of 5; fastest {:?}, first run {first:?}); {} clips, {layers} layers after",
        warm[0],
        after.timeline.main.len()
    );
    assert!(after.timeline.main.len() > 1);
    assert!(median < Duration::from_millis(15), "{median:?}");
    assert!(first <= Duration::from_millis(40), "first run {first:?}");
}

#[test]
#[ignore = "measurement; run in release with --ignored --nocapture"]
fn two_hundred_layer_moves_fit_the_history_budget() {
    let mut doc = Document::new(one_hour_project()).unwrap();
    let started = Instant::now();
    for j in 0..200 {
        doc.apply(&Transaction {
            label: "Move layer".into(),
            ops: vec![Op::MoveItem {
                id: format!("title-{j}"),
                track_id: None,
                start_us: secs(5.5 * j as f64 + 1.0),
            }],
            origin: None,
            ..Default::default()
        })
        .unwrap();
    }
    let bytes = doc.history_bytes();
    println!(
        "200 layer moves: {} steps, {bytes} history bytes, {:?} per edit",
        doc.undo_steps(),
        started.elapsed() / 200
    );
    assert_eq!(doc.undo_steps(), 200);
    assert!(bytes < 8 << 20, "{bytes}");
}

/// The model's share of the op round trip (budget: p95 under 30 ms) on the 1-hour project: the change, its JSON and
/// the clone the engine feed takes, for 50 layer moves, then 50 undos and 50 redos (each rebuilds the whole project),
/// against the whole snapshot's JSON the UI used to get. The app's `op_round_trip_on_an_hour` times the same through
/// the command; this one builds in release without the app's dependencies.
#[test]
#[ignore = "timing; run in release with --ignored --nocapture"]
fn patch_round_trip_on_an_hour() {
    let mut doc = Document::new(one_hour_project()).unwrap();
    let percentiles = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[v.len() / 2], v[v.len() * 95 / 100])
    };
    let mut timed = |f: &mut dyn FnMut(&mut Document) -> Vec<u8>| {
        let (mut ms, mut bytes) = (Vec::new(), 0);
        for _ in 0..50 {
            let started = Instant::now();
            let json = f(&mut doc);
            let feed = doc.project().clone();
            ms.push(started.elapsed().as_secs_f64() * 1e3);
            bytes = bytes.max(json.len());
            drop(feed);
        }
        (percentiles(ms), bytes)
    };
    let json =
        |doc: &Document, change: Change| serde_json::to_vec(&doc.snapshot_patch(change)).unwrap();
    let mut k = 0;
    let edits = timed(&mut |doc| {
        k += 1;
        let tx = Transaction {
            label: "Move layer".into(),
            ops: vec![Op::MoveItem {
                id: format!("title-{k}"),
                track_id: None,
                start_us: secs(5.5 * k as f64 + 1.0),
            }],
            origin: None,
            ..Default::default()
        };
        let change = doc.apply_change(&tx).unwrap();
        json(doc, change)
    });
    let undos = timed(&mut |doc| {
        let change = doc.undo_change().unwrap();
        json(doc, change)
    });
    let redos = timed(&mut |doc| {
        let change = doc.redo_change().unwrap();
        json(doc, change)
    });
    let whole = timed(&mut |doc| serde_json::to_vec(&doc.snapshot()).unwrap());
    for (name, ((p50, p95), bytes)) in [
        ("move a layer", edits),
        ("undo", undos),
        ("redo", redos),
        ("the whole snapshot", whole),
    ] {
        println!("{name}: p50/p95 {p50:.2}/{p95:.2} ms, up to {bytes} bytes of JSON");
    }
}

/// What a burst of 240 small changes by one app (the pace's most for all apps in 5 minutes)
/// costs the history on the 1-hour project. From the 13th on, each commit folds the app's two oldest steps at its share
/// of 12, composing the growing folded step with the next and sizing it, under the project's lock. Per commit (the
/// diff, the step and its folds): p50, p95 and the most, the whole burst, and the folded step's share of the history.
#[test]
#[ignore = "timing; run in release with --ignored --nocapture"]
fn an_apps_burst_of_240_changes_folding_at_its_share() {
    let mut doc = Document::new(one_hour_project()).unwrap();
    doc.set_folded_label(|app| {
        let params = [("client".to_owned(), app.into())].into();
        ("mcp.fromEarlier".to_owned(), Some(params))
    });
    let shares = [
        Share {
            of: Whose::App("app"),
            steps: 12,
            bytes: 4 << 20,
        },
        Share {
            of: Whose::Apps,
            steps: 25,
            bytes: 8 << 20,
        },
    ];
    let by = ByApp {
        app: "app",
        shares: &shares,
        after_point: false,
    };
    let mut ms = Vec::new();
    let started = Instant::now();
    for k in 0..240 {
        let ops = if k % 2 == 0 {
            vec![Op::MoveItem {
                id: format!("title-{k}"),
                track_id: None,
                start_us: secs(5.5 * k as f64 + 1.0),
            }]
        } else {
            vec![Op::Rename {
                name: format!("Take {k}"),
            }]
        };
        let tx = Transaction {
            label: "mcp.from".into(),
            ops,
            origin: None,
            ..Default::default()
        };
        let next = apply_ops(doc.project(), &tx.ops).unwrap();
        let commit = Instant::now();
        doc.commit_tx_keeping(&tx, next, &by).unwrap();
        ms.push(commit.elapsed().as_secs_f64() * 1e3);
    }
    let whole = started.elapsed();
    let most = ms.iter().copied().fold(0.0, f64::max);
    let total: f64 = ms.iter().sum();
    ms.sort_by(f64::total_cmp);
    println!(
        "240 app changes: commit p50/p95/max {:.2}/{:.2}/{most:.2} ms, {total:.0} ms committing, {whole:?} with the \
         dry runs; {} undo steps, {} history bytes",
        ms[ms.len() / 2],
        ms[ms.len() * 95 / 100],
        doc.undo_steps(),
        doc.history_bytes()
    );
    assert_eq!(doc.undo_steps(), 12, "the app holds its share");
}
