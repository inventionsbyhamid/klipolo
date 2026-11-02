//! Volume keys set by hand (`Clip::volume_keys`, a sound layer's): the plan carries each clip's or layer's line from its
//! start, the mixer follows it sample by sample, and export plays what the preview plays.
mod support;

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use klipolo_engine::api::ExportQuality;
use klipolo_engine::audio::mix::{Mixdown, RATE, finish};
use klipolo_engine::audio::pcm::{Pcm, PcmKey, PcmStore};
use klipolo_engine::audio::plan::{AudioPlan, AudioSource, audio_plan};
use klipolo_engine::export::audio::{CEILING_DBTP, ExportAudio};
use klipolo_engine::registry::Registry;
use klipolo_model::motion::Key;
use klipolo_model::project::{Loudness, Project};
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::{ItemContent, Track, TrackKind};
use support::*;

/// The tone's peak: at the levels here, far under the soft clip's knee and the limiter's ceiling.
const AMPLITUDE: f32 = 0.25;
const TONE_HZ: f32 = 1_000.0;
const HEARD_S: f64 = 12.0;

fn key(t: f64, v: f64) -> Key {
    Key {
        t_us: secs(t),
        v,
        ease: None,
    }
}

fn no_speech() -> Arc<[RangeUs]> {
    Arc::from(Vec::new())
}

fn tone_at(n: usize) -> f32 {
    AMPLITUDE * (2.0 * std::f32::consts::PI * TONE_HZ * n as f32 / RATE as f32).sin()
}

/// The engine fixture with every clip muted and a music bed at 2 s, 10 s long, at 80 %, whose line goes from full to
/// silent between 2 s and 6 s into it and back up to half by 8 s: with keys, the line is its level.
fn with_line() -> Project {
    let mut p = project();
    for clip in &mut p.timeline.main {
        clip.muted = true;
    }
    let mut track = Track::new(TrackKind::Audio, "Music");
    track.id = "music-track".into();
    track.items.push(item(
        "bed",
        2.0,
        10.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 0.8,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
            volume_keys: vec![key(2.0, 1.0), key(6.0, 0.0), key(8.0, 0.5)],
        },
    ));
    p.timeline.tracks.push(track);
    p.audio.loudness = Loudness::Off;
    klipolo_model::validate(&p).unwrap();
    p
}

#[test]
fn a_clip_s_keys_plan_its_line_from_the_clip_s_start() {
    let mut p = project();
    p.timeline.main[1].volume_keys = vec![key(1.0, 0.0), key(3.0, 1.5)];
    p.timeline.main[1].volume = 0.3;
    let plan = audio_plan(&p, &Registry::builtin(), no_speech());
    let b = plan
        .sources
        .iter()
        .find(|s| s.path.ends_with("broll"))
        .unwrap();
    let line = b.line.as_ref().expect("the clip's line");
    assert_eq!(line.origin_us, secs(20.0));
    assert_eq!(
        b.gain, 1.0,
        "the line is the level: the volume waits for when it goes"
    );
    assert_eq!(b.level_at(secs(22.0)), 0.75);
    assert_eq!(b.level_at(secs(30.0)), 1.5);
    // A recording clip's sources each read the clip's line, at their mix's volume.
    p.timeline.main[0].volume_keys = vec![key(0.0, 0.5)];
    let plan = audio_plan(&p, &Registry::builtin(), no_speech());
    let mic = plan
        .sources
        .iter()
        .find(|s| s.path.ends_with("mic"))
        .unwrap();
    assert_eq!(
        (
            mic.line.as_ref().unwrap().origin_us,
            mic.level_at(secs(5.0))
        ),
        (0, 0.5)
    );
}

