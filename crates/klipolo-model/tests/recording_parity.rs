//! The timeline's run mapping in TypeScript (`apps/desktop/src/lib/recording.ts`: `runAt`, `runInClip`) is pinned to
//! the model's by `apps/desktop/src/lib/recording-parity.json`, written here with `UPDATE_GOLDEN=1`. Rust is the truth
//! the lanes' waveforms and thumbnails land where the engine plays each run, drift and nudge included.
//! Its `soundSources` part pins which sources a recording clip can be heard from (`query::sound_sources`, the
//! UI's `soundSources`): the lanes and the Sounds popover list them, a synced camera's own sound among them.
mod common;
use std::path::PathBuf;

use common::*;
use klipolo_model::project::*;
use klipolo_model::query::{run_in_clip, sound_sources};
use klipolo_model::time::{drifted, scaled, Us};
use klipolo_model::timeline::{Clip, ClipSource};
use serde_json::json;

fn run(asset: &str, offset_us: Us, duration_us: Us, drift_ppm: f64) -> SourceRun {
    SourceRun {
        asset_id: asset.into(),
        offset_us,
        duration_us,
        drift_ppm,
        file_in_us: 0,
    }
}

fn source(id: &str, role: SourceRole, runs: Vec<SourceRun>, nudge_us: Us) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs,
        nudge_us,
        mix: SourceMix::default(),
        synced: None,
    }
}

const S: Us = 1_000_000;

#[test]
fn the_ui_maps_runs_as_the_model_does() {
    let sources = vec![
        // A clean mic: one run, no drift, no nudge.
        source(
            "mic",
            SourceRole::Mic,
            vec![run("m1", 0, 3_600 * S, 0.0)],
            0,
        ),
        // Game sound healed: a gap, then a second run whose device clock runs fast; nudged 40 ms later.
        source(
            "game-sound",
            SourceRole::GameAudio,
            vec![
                run("g1", 0, 600 * S, -37.5),
                run("g2", 612_345_678, 2_400 * S + 333, 100.0),
            ],
            40_000,
        ),
        // A synced recorder: it started 90 s before the camera, so its run starts 90 s into its file; 50 ppm fast.
        source(
            "zoom",
            SourceRole::Mic,
            vec![SourceRun {
                file_in_us: 90 * S,
                ..run("z1", 0, 610 * S, 50.0)
            }],
            0,
        ),
        // A camera that started late and runs slow, nudged 250 ms earlier.
        source(
            "cam",
            SourceRole::Camera,
            vec![run("c1", 200_000, 1_800 * S, -250.25)],
            -250_000,
        ),
    ];
    // Clips over the take at several speeds, back to back: (source in, source out, speed).
    let shapes = [
        (0, 300 * S, 1.0),
        (590 * S, 640 * S + 7, 1.5),
        (1_799 * S + 900_000, 3_000 * S, 0.75),
        (3_590 * S, 3_600 * S, 2.0),
    ];
    let mut start = 0;
    let mut clips = Vec::new();
    let mut spans = Vec::new();
    for (index, &(source_in, source_out, speed)) in shapes.iter().enumerate() {
        let mut clip = Clip::new(
            ClipSource::Recording { id: "rec".into() },
            source_in,
            source_out,
        );
        clip.speed = speed;
        for s in &sources {
            for (r, run) in s.runs.iter().enumerate() {
                if let Some(hit) = run_in_clip(&clip, start, s, run) {
                    spans.push(json!({
                        "clip": index,
                        "source": s.id,
                        "run": r,
                        "startUs": hit.start_us,
                        "endUs": hit.end_us,
                        "runInUs": hit.run_in_us,
                    }));
                }
            }
        }
        clips.push(json!({
            "sourceInUs": source_in,
            "sourceOutUs": source_out,
            "speed": speed,
            "startUs": start,
        }));
        start += scaled(source_out - source_in, speed);
    }
    // Recording times around every run's edges (after the nudge and the drift's shrink), and inside each.
    let mut times: Vec<Us> = vec![0, 1, 300 * S, 1_234_567_891, 3_000 * S, 3_599 * S];
    for s in &sources {
        for r in &s.runs {
            let (from, to) = r.rec_span(s.nudge_us);
            times.extend([from - 1, from, from + 1, to - 1, to, to + 1]);
        }
    }
    times.sort_unstable();
    times.dedup();
    let at: Vec<_> = sources
        .iter()
        .flat_map(|s| {
            times.iter().map(move |&us| {
                let hit = s.run_at(us);
                json!({
                    "source": s.id,
                    "us": us,
                    "assetId": hit.map(|(r, _)| r.asset_id.clone()),
                    "localUs": hit.map(|(_, local)| local),
                })
            })
        })
        .collect();
    // A separated sound layer plays its file at its run's rate from its own start: the file's time
    // `offset` into the layer is `sourceIn + drifted(offset, ppm)`, as the engine's audio plan steps it. The timeline
    // draws it there: `recording.test.ts` checks paint's mapping (`sourceAtUs`) at the layer's rate (`layerRate`).
    let mut layers = Vec::new();
    for ppm in [100.0, -37.5, -250.25, 0.0] {
        for source_in in [0, 1_234_567] {
            for offset in [0, 1, 999_999, 3_582_500_000, 3_600 * S] {
                layers.push(json!({
                    "driftPpm": ppm,
                    "sourceInUs": source_in,
                    "offsetUs": offset,
                    "fileUs": source_in + drifted(offset, ppm),
                }));
            }
        }
    }
    let golden = sorted_keys(json!({
        "sources": sources,
        "clips": clips,
        "spans": spans,
        "at": at,
        "layers": layers,
        "soundSources": sound_sources_case(),
    }));
    assert_golden(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/lib/recording-parity.json"),
        &golden,
    );
}

