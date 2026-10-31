//! Import's rules: every run kept inside its file, what is left out named, markers sorted, video without
//! drift, and the one-sided mic's seam.
#![cfg(feature = "synthetic")]
mod common;

use std::fs::OpenOptions;
use std::io::Write;

use common::fake::{manifest_with, probe_ok};
use klipolo_capture::english::english;
use klipolo_capture::import::{centred_channel, manifest_peaks, recording_from};
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest::{Manifest, ManifestRun};
use klipolo_capture::media::AudioFormat;
use klipolo_model::project::{Marker, MarkerKind, Project, SourceRole};
use klipolo_model::validate::validate;

fn marker(id: &str, s: f64) -> Marker {
    Marker {
        id: id.into(),
        at_us: (s * 1e6) as i64,
        kind: MarkerKind::Hotkey,
        label: None,
    }
}

#[test]
fn an_import_keeps_each_run_inside_its_file_and_names_what_it_leaves_out() {
    let (mut m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[(0, 60.0)]),
        ("camera", SourceRole::Camera, &[(0, 60.0)]),
        ("mic", SourceRole::Mic, &[(0, 20.0), (25_000_000, 35.0)]),
        ("computerSound", SourceRole::SystemAudio, &[]),
    ]);
    m.sources[0].runs[0].drift_ppm = 40.0; // a video run never carries drift
    m.sources[2].runs[0].drift_ppm = 50.0;
    m.markers = vec![marker("m2", 30.0), marker("m1", 10.0), marker("m3", 70.0)];
    // The computer's sound opened a run the manifest left out: its file has no measured length.
    let mut j = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.journal())
        .unwrap();
    for e in [
        Entry::RunStarted {
            source: "computerSound".into(),
            run: 1,
            file: "computerSound/run-01.caf".into(),
            offset_us: 0,
            video: None,
            audio: Some(AudioFormat {
                rate: 48_000,
                channels: 2,
            }),
        },
        Entry::RunEnded {
            source: "computerSound".into(),
            run: 1,
            end_us: 60_000_000,
        },
    ] {
        j.write_all(&journal::line(&e)).unwrap();
    }
    // The camera's file is a second short of its run, and the mic's second run doesn't probe.
    let mut ok = probe_ok();
    let mut probe = |path: &std::path::Path, name: &str| match name {
        "mic/run-02.caf" => Err("moov atom not found".to_string()),
        _ => ok(path, name).map(|mut a| {
            if name == "camera/run-01.mp4" {
                a.duration_us -= 1_000_000;
            }
            a
        }),
    };
    let (imported, notes) = recording_from(&m, &dir, &manifest_peaks, &mut probe).unwrap();
    let r = &imported.recording;
    assert_eq!(r.id, "take", "the session id, so a second import is seen");
    let source = |id: &str| r.source(id).unwrap();
    assert_eq!(source("screen").runs[0].drift_ppm, 0.0);
    assert_eq!(source("mic").runs[0].drift_ppm, 50.0);
    assert_eq!(
        source("camera").runs[0].duration_us,
        59_000_000,
        "inside its file"
    );
    assert_eq!(source("mic").runs.len(), 1);
    assert!(r.source("computerSound").is_none());
    let words: Vec<String> = notes.iter().map(english).collect();
    assert_eq!(
        words,
        [
            "Camera stopped for 1 s at 0:59. Everything else was recorded.",
            "Part of Mic (35 s at 0:25) couldn't be read, so it isn't in the edit. Its file is kept in the take's folder.",
            "Computer sound stopped for 60 s at 0:00. Everything else was recorded.",
        ]
    );
    let at: Vec<(&str, i64)> = r.markers.iter().map(|m| (m.id.as_str(), m.at_us)).collect();
    assert_eq!(
        at,
        [("m1", 10_000_000), ("m2", 30_000_000), ("m3", 60_000_000)],
        "sorted, inside the take"
    );
    assert_eq!(imported.assets[0].name, "screen/run-01.mp4");
    let mut p = Project::new("Recording");
    p.assets.extend(imported.assets.iter().cloned());
    p.recordings.push(r.clone());
    assert_eq!(
        validate(&p),
        Ok(()),
        "the app applies it in one transaction"
    );
    for key in ["mic/run-02.caf", "camera/run-01.mp4", "screen/run-01.mp4"] {
        assert!(dir.path(key).exists(), "{key} is kept");
    }
}

