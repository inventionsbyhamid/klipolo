//! A synced recording's sound: a picture source whose file carries sound plays it at its mix (an imported camera's own
//! camera's own sound), Klipolo's takes stay as they were, and the synced mic plays where the camera heard it, its run
//! starting inside its file and corrected for its drift. A picture file's own sound plays with its sound lead.
use std::sync::Arc;

use klipolo_engine::audio::mix::{Mixdown, RATE, read_position_after};
use klipolo_engine::audio::pcm::Pcm;
use klipolo_engine::audio::plan::{AudioSource, audio_plan, may_change_sound};
use klipolo_engine::registry::Registry;
use klipolo_model::Op;
use klipolo_model::contrib::builtin;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::ops::SourcePatch;
use klipolo_model::project::*;
use klipolo_model::time::{Us, secs_to_us};
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind, Transition};

/// The mic started 9 s before the camera, so its run starts 9 s into its file; it runs 50 ppm fast.
const FILE_IN_S: f64 = 9.0;
const PPM: f64 = 50.0;

fn asset(id: &str, kind: AssetKind, seconds: f64, has_audio: bool) -> Asset {
    let video = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs_to_us(seconds),
        width: if video { 1920 } else { 0 },
        height: if video { 1080 } else { 0 },
        fps: if video { 30.0 } else { 0.0 },
        has_audio,
        audio_lead_us: None,
        rotation: None,
    }
}