#[test]
fn a_silent_line_plays_nothing_and_a_line_over_no_volume_plays() {
    let mut p = project();
    p.timeline.main[1].volume_keys = vec![key(1.0, 0.0), key(3.0, 0.0)];
    let plan = audio_plan(&p, &Registry::builtin(), no_speech());
    assert!(!plan.sources.iter().any(|s| s.path.ends_with("broll")));
    p.timeline.main[1].volume = 0.0;
    p.timeline.main[1].volume_keys = vec![key(1.0, 0.2)];
    let plan = audio_plan(&p, &Registry::builtin(), no_speech());
    assert!(plan.sources.iter().any(|s| s.path.ends_with("broll")));
}

/// The mix from frame 0 as playback renders it: one `Mixdown`, block after block, finished at unity.
fn render(plan: AudioPlan, pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>) -> Vec<f32> {
    const BLOCK: usize = 1_024;
    let frames = (HEARD_S * f64::from(RATE)) as usize;
    let mut mixdown = Mixdown::new(Arc::new(plan));
    let mut out = vec![0f32; 2 * frames];
    for (k, block) in out.chunks_mut(2 * BLOCK).enumerate() {
        mixdown.render(pcm, (k * BLOCK) as u64, block);
        finish(block, 1.0, 1.0);
    }
    out
}

/// The left channel's RMS over `[from, to)` seconds, as a share of the tone's own.
fn level(mix: &[f32], from: f64, to: f64) -> f64 {
    let frame = |s: f64| (s * f64::from(RATE)).round() as usize;
    let (a, b) = (frame(from), frame(to));
    let power: f64 = mix[2 * a..2 * b]
        .chunks_exact(2)
        .map(|lr| f64::from(lr[0]).powi(2))
        .sum::<f64>()
        / (b - a) as f64;
    power.sqrt() / (f64::from(AMPLITUDE) / 2f64.sqrt())
}

#[test]
fn export_plays_the_line_as_the_preview_does() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("music.wav");
    write_tone(&wav, 30);
    let mut p = with_line();
    p.assets.iter_mut().find(|a| a.id == "music").unwrap().path = wav.display().to_string();
    let registry = Registry::builtin();
    let cache = dir.path().join("cache");
    let (export, levels) = ExportAudio::prepare(
        &p,
        &registry,
        ExportQuality::High,
        Some(&cache),
        None,
        no_speech(),
    )
    .unwrap();
    assert_eq!(levels.gain_db, 0.0, "loudness off: unity");
    let preview = audio_plan(&p, &registry, no_speech());
    assert_eq!(**export.plan(), preview);
    assert!(preview.sources.iter().any(|s| s.line.is_some()));
    let store = PcmStore::new(cache);
    let heard = render(preview, &|s: &AudioSource| store.build(&PcmKey::of(s)).ok());
    let mut stream = export.stream(CEILING_DBTP);
    let mut exported = vec![0f32; heard.len()];
    for block in exported.chunks_mut(2 * 1_024) {
        stream.next(block);
    }
    let worst = heard
        .iter()
        .zip(&exported)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(worst < 1e-3, "export differs from the preview by {worst}");
    // The line, not the 80 % volume: full before 4 s, half way down at 6 s, silent at 8 s, a quarter up at 9 s.
    for (from, to, want) in [
        (2.5, 3.5, 1.0),
        (5.95, 6.05, 0.5),
        (7.98, 8.02, 0.0),
        (8.95, 9.05, 0.25),
        (10.5, 11.5, 0.5),
    ] {
        let got = level(&exported, from, to);
        assert!(
            (got - want).abs() < 0.02,
            "{from} to {to} s: {got:.3}, not {want}"
        );
    }
}

fn write_tone(path: &Path, seconds: usize) {
    let frames = seconds * RATE as usize;
    let data = (frames * 4) as u32;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for part in [
        &b"RIFF"[..],
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &2u16.to_le_bytes(),
        &RATE.to_le_bytes(),
        &(RATE * 4).to_le_bytes(),
        &4u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ] {
        out.write_all(part).unwrap();
    }
    for n in 0..frames {
        let x = ((tone_at(n) * 32767.0).round() as i16).to_le_bytes();
        out.write_all(&x).unwrap();
        out.write_all(&x).unwrap();
    }
}