#[test]
fn a_mic_silent_on_one_side_plays_centred_from_the_other() {
    let peaks = |runs: &[&[f32]]| centred_channel(runs.iter().map(|p| Some(*p)));
    assert_eq!(
        peaks(&[&[0.5, 0.0005], &[0.4, 0.0]]),
        Some(0),
        "[voice, silence] on every run"
    );
    assert_eq!(peaks(&[&[0.0, 0.3]]), Some(1), "a mic on input 2");
    assert_eq!(peaks(&[&[0.5, 0.2]]), None, "stereo stays stereo");
    assert_eq!(peaks(&[&[0.5]]), None, "a mono file is already centred");
    assert_eq!(
        peaks(&[&[0.5, 0.0], &[0.4]]),
        Some(0),
        "a heal onto a one-channel mic plays centred either way"
    );
    assert_eq!(peaks(&[&[0.0, 0.0]]), None, "nothing to centre");
    assert_eq!(
        peaks(&[&[0.5, 0.0], &[0.0008, 0.0], &[0.4, 0.0]]),
        Some(0),
        "a run silent on both sides (a short reconnect, a quiet stretch) doesn't cancel it"
    );
    assert_eq!(
        peaks(&[&[0.5, 0.0], &[0.0, 0.5]]),
        None,
        "the live side moved"
    );
    assert_eq!(
        centred_channel([Some(&[0.5, 0.0][..]), None]),
        None,
        "a run without levels"
    );
}

/// Import plays a mic centred from its live channel when its other channel stayed silent in every run kept
/// (the take's per-run peaks), and as recorded when both carry sound. Any other source plays as recorded, one-sided or
/// not (a hard-panned demo was heard that way while recording).
#[test]
fn import_centres_a_sound_whose_other_channel_stayed_silent() {
    let (m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[(0, 60.0)]),
        ("mic", SourceRole::Mic, &[(0, 20.0), (25_000_000, 35.0)]),
        ("gameSound", SourceRole::GameAudio, &[(0, 60.0)]),
        ("computer", SourceRole::SystemAudio, &[(0, 60.0)]),
    ]);
    fn peaks(run: &ManifestRun) -> Option<&[f32]> {
        match run.file.as_str() {
            "mic/run-01.caf" => Some(&[0.6, 0.0004]),
            "mic/run-02.caf" => Some(&[0.5, 0.0]),
            "gameSound/run-01.caf" => Some(&[0.7, 0.6]),
            _ => Some(&[0.5, 0.0]),
        }
    }
    let (imported, _) = recording_from(&m, &dir, &peaks, &mut probe_ok()).unwrap();
    let centre = |id: &str| imported.recording.source(id).unwrap().mix.centre_from;
    assert_eq!(centre("mic"), Some(0), "[voice, silence] plays centred");
    assert_eq!(centre("gameSound"), None, "stereo plays as recorded");
    assert_eq!(
        centre("computer"),
        None,
        "a one-sided computer sound plays as it was heard"
    );
    assert_eq!(centre("screen"), None, "a picture source");
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    assert!(
        imported
            .recording
            .sources
            .iter()
            .all(|s| s.mix.centre_from.is_none()),
        "no peaks: as recorded"
    );
}

/// The recorder's own measure reaches import. A mic on input 2 of an interface left its first channel silent in
/// both runs, as the take's manifest (`channel_peaks`, from the journal's whole-run `Levels`) says: through
/// `manifest_peaks` it imports centred from its second channel. A stereo game sound and a run without levels play as
/// recorded.
#[test]
fn a_manifest_whose_mic_run_has_a_silent_second_channel_imports_centred() {
    let (mut m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[(0, 60.0)]),
        ("mic", SourceRole::Mic, &[(0, 20.0), (25_000_000, 35.0)]),
        ("gameSound", SourceRole::GameAudio, &[(0, 60.0)]),
        ("micTwo", SourceRole::Mic, &[(0, 60.0)]),
    ]);
    m.sources[1].runs[0].channel_peaks = Some(vec![0.0004, 0.6]);
    m.sources[1].runs[1].channel_peaks = Some(vec![0.0, 0.5]);
    m.sources[2].runs[0].channel_peaks = Some(vec![0.7, 0.6]);
    // As the app reads it: manifest.json, written and read back.
    let m: Manifest = serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    let centre = |id: &str| imported.recording.source(id).unwrap().mix.centre_from;
    assert_eq!(
        centre("mic"),
        Some(1),
        "[silence, voice] plays centred from its second channel"
    );
    assert_eq!(centre("gameSound"), None, "stereo plays as recorded");
    assert_eq!(centre("micTwo"), None, "no levels measured: as recorded");
}

/// A picture file with sound (a recorder that muxes it in) imports with its source muted, so its sound never
/// doubles the take's mic; a silent picture file and the mic play as before.
#[test]
fn a_picture_file_with_sound_imports_muted() {
    let (m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[(0, 60.0)]),
        ("camera", SourceRole::Camera, &[(0, 60.0)]),
        ("mic", SourceRole::Mic, &[(0, 60.0)]),
    ]);
    let mut probe = probe_ok();
    // The screen's file carries sound; the camera's doesn't.
    let mut sounding = |path: &std::path::Path, name: &str| {
        probe(path, name).map(|mut a| {
            a.has_audio |= name.starts_with("screen/");
            a
        })
    };
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut sounding).unwrap();
    let muted = |id: &str| imported.recording.source(id).unwrap().mix.muted;
    assert!(muted("screen"), "a picture file with sound plays muted");
    assert!(!muted("camera"), "a silent picture file is left as it was");
    assert!(!muted("mic"));
}
