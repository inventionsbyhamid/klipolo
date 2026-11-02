//! What analysis is for (`analysed::analysed_assets`): the videos and sounds a main-track clip plays, never music, a
//! sound effect or a picture on a layer, nor a file imported and never placed. The page's `analysedAssets`
//! (`apps/desktop/src/lib/analysed.ts`) is pinned to it by `apps/desktop/src/lib/analysed-parity.json`, written here
//! with `UPDATE_GOLDEN=1`. A recording counts as one clip: the page offers its voice files, the ones its captions read
//! (`speech::run_spans`), and knows which files are heard for their words only (`analysed::words_only`), both pinned by
//! `apps/desktop/src/lib/voices-parity.json`, written here too.
mod common;
use std::path::PathBuf;

use common::*;
use klipolo_model::analysed::{analysed_assets, camera_footage, camera_footage_file, words_only};
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{Asset, AssetKind, RecordingKind, RecordingSource, SourceMix};
use klipolo_model::speech::run_spans;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind};
use klipolo_model::Project;
use serde_json::json;

fn file(id: &str, kind: AssetKind) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs(16.0),
        width: 0,
        height: 0,
        fps: 0.0,
        rotation: None,
        has_audio: kind != AssetKind::Image,
        audio_lead_us: None,
        made_with: None,
    }
}

/// A sound layer of `asset` from 0 for 4 s, ducking `duck` (music) or not (a sound effect).
fn sound(id: &str, asset: &str, duck: Option<f64>) -> Item {
    Item {
        id: id.into(),
        start_us: 0,
        duration_us: secs(4.0),
        attach: None,
        layout: Layout::new(Anchor::TopLeft, BoxN::FULL),
        content: ItemContent::Audio {
            asset_id: asset.into(),
            source_in_us: 0,
            volume: 0.3,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck,
            volume_keys: Vec::new(),
        },
        effects: Vec::new(),
    }
}

/// The fixture (a screen recording with camera and mic, then b-roll) with the pack's bed as music, a placed whoosh, a
/// song imported and never placed, and the fixture's own music file and logo, which no clip plays.
fn with_media() -> Project {
    let mut p = fixture();
    p.assets.extend([
        file("klipolo-pack-soft-ambient", AssetKind::Audio),
        file("klipolo-pack-whoosh", AssetKind::Audio),
        file("my-song", AssetKind::Audio),
    ]);
    let mut audio = Track::new(TrackKind::Audio, "Audio 1");
    audio.id = "audio".into();
    audio.items = vec![sound("bed", "klipolo-pack-soft-ambient", Some(-12.0))];
    let mut sounds = Track::new(TrackKind::Audio, "Audio 2");
    sounds.id = "sounds".into();
    sounds.items = vec![sound("whoosh", "klipolo-pack-whoosh", None)];
    p.timeline.tracks = vec![audio, sounds];
    p
}

fn ids(p: &Project) -> Vec<&str> {
    analysed_assets(p).iter().map(|a| a.id.as_str()).collect()
}

#[test]
fn only_the_files_main_track_clips_play_are_analysed() {
    let p = with_media();
    assert_eq!(ids(&p), ["screen", "camera", "mic", "broll"]);
    // An imported video alone on the main track, twice: once.
    let mut alone = with_media();
    alone.recordings.clear();
    let mut again = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(4.0));
    again.id = "c".into();
    alone.timeline.main.remove(0);
    alone.timeline.main.push(again);
    assert_eq!(ids(&alone), ["broll"]);
    // Nothing on the main track: nothing to analyse, the song and the bed included.
    let mut empty = with_media();
    empty.timeline.main.clear();
    assert_eq!(ids(&empty), Vec::<&str>::new());
}

#[test]
fn analysed_parity_json_is_current() {
    let mut alone = with_media();
    alone.timeline.main.remove(0);
    let mut empty = with_media();
    empty.timeline.main.clear();
    let cases: Vec<serde_json::Value> = [
        (
            "a recording and b-roll, with music and sounds on layers",
            with_media(),
        ),
        ("b-roll alone", alone),
        ("nothing on the main track", empty),
    ]
    .into_iter()
    .map(|(name, p)| {
        json!({
            "name": name,
            "project": {
                "assets": p.assets,
                "recordings": p.recordings,
                "timeline": { "main": p.timeline.main, "tracks": p.timeline.tracks },
            },
            "analysed": ids(&p),
        })
    })
    .collect();
    assert_golden(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/lib/analysed-parity.json"),
        &sorted_keys(json!({ "cases": cases })),
    );
}

