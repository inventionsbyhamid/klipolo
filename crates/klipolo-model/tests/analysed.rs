//! What analysis is for (`analysed::analysed_assets`): the videos and sounds a main-track clip plays, never music, a
//! sound effect or a picture on a layer, nor a file imported and never placed. The page's `analysedAssets`
//! (`apps/desktop/src/lib/analysed.ts`) is pinned to it by `apps/desktop/src/lib/analysed-parity.json`, written here
//! with `UPDATE_GOLDEN=1`.
mod common;
use std::path::PathBuf;

use common::*;
use klipolo_model::analysed::analysed_assets;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{Asset, AssetKind};
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
