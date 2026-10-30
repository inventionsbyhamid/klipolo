//! The camera's sound fills true gaps: where no sound source of a synced recording has a run, its
//! muted camera sound that fills gaps plays, at its mix's volume and with its file's lead; never under a mic that is
//! muted or separated, which still has its runs.

use std::sync::Arc;

use klipolo_engine::audio::mix::mix;
use klipolo_engine::audio::pcm::Pcm;
use klipolo_engine::audio::plan::{AudioSource, audio_plan};
use klipolo_engine::registry::Registry;
use klipolo_model::Op;
use klipolo_model::project::*;
use klipolo_model::time::{Us, secs_to_us};
use klipolo_model::timeline::{Clip, ClipSource};

fn asset(id: &str, kind: AssetKind) -> Asset {
    let video = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs_to_us(500.0),
        width: if video { 1920 } else { 0 },
        height: if video { 1080 } else { 0 },
        fps: if video { 30.0 } else { 0.0 },
        has_audio: true,
        audio_lead_us: None,
        rotation: None,
    }
}

fn source(id: &str, role: SourceRole, from_s: f64, to_s: f64, mix: SourceMix) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("{id}.file"),
        runs: vec![SourceRun {
            asset_id: id.into(),
            offset_us: secs_to_us(from_s),
            duration_us: secs_to_us(to_s - from_s),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix,
        synced: None,
    }
}

/// The r9 shape at a larger scale: the camera over recording 0–500 s, muted at 50 % and filling gaps; the mic over
/// recording 20 s to `mic_to_s`. Clips c1 (recording 10–100 s, from 0 on the timeline, fading in 1 s and out 2 s) and
/// c2 (150–400 s, from 90 s).
fn gapped(mic_to_s: f64) -> Project {
    let mut p = Project::new("Gapped");
    p.assets = vec![
        asset("cam", AssetKind::Video),
        asset("mic", AssetKind::Audio),
    ];
    let cam = SourceMix {
        volume: 0.5,
        muted: true,
        fills_gaps: true,
        ..SourceMix::default()
    };
    p.recordings = vec![Recording {
        id: "r9".into(),
        kind: RecordingKind::Camera,
        sources: vec![
            source("cam", SourceRole::Camera, 0.0, 500.0, cam),
            source("mic", SourceRole::Mic, 20.0, mic_to_s, SourceMix::default()),
        ],
        primary: "cam".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }];
    let clip = |id: &str, from: f64, to: f64| {
        let mut c = Clip::new(
            ClipSource::Recording { id: "r9".into() },
            secs_to_us(from),
            secs_to_us(to),
        );
        c.id = id.into();
        c
    };
    let mut c1 = clip("c1", 10.0, 100.0);
    c1.fade_in_us = secs_to_us(1.0);
    c1.fade_out_us = secs_to_us(2.0);
    p.timeline.main = vec![c1, clip("c2", 150.0, 400.0)];
    klipolo_model::validate(&p).unwrap();
    p
}

/// What plays: (file, from s, to s, gain, its file's second at the start, fade in s, fade out s).
type Heard = (String, f64, f64, f64, f64, f64, f64);

fn heard(p: &Project) -> Vec<Heard> {
    let s = |us: Us| us as f64 / 1e6;
    audio_plan(p, &Registry::builtin())
        .sources
        .iter()
        .map(|a| {
            let file = a.path.trim_start_matches("/media/").to_owned();
            let at = (s(a.start_us), s(a.end_us));
            (
                file,
                at.0,
                at.1,
                a.gain,
                s(a.source_in_us),
                s(a.fade_in_us),
                s(a.fade_out_us),
            )
        })
        .collect()
}

fn line(file: &str, from: f64, to: f64, gain: f64, file_s: f64, fades: (f64, f64)) -> Heard {
    (file.into(), from, to, gain, file_s, fades.0, fades.1)
}