/// Every file of a recording is heard for its words only, in any of its sources' runs, and so is the same file imported
/// again (by its path): neither is ever watched for a scene log. A file imported on its own is watched.
#[test]
fn a_recordings_files_and_the_same_files_imported_again_are_words_only() {
    let mut p = fixture();
    let mut again = p.asset("screen").unwrap().clone();
    again.id = "screen-again".into();
    p.assets.push(again);
    for id in ["screen", "camera", "mic", "screen-again"] {
        assert!(words_only(&p, id), "{id}");
    }
    for id in ["broll", "music", "logo", "gone"] {
        assert!(!words_only(&p, id), "{id}");
    }
}

/// The fixture's take as a camera take ("Record yourself"): its camera (0.2 s late) and its mic, no screen.
fn camera_take() -> Project {
    let mut p = fixture();
    let rec = &mut p.recordings[0];
    rec.sources.retain(|s| s.id != "screen");
    (rec.kind, rec.primary) = (RecordingKind::Camera, "camera".into());
    p
}

/// A camera take's camera is its main footage: watched for a scene log as imported camera footage is, and so is the
/// same file imported again. Its mic is heard for its words only, and a screen take's camera bubble stays words only.
#[test]
fn a_camera_take_s_footage_is_watched_and_its_mic_heard_for_words() {
    let mut p = camera_take();
    let mut again = p.asset("camera").unwrap().clone();
    again.id = "camera-again".into();
    p.assets.push(again);
    for id in ["camera", "camera-again"] {
        assert!(camera_footage_file(&p, id), "{id}");
        assert!(!words_only(&p, id), "{id}");
    }
    assert!(words_only(&p, "mic"));
    assert!(!camera_footage_file(&p, "mic"));
    assert!(!camera_footage_file(&fixture(), "camera"), "a bubble");
    assert!(words_only(&fixture(), "camera"));
}

/// A camera take's clip plays its camera's runs on the take's clock; any other clip has no camera footage.
#[test]
fn a_camera_take_s_clip_plays_its_camera_runs_on_the_take_s_clock() {
    let p = camera_take();
    let runs = camera_footage(&p, &p.timeline.main[0]);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].asset_id(), "camera");
    // The camera started 0.2 s into the take: its file's 0 is the clip's 0.2 s.
    assert_eq!(runs[0].source_of(0), 200_000);
    assert_eq!(runs[0].source_of(secs(1.0)), secs(1.2));
    assert_eq!(runs[0].source_span(), (200_000, secs(60.0)));
    assert_eq!(runs[0].file_of(secs(1.2)), secs(1.0));
    assert!(camera_footage(&p, &p.timeline.main[1]).is_empty(), "b-roll");
    assert!(camera_footage(&fixture(), &fixture().timeline.main[0]).is_empty(), "a screen take");
}

/// `p` with file `id`'s sound on or off.
fn audio(p: &mut Project, id: &str, on: bool) {
    p.assets.iter_mut().find(|a| a.id == id).unwrap().has_audio = on;
}

/// The fixture recording's source `id`.
fn source<'a>(p: &'a mut Project, id: &str) -> &'a mut RecordingSource {
    p.recordings[0]
        .sources
        .iter_mut()
        .find(|s| s.id == id)
        .unwrap()
}

/// A clip `id` of the fixture recording, `from` to `to` seconds of it.
fn take_clip(id: &str, from: f64, to: f64) -> Clip {
    let mut c = Clip::new(
        ClipSource::Recording { id: "rec".into() },
        secs(from),
        secs(to),
    );
    c.id = id.into();
    c
}