fn source(id: &str, role: SourceRole, file_in_s: f64, seconds: f64, ppm: f64) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("{id}.file"),
        runs: vec![SourceRun {
            asset_id: id.into(),
            offset_us: 0,
            duration_us: secs_to_us(seconds),
            drift_ppm: ppm,
            file_in_us: secs_to_us(file_in_s),
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

/// A recording of a 60 s camera file (with sound when `camera_sound`) and a 79 s mic file, as clips `c1` (2–20 s) and
/// `c2` (25–45 s at 1.5×, from 18 s on the timeline). The camera's sound is muted, as Sync leaves it.
fn synced(camera_sound: bool) -> Project {
    let mut p = Project::new("Synced");
    p.id = "project".into();
    p.assets = vec![
        asset("cam", AssetKind::Video, 60.0, camera_sound),
        asset("mic", AssetKind::Audio, 80.0, true),
    ];
    let mut cam = source("cam", SourceRole::Camera, 0.0, 60.0, 0.0);
    cam.mix.muted = true;
    p.recordings = vec![Recording {
        id: "r9".into(),
        kind: RecordingKind::Camera,
        sources: vec![cam, source("mic", SourceRole::Mic, FILE_IN_S, 70.0, PPM)],
        primary: "cam".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }];
    let clip = |id: &str, from: f64, to: f64, speed: f64| {
        let mut c = Clip::new(
            ClipSource::Recording { id: "r9".into() },
            secs_to_us(from),
            secs_to_us(to),
        );
        c.id = id.into();
        c.speed = speed;
        c
    };
    p.timeline.main = vec![clip("c1", 2.0, 20.0, 1.0), clip("c2", 25.0, 45.0, 1.5)];
    klipolo_model::validate(&p).unwrap();
    p
}

/// Each planned source as (file, gain).
fn heard(p: &Project) -> Vec<(String, f64)> {
    audio_plan(p, &Registry::builtin())
        .sources
        .iter()
        .map(|s| (s.path.clone(), s.gain))
        .collect()
}

fn unmute_camera(p: &mut Project, volume: f64) {
    let cam = &mut p.recordings[0].sources[0];
    cam.mix.muted = false;
    cam.mix.volume = volume;
}

#[test]
fn a_picture_source_with_sound_plays_it_at_its_mix_and_klipolo_takes_stay_silent() {
    let mut p = synced(true);
    let mic = ("/media/mic".to_owned(), 1.0);
    assert_eq!(
        heard(&p),
        [mic.clone(), mic.clone()],
        "the camera's sound is muted"
    );
    unmute_camera(&mut p, 0.5);
    let cam = ("/media/cam".to_owned(), 0.5);
    assert_eq!(
        heard(&p),
        [cam.clone(), mic.clone(), cam, mic.clone()],
        "in source order, per clip"
    );
    // A Klipolo take: its picture file has no sound, so unmuting its source plays nothing more.
    let mut take = synced(false);
    unmute_camera(&mut take, 1.0);
    assert_eq!(heard(&take), [mic.clone(), mic]);
}

/// Mono PCM whose every frame holds its own index ÷ 1e6, so a sample read names the file frame it came from.
fn frame_numbers(seconds: usize) -> Arc<Pcm> {
    let frames = seconds * RATE as usize;
    Arc::new(Pcm::from_channels(
        (0..frames).map(|i| i as f32 / 1e6).collect(),
        1,
    ))
}

#[test]
fn the_mixer_plays_the_synced_mic_where_the_camera_heard_it() {
    // The mic's file frame at recording time rec is (rec × (1 + 50 ppm) + 9 s) × 48 kHz.
    let file_frame = |rec: f64| (rec * (1.0 + PPM * 1e-6) + FILE_IN_S) * f64::from(RATE);
    let at = |frame: u64| frame as f64 / f64::from(RATE);
    let plan = Arc::new(audio_plan(&synced(true), &Registry::builtin()));
    // Inside c1 (1×, its in-point 2 s at 0 on the timeline), rec(t) = t + 2 s: the mixer reads that very frame.
    let pcm = frame_numbers(80);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let mut mixdown = Mixdown::new(plan.clone());
    let mut block = vec![0f32; 2 * 4_800];
    let from = 10 * u64::from(RATE);
    mixdown.render(&lookup, from, &mut block);
    // Past the 3 ms ramp in after the seek.
    for (k, lr) in block.chunks_exact(2).enumerate().skip(200) {
        let want = file_frame(at(from + k as u64) + 2.0);
        let heard = f64::from(lr[0]) * 1e6;
        assert!((heard - want).abs() < 2.0, "frame {k}: {heard} vs {want}");
    }
    // Inside c2 (1.5×, from 18 s on the timeline), rec(t) = 25 s + (t − 18 s) × 1.5. A sped-up voice plays through
    // `atempo` (pitch kept, so its samples don't name their frame): the voice reads where its source's position says.
    let mic = plan
        .sources
        .iter()
        .find(|s| s.start_us == 18_000_000)
        .unwrap();
    for t in [22.0, 30.5] {
        let into = ((t - 18.0) * f64::from(RATE)) as u64;
        let want = file_frame(25.0 + (t - 18.0) * 1.5);
        let reads = read_position_after(mic, into);
        assert!((reads - want).abs() < 2.0, "at {t} s: {reads} vs {want}");
    }
}

#[test]
fn sync_ops_replan_the_sound() {
    let p = synced(true);
    let runs = p.recordings[0].sources[1].runs.clone();
    let mut extra = source("mic", SourceRole::Mic, 0.0, 10.0, 0.0);
    extra.id = "mic2".into();
    for op in [
        Op::UseRecording {
            clip_id: "c1".into(),
            recording_id: "r9".into(),
        },
        Op::AddRecordingSource {
            recording_id: "r9".into(),
            source: extra,
        },
        Op::UpdateRecordingSource {
            recording_id: "r9".into(),
            source_id: "mic".into(),
            patch: SourcePatch {
                runs: Some(runs),
                ..SourcePatch::default()
            },
        },
    ] {
        assert!(may_change_sound(&p, &op), "{op:?}");
    }
}

/// `p` with the camera file's sound starting `lead_us` after its picture (the probe's `audio_lead_us`).
fn with_lead(p: &Project, lead_us: Us) -> Project {
    let mut p = p.clone();
    p.assets[0].audio_lead_us = Some(lead_us);
    p
}

/// An imported clip of the camera file, showing its `from`–`to` s.
fn imported(id: &str, from: f64, to: f64) -> Clip {
    let mut c = Clip::new(
        ClipSource::Asset { id: "cam".into() },
        secs_to_us(from),
        secs_to_us(to),
    );
    c.id = id.into();
    c
}

#[test]
fn the_camera_sound_plays_with_its_own_lead() {
    // The camera's sound reads its file at the time its picture shows less its lead, wherever its picture plays:
    // the picture source's runs (c1, and c2 at 2× from a 1 s dissolve), an imported clip of the file (c3, its 5–10 s)
    // and an overlay video layer of it (its 7 s, at 1 s). The mic is a sound file: it has no lead and doesn't move.
    let mut p = synced(true);
    unmute_camera(&mut p, 1.0);
    p.timeline.main[0].transition_out = Some(Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs_to_us(1.0),
        params: Default::default(),
    });
    p.timeline.main[1].speed = 2.0;
    p.timeline.main.push(imported("c3", 5.0, 10.0));
    let mut layers = Track::new(TrackKind::Overlay, "layers");
    layers.items.push(Item {
        id: "v".into(),
        start_us: secs_to_us(1.0),
        duration_us: secs_to_us(4.0),
        attach: None,
        layout: Layout::new(
            Anchor::Center,
            BoxN {
                x: 0.5,
                y: 0.5,
                w: 0.3,
                h: 0.3,
            },
        ),
        content: ItemContent::Video {
            asset_id: "cam".into(),
            source_in_us: secs_to_us(7.0),
            speed: 1.0,
            volume: 1.0,
            muted: false,
            mirror: false,
            tie: None,
        },
        effects: Vec::new(),
    });
    p.timeline.tracks.push(layers);
    let plan = |lead| audio_plan(&with_lead(&p, lead), &Registry::builtin()).sources;
    let none = plan(0);
    let shown: Vec<Us> = none
        .iter()
        .filter(|s| s.path == "/media/cam")
        .map(|s| s.source_in_us)
        .collect();
    assert_eq!(
        shown,
        [2.0, 24.0, 5.0, 7.0].map(secs_to_us),
        "the file times the camera's picture shows: c1, c2 (from 25 s less 0.5 s of pre-roll at 2×), c3, the layer"
    );
    // A sound that starts 40 ms after the picture reads 40 ms earlier, and one that starts 21 ms before it 21 ms later:
    // once, never scaled by c2's speed or its pre-roll. A hand-edited lead past the probe's ±500 ms counts as 500 ms.
    for (lead, moved) in [
        (40_000, 40_000),
        (-21_000, -21_000),
        (i64::MAX, 500_000),
        (i64::MIN, -500_000),
    ] {
        let led = plan(lead);
        assert_eq!(led.len(), none.len());
        for (was, now) in none.iter().zip(&led) {
            let moved = if was.path == "/media/cam" { moved } else { 0 };
            assert_eq!(
                *now,
                AudioSource {
                    source_in_us: was.source_in_us - moved,
                    ..was.clone()
                },
                "lead {lead}: only where it reads its file moves"
            );
        }
    }
}

#[test]
fn a_sound_that_starts_after_its_picture_is_silent_until_it_starts() {
    // An imported clip from the file's start, its sound 40 ms late: 40 ms of silence, then file frame 0 onwards.
    let mut p = with_lead(&synced(true), 40_000);
    p.timeline.main = vec![imported("c", 0.0, 2.0)];
    let plan = Arc::new(audio_plan(&p, &Registry::builtin()));
    assert_eq!(plan.sources[0].source_in_us, -40_000);
    let pcm = frame_numbers(2);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let mut block = vec![0f32; 2 * 4_800];
    Mixdown::new(plan).render(&lookup, 0, &mut block);
    let late = (0.04 * f64::from(RATE)) as usize;
    for (k, lr) in block.chunks_exact(2).enumerate() {
        let want = k.saturating_sub(late) as f64;
        let heard = f64::from(lr[0]) * 1e6;
        assert!((heard - want).abs() < 0.5, "frame {k}: {heard} vs {want}");
    }
}