#[test]
fn a_true_gap_plays_the_camera_sound_and_nowhere_else() {
    // c1 plays recording 10–100 s: the camera over its 10–20 s at the camera mix's 50 % (the clip's fade in), then the
    // mic over 20–100 s (its fade out at the clip's end); where the mic takes over, the two cross over 10 ms. c2 has
    // the mic only.
    let cam = |from, to, file_s, fades| line("cam", from, to, 0.5, file_s, fades);
    let mic = |from, to, file_s, fades| line("mic", from, to, 1.0, file_s, fades);
    assert_eq!(
        heard(&gapped(500.0)),
        [
            cam(0.0, 10.01, 10.0, (1.0, 0.01)),
            mic(10.0, 90.0, 0.0, (0.01, 2.0)),
            mic(90.0, 340.0, 130.0, (0.0, 0.0)),
        ]
    );
    // With the mic's run ending at 300 s, c2 gains the camera over recording 300–400 s (240–340 s on the timeline),
    // crossing from the mic's last 10 ms.
    let short = heard(&gapped(300.0));
    assert_eq!(
        short[2..],
        [
            cam(239.99, 340.0, 299.99, (0.01, 0.0)),
            mic(90.0, 240.0, 130.0, (0.0, 0.01))
        ]
    );
    // The fill plays with the camera file's sound lead, as the camera's sound does everywhere.
    let mut led = gapped(500.0);
    led.assets[0].audio_lead_us = Some(40_000);
    assert_eq!(heard(&led)[0], cam(0.0, 10.01, 9.96, (1.0, 0.01)));
}

#[test]
fn a_gap_edge_crosses_without_a_dip() {
    // Where the mic takes over from the camera's fill (10 s on the timeline), two sounds at the same
    // level keep it through the edge: their straight fades add up to 1. Two 3 ms edge ramps used to dip to silence.
    let mut p = gapped(500.0);
    p.recordings[0].sources[0].mix.volume = 1.0;
    let plan = audio_plan(&p, &Registry::builtin());
    let level = Arc::new(Pcm::from_channels(vec![0.25; 48_000 * 30], 1));
    let pcm = |_: &AudioSource| Some(level.clone());
    // 9.95–10.05 s on the timeline: the camera at full (its 1 s fade in is long over), then the crossing.
    let mut out = vec![0.0f32; 2 * 4_800];
    mix(&plan, &pcm, 477_600, 1.0, &mut out);
    let (low, high) = out
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &x| (l.min(x), h.max(x)));
    assert!(high > 0.1, "{high}");
    assert!(
        low >= high * 0.98,
        "a dip at the edge: {low} against {high}"
    );
}

#[test]
fn a_separated_mic_never_lets_the_camera_fill() {
    // c2's mic separated onto its own track. It plays from its layers, and c2's own mix of it is muted; it still
    // has its runs, so c2 has no camera sound at all.
    let p = gapped(500.0);
    let detach = Op::DetachSource {
        clip_id: "c2".into(),
        source_id: "mic".into(),
    };
    let q = klipolo_model::apply_ops(&p, &[detach]).unwrap();
    let c2: Vec<Heard> = heard(&q).into_iter().filter(|h| h.1 >= 90.0).collect();
    assert_eq!(
        c2,
        [line("mic", 90.0, 340.0, 1.0, 130.0, (0.0, 0.0))],
        "the mic's layer alone"
    );
}

#[test]
fn muting_the_mic_in_a_clip_stays_silent() {
    // A mute is the person's choice, so it means silence: c2 has neither the mic nor the camera.
    let mute = Op::SetClipSourceMix {
        clip_id: "c2".into(),
        source_id: "mic".into(),
        mix: Some(SourceMix {
            muted: true,
            ..SourceMix::default()
        }),
    };
    let q = klipolo_model::apply_ops(&gapped(500.0), &[mute]).unwrap();
    assert!(heard(&q).iter().all(|h| h.2 <= 90.0), "{:?}", heard(&q));
}