/// Takes whose voice the captions read from more than the mic, or from no mic at all: the fixture's take (screen with
/// sound, camera without, mic) in clip "a", 0 to 20 s, then b-roll, changed as each case says.
fn voice_cases() -> Vec<(&'static str, Project)> {
    let mut cases = vec![("a mic", fixture())];

    let mut p = fixture();
    p.recordings[0].sources.retain(|s| s.id != "mic");
    audio(&mut p, "screen", false);
    audio(&mut p, "camera", true);
    cases.push(("no mic: a camera with sound and a silent screen", p));

    let mut p = fixture();
    p.recordings[0].sources.retain(|s| s.id != "mic");
    cases.push(("no mic: the screen's own sound", p));

    // The mic reconnected 30 s in, into a second file; "a" plays the first 20 s, "a2" 25 to 40 s.
    let mut p = fixture();
    second_file(&mut p, "mic", "mic2");
    let mic = source(&mut p, "mic");
    mic.runs[0].duration_us = secs(30.0);
    let mut run = mic.runs[0].clone();
    run.asset_id = "mic2".into();
    run.offset_us = secs(30.0);
    mic.runs.push(run);
    p.timeline.main.push(take_clip("a2", 25.0, 40.0));
    cases.push(("a mic in two files under trimmed clips", p));

    // The mic started 5 s late; the camera's own sound, muted, fills the stretch before it. "a2" starts after it.
    let mut p = fixture();
    audio(&mut p, "screen", false);
    audio(&mut p, "camera", true);
    let camera = source(&mut p, "camera");
    (camera.mix.muted, camera.mix.fills_gaps) = (true, true);
    let mic = source(&mut p, "mic");
    (mic.runs[0].offset_us, mic.runs[0].duration_us) = (secs(5.0), secs(55.0));
    p.timeline.main.push(take_clip("a2", 5.0, 20.0));
    cases.push((
        "a muted camera filling the stretch before a late mic",
        p.clone(),
    ));

    // The same camera unmuted plays everywhere, so it fills the gap too, unless a clip's own mix mutes it there.
    let camera = source(&mut p, "camera");
    (camera.mix.muted, camera.mix.fills_gaps) = (false, false);
    cases.push(("an unmuted camera with sound over a late mic", p.clone()));
    let muted = SourceMix {
        muted: true,
        ..SourceMix::default()
    };
    p.timeline.main[0].source_mix.insert("camera".into(), muted);
    cases.push(("a clip's own mix muting that camera", p));

    // The mic nudged 2 s later: the screen's own sound plays before it.
    let mut p = fixture();
    source(&mut p, "mic").nudge_us = secs(2.0);
    cases.push(("a nudged mic", p));

    // The take's screen file imported again (the same path) and its silent camera, each a clip of its own.
    let mut p = fixture();
    let mut again = p.asset("screen").unwrap().clone();
    again.id = "screen-again".into();
    p.assets.push(again);
    for (id, asset) in [("c", "screen-again"), ("d", "camera")] {
        let mut c = Clip::new(ClipSource::Asset { id: asset.into() }, 0, secs(5.0));
        c.id = id.into();
        p.timeline.main.push(c);
    }
    cases.push(("a take's files placed on their own", p));

    // "Record yourself": the camera is the footage, watched; the mic is its voice.
    cases.push(("a camera take: its camera is the footage", camera_take()));
    cases
}

#[test]
fn voices_parity_json_is_current() {
    let cases: Vec<serde_json::Value> = voice_cases()
        .into_iter()
        .map(|(name, p)| {
            let spans = run_spans(&p);
            let takes: Vec<serde_json::Value> = (p.timeline.main.iter())
                .filter_map(|clip| {
                    let ClipSource::Recording { id } = &clip.source else {
                        return None;
                    };
                    let mut voices: Vec<&str> = (spans.iter())
                        .filter(|s| s.clip == clip.id)
                        .map(|s| s.asset.as_str())
                        .collect();
                    voices.sort_unstable();
                    voices.dedup();
                    Some(json!({ "clip": clip.id, "recording": id, "voices": voices }))
                })
                .collect();
            let words_only: Vec<&str> = (p.assets.iter())
                .filter(|a| words_only(&p, &a.id))
                .map(|a| a.id.as_str())
                .collect();
            json!({
                "name": name,
                "project": {
                    "assets": p.assets,
                    "recordings": p.recordings,
                    "timeline": { "main": p.timeline.main },
                },
                "takes": takes,
                "wordsOnly": words_only,
            })
        })
        .collect();
    assert_golden(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/lib/voices-parity.json"),
        &sorted_keys(json!({ "cases": cases })),
    );
}
