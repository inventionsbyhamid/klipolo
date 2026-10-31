//! Format 4 for Sync: a synced recording made from an imported video and a recorder's file. `clips` moves the video's
//! video's clips onto it (`useRecording`) and checks nothing seen, placed or heard is lost; `sources` checks what a
//! synced source may hold (two mics, a run inside its file, re-timed runs, a source added to a take); `paused` a recorder
//! paused inside one file, as two runs of it.

#[path = "../common/mod.rs"]
mod common;

mod chapters;
mod clips;
mod fill;
mod follow;
mod jcut;
mod less_sound;
mod paused;
mod rules;
mod sources;

use klipolo_model::project::{
    Asset, AssetKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
    SyncLevel, Synced, SyncedBy,
};
use klipolo_model::time::Us;

use common::secs;

/// A run of `asset`: `file_in_us` into its file at the clock's zero (offset 0), `duration_us` of its own time.
pub fn run(asset: &str, file_in_us: Us, duration_us: Us, drift_ppm: f64) -> SourceRun {
    SourceRun {
        asset_id: asset.into(),
        offset_us: 0,
        duration_us,
        drift_ppm,
        file_in_us,
    }
}

pub const SURE: Option<Synced> = Some(Synced {
    by: SyncedBy::Sound,
    level: SyncLevel::Sure,
});

/// The recording Sync makes: the camera is the clock (offset 0); the mic started 90 s earlier, so its run starts
/// 90 s into its file, at the clock's zero; it runs 50 ppm fast.
pub fn synced() -> Recording {
    Recording {
        id: "r9".into(),
        kind: RecordingKind::Camera,
        sources: vec![
            RecordingSource {
                id: "cam".into(),
                role: SourceRole::Camera,
                label: "interview.mov".into(),
                runs: vec![run("cam", 0, 600_000_000, 0.0)],
                nudge_us: 0,
                mix: SourceMix {
                    muted: true,
                    ..SourceMix::default()
                },
                synced: None,
            },
            RecordingSource {
                id: "mic".into(),
                role: SourceRole::Mic,
                label: "ZOOM0001.WAV".into(),
                runs: vec![run("mic", 90_000_000, 610_000_000, 50.0)],
                nudge_us: 0,
                mix: SourceMix::default(),
                synced: SURE,
            },
        ],
        primary: "cam".into(),
        cursor: None,
        markers: vec![],
        game: None,
    }
}

/// A file like the fixture's `cam` (a video with sound) or `mic` (a sound), under another id and length.
pub fn file(id: &str, kind: AssetKind, seconds: f64) -> Asset {
    let video = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs(seconds),
        width: if video { 1920 } else { 0 },
        height: if video { 1080 } else { 0 },
        fps: if video { 30.0 } else { 0.0 },
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }
}

/// A one-run source of `asset` at the clock's zero.
pub fn source(id: &str, role: SourceRole, asset: &str, seconds: f64) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("{id}.wav"),
        runs: vec![run(asset, 0, secs(seconds), 0.0)],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: SURE,
    }
}