fn file(id: &str, kind: AssetKind, has_audio: bool) -> Asset {
    let video = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: 600 * S,
        width: if video { 1920 } else { 0 },
        height: if video { 1080 } else { 0 },
        fps: if video { 30.0 } else { 0.0 },
        rotation: None,
        has_audio,
        audio_lead_us: None,
    }
}

/// A recording whose sources play `files` (source id, role, file), each named after its file.
fn recording(id: &str, kind: RecordingKind, files: &[(&str, SourceRole, &str)]) -> Recording {
    Recording {
        id: id.into(),
        kind,
        sources: files
            .iter()
            .map(|&(s, role, f)| RecordingSource {
                label: f.into(),
                ..source(s, role, vec![run(f, 0, 600 * S, 0.0)], 0)
            })
            .collect(),
        primary: files[0].0.into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }
}

/// The sources a recording clip can be heard from: a synced interview whose camera file carries sound (the
/// camera first, in source order, then its two synced mics), and a take Klipolo recorded, whose screen and camera files
/// have none (its mic alone).
fn sound_sources_case() -> serde_json::Value {
    let mut p = Project::new("Sounds");
    p.assets = vec![
        file("interview.mov", AssetKind::Video, true),
        file("ZOOM0001.WAV", AssetKind::Audio, true),
        file("lav.wav", AssetKind::Audio, true),
        file("screen.mp4", AssetKind::Video, false),
        file("camera.mp4", AssetKind::Video, false),
        file("mic.caf", AssetKind::Audio, true),
    ];
    let mut interview = recording(
        "interview",
        RecordingKind::Camera,
        &[
            ("cam", SourceRole::Camera, "interview.mov"),
            ("zoom", SourceRole::Mic, "ZOOM0001.WAV"),
            ("lav", SourceRole::Mic, "lav.wav"),
        ],
    );
    interview.sources[0].mix.muted = true;
    for s in &mut interview.sources[1..] {
        s.synced = Some(Synced {
            by: SyncedBy::Sound,
            level: SyncLevel::Sure,
        });
    }
    let take = recording(
        "take",
        RecordingKind::Screen,
        &[
            ("screen", SourceRole::Screen, "screen.mp4"),
            ("camera", SourceRole::Camera, "camera.mp4"),
            ("mic", SourceRole::Mic, "mic.caf"),
        ],
    );
    p.recordings = vec![interview, take];
    let found: Vec<_> = p
        .recordings
        .iter()
        .map(|r| {
            let ids: Vec<&str> = sound_sources(&p, r).map(|s| s.id.as_str()).collect();
            json!({ "recording": r.id, "sources": ids })
        })
        .collect();
    assert_eq!(found[0]["sources"], json!(["cam", "zoom", "lav"]));
    assert_eq!(found[1]["sources"], json!(["mic"]));
    json!({ "assets": p.assets, "recordings": p.recordings, "found": found })
}
