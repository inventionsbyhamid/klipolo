//! What Sync starts from: an imported video on the main track, cut into clips, with titles tied to them, and a
//! separately recorded sound already placed on an audio track.

use klipolo_model::contrib::builtin;
use klipolo_model::project::*;
use klipolo_model::query::resolve_attach;
use klipolo_model::time::Us;
use klipolo_model::timeline::*;

use super::{asset, overlay_track, secs, still_item, text_item};

/// - the video asset `cam` (600 s, with sound) on the main track as clips `c1` (10–100 s of the file), `c2` (150–400 s,
///   at 1.5×) and `c9` (450–500 s), with a transition from `c1` into `c2`;
/// - titles tied to `c2` at its file second 200 (`t2`) and to `c9` at 470 (`t9`);
/// - the sound asset `mic` (700 s) and a layer of it, `i9`, on an audio track.
pub fn imported() -> Project {
    let mut p = Project::new("Imported");
    p.id = "project".into();
    p.assets = vec![
        asset("cam", AssetKind::Video, 600.0, 1920, 1080, true),
        asset("mic", AssetKind::Audio, 700.0, 0, 0, true),
    ];
    let cam = || ClipSource::Asset { id: "cam".into() };
    let clip = |id: &str, from: f64, to: f64| {
        let mut c = Clip::new(cam(), secs(from), secs(to));
        c.id = id.into();
        c
    };
    let mut c1 = clip("c1", 10.0, 100.0);
    c1.transition_out = Some(Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(1.0),
        params: Default::default(),
    });
    let mut c2 = clip("c2", 150.0, 400.0);
    c2.speed = 1.5;
    p.timeline.main = vec![c1, c2, clip("c9", 450.0, 500.0)];
    let main = &p.timeline.main;
    let title = |id: &str, on: &str, at: f64| {
        let mut t = text_item(id, 0.0, 3.0);
        let attach = Attach {
            clip_id: on.into(),
            source: cam(),
            source_us: secs(at),
        };
        t.start_us = resolve_attach(main, &attach).unwrap();
        t.attach = Some(attach);
        t
    };
    let titles = overlay_track(
        "titles",
        vec![title("t2", "c2", 200.0), title("t9", "c9", 470.0)],
    );
    let mut sound = Track::new(TrackKind::Audio, "sound");
    sound.id = "sound".into();
    sound.items = vec![Item {
        content: ItemContent::Audio {
            asset_id: "mic".into(),
            source_in_us: secs(100.0),
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        ..still_item("i9", 0.0, 90.0)
    }];
    p.timeline.tracks = vec![titles, sound];
    p
}

pub fn clip<'a>(p: &'a Project, id: &str) -> &'a Clip {
    p.timeline.main.iter().find(|c| c.id == id).unwrap()
}

/// The title tied to clip `on`.
pub fn title_on<'a>(p: &'a Project, on: &str) -> &'a Item {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| {
            matches!(i.content, ItemContent::Text { .. })
                && i.attach.as_ref().is_some_and(|a| a.clip_id == on)
        })
        .unwrap()
}

pub fn title_start(p: &Project, on: &str) -> Us {
    title_on(p, on).start_us
}

/// Sync's part of the frozen shapes: a synced recording whose camera file has sound starting 40 ms after its picture
/// picture (`audioLeadUs`) and fills the gaps in the recorded sound (`fillsGaps`), and whose mic starts
/// inside its file, added to `p`, and Sync's ops (`useRecording`,
/// `addRecordingSource`, re-timed runs) for the ops golden.
pub fn sync_shapes(p: &mut Project) -> Vec<klipolo_model::ops::Op> {
    use klipolo_model::ops::{Op, SourcePatch};
    p.assets.push(Asset {
        audio_lead_us: Some(40_000),
        made_with: None,
        ..asset("interview", AssetKind::Video, 600.0, 1920, 1080, true)
    });
    p.assets
        .push(asset("zoom", AssetKind::Audio, 700.0, 0, 0, true));
    let run = |asset: &str, file_in_us: Us, duration_us: Us, drift_ppm: f64| SourceRun {
        asset_id: asset.into(),
        offset_us: 0,
        duration_us,
        drift_ppm,
        file_in_us,
    };
    let sure = Some(Synced {
        by: SyncedBy::Sound,
        level: SyncLevel::Sure,
    });
    // The camera's sound, kept muted, fills the gaps in the recorded sound.
    let source = |id: &str, role, label: &str, runs, muted, synced| RecordingSource {
        id: id.into(),
        role,
        label: label.into(),
        runs,
        nudge_us: 0,
        mix: SourceMix {
            muted,
            fills_gaps: role == SourceRole::Camera,
            ..SourceMix::default()
        },
        synced,
    };
    p.recordings.push(Recording {
        id: "synced".into(),
        kind: RecordingKind::Camera,
        sources: vec![
            source(
                "cam",
                SourceRole::Camera,
                "interview.mov",
                vec![run("interview", 0, secs(600.0), 0.0)],
                true,
                None,
            ),
            source(
                "mic",
                SourceRole::Mic,
                "ZOOM0001.WAV",
                vec![run("zoom", secs(90.0), secs(610.0), 50.0)],
                false,
                sure,
            ),
        ],
        primary: "cam".into(),
        cursor: None,
        markers: vec![],
        game: None,
    });
    let hand = Some(Synced {
        by: SyncedBy::Hand,
        level: SyncLevel::Check,
    });
    vec![
        Op::UseRecording {
            clip_id: "b".into(),
            recording_id: "synced".into(),
        },
        Op::AddRecordingSource {
            recording_id: "rec".into(),
            source: source(
                "lav",
                SourceRole::Mic,
                "LAV.WAV",
                vec![run("music", secs(5.0), secs(50.0), -30.0)],
                false,
                hand,
            ),
        },
        Op::UpdateRecordingSource {
            recording_id: "synced".into(),
            source_id: "mic".into(),
            patch: SourcePatch {
                nudge_us: Some(0),
                runs: Some(vec![run("zoom", secs(90.5), secs(609.0), 48.0)]),
                synced: sure,
                ..SourcePatch::default()
            },
        },
    ]
}
