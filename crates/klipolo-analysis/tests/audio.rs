//! The 16 kHz mono WAV for Voice Transcribe: format, length, the mono mix and the decimation filter. The fixture tests
//! need `scripts/gen-engine-fixtures.sh` (the gate runs them with `--ignored`); the rest make their own 48 kHz WAVs.

#[path = "../../klipolo-engine/tests/support/fixture_set.rs"]
mod fixture_set;

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use klipolo_analysis::audio::{self, AudioInfo, RATE_16K};

fn fixture(name: &str) -> PathBuf {
    let path = fixture_set::fixture_dir().join(name);
    assert!(
        path.exists(),
        "{} is missing: run scripts/gen-engine-fixtures.sh",
        path.display()
    );
    path
}

/// A 16-bit PCM WAV of interleaved samples.
fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[i16]) {
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2 * u32::from(channels)).to_le_bytes());
    bytes.extend_from_slice(&(2 * channels).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A 32-bit float WAV, one channel.
fn write_wav_f32(path: &Path, rate: u32, samples: &[f32]) {
    let data = (samples.len() * 4) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// `seconds` of a sine at `hz` and `amplitude` (0-1), 48 kHz, one channel.
fn tone(hz: f64, amplitude: f64, seconds: f64) -> Vec<i16> {
    let n = (seconds * 48_000.0) as usize;
    (0..n)
        .map(|i| (amplitude * (2.0 * PI * hz * i as f64 / 48_000.0).sin() * 32767.0).round() as i16)
        .collect()
}

/// The WAV header's fields, checked against a mono 16-bit 16 kHz PCM file whose data runs to the end.
fn check_header(wav: &[u8]) {
    assert!(wav.len() >= 44, "{} bytes", wav.len());
    let u16_at = |i: usize| u16::from_le_bytes([wav[i], wav[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(wav[i..i + 4].try_into().unwrap());
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(u32_at(4) as usize, wav.len() - 8);
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(u32_at(16), 16);
    assert_eq!(u16_at(20), 1, "PCM");
    assert_eq!(u16_at(22), 1, "mono");
    assert_eq!(u32_at(24), 16_000);
    assert_eq!(u32_at(28), 32_000, "byte rate");
    assert_eq!(u16_at(32), 2, "block align");
    assert_eq!(u16_at(34), 16, "bits");
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32_at(40) as usize, wav.len() - 44);
}

/// RMS (0-1) of `samples[from_s..to_s]` at 16 kHz.
fn rms(samples: &[i16], from_s: f64, to_s: f64) -> f64 {
    let (a, b) = ((from_s * 16_000.0) as usize, (to_s * 16_000.0) as usize);
    let part = &samples[a..b];
    let power = part
        .iter()
        .map(|&s| (f64::from(s) / 32767.0).powi(2))
        .sum::<f64>()
        / part.len() as f64;
    power.sqrt()
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// Extracts `src` and returns what it reported with the WAV's samples (the header checked).
fn extract(src: &Path) -> (AudioInfo, Vec<i16>) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sound.wav");
    let info = audio::extract_wav_16k(src, &out).unwrap();
    let wav = std::fs::read(&out).unwrap();
    check_header(&wav);
    let pcm = audio::read_pcm(&wav).unwrap();
    assert_eq!(pcm.rate, RATE_16K);
    assert_eq!(
        info.duration_us,
        pcm.samples.len() as i64 * 1_000_000 / 16_000,
        "the reported length is the WAV's"
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        ["sound.wav"],
        "no temporary file is left beside it"
    );
    (info, pcm.samples)
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn extracts_16k_mono_with_right_duration() {
    // 4 s of 1080p30 with mono AAC: a 1 kHz beep (ffmpeg's sine, amplitude 1/8, -18 dBFS) for 50 ms every second.
    let (info, samples) = extract(&fixture("bars-30fps-0.5gop.mp4"));
    assert!(info.has_audio);
    assert!(
        (info.duration_us - 4_000_000).abs() <= 30_000,
        "{} µs",
        info.duration_us
    );
    assert!(
        (-19.5..=-16.5).contains(&info.peak_dbfs),
        "{} dBFS",
        info.peak_dbfs
    );
    // Each beep lands at its second, at the source's level, with silence after it.
    for k in 1..4 {
        let k = f64::from(k);
        let beep = db(rms(&samples, k + 0.01, k + 0.04));
        let after = db(rms(&samples, k + 0.3, k + 0.6));
        assert!(
            (beep - (-18.0 - 3.01)).abs() < 1.5,
            "beep at {k} s: {beep:.1} dB"
        );
        assert!(after < -60.0, "after the beep at {k} s: {after:.1} dB");
    }

    // 2 s of stereo AAC: ffmpeg's sine made stereo, which puts it in each channel 3 dB down (-21 dBFS peak). One channel
    // out, at each channel's level: averaged, not summed (that would be 6 dB up).
    let (info, samples) = extract(&fixture("lead-40ms.mp4"));
    assert!(info.has_audio);
    assert!(
        (info.duration_us - 2_000_000).abs() <= 50_000,
        "{} µs",
        info.duration_us
    );
    assert!(
        (-22.5..=-19.5).contains(&info.peak_dbfs),
        "{} dBFS",
        info.peak_dbfs
    );
    let level = db(rms(&samples, 0.5, 1.5));
    assert!((level - (-21.0 - 3.01)).abs() < 1.0, "{level:.1} dB");
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn no_audio_track_reports_has_audio_false() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sound.wav");
    let info = audio::extract_wav_16k(&fixture("camera-720p.mp4"), &out).unwrap();
    assert!(!info.has_audio);
    assert_eq!(info.duration_us, 0);
    assert_eq!(info.peak_dbfs, audio::SILENCE_DBFS);
    assert!(!out.exists(), "no WAV for a clip without sound");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_missing_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sound.wav");
    assert!(audio::extract_wav_16k(&dir.path().join("gone.mp4"), &out).is_err());
    assert!(!out.exists());
}

#[test]
fn a_1_khz_tone_passes_within_half_a_db() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("1k.wav");
    write_wav(&src, 48_000, 1, &tone(1_000.0, 0.5, 3.0));
    let (info, samples) = extract(&src);
    assert_eq!(
        info.duration_us, 3_000_000,
        "48 000 × 3 frames make 16 000 × 3 samples"
    );
    let gain = db(rms(&samples, 0.5, 2.5) / (0.5 / 2f64.sqrt()));
    assert!(gain.abs() <= 0.5, "1 kHz: {gain:.3} dB");
    assert!(
        (info.peak_dbfs - db(0.5) as f32).abs() < 0.5,
        "{} dBFS",
        info.peak_dbfs
    );
}

#[test]
fn a_9_khz_tone_is_cut_by_at_least_50_db() {
    // Above the 8 kHz Nyquist of 16 kHz: left in, it would fold down to 7 kHz.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("9k.wav");
    write_wav(&src, 48_000, 1, &tone(9_000.0, 0.5, 3.0));
    let (_, samples) = extract(&src);
    let gain = db(rms(&samples, 0.5, 2.5) / (0.5 / 2f64.sqrt()));
    assert!(gain <= -50.0, "9 kHz: {gain:.1} dB");
}

#[test]
fn stereo_is_averaged_to_mono() {
    // A 1 kHz tone on the left only: half its level in the mono mix.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("left.wav");
    let left = tone(1_000.0, 0.5, 2.0);
    let interleaved: Vec<i16> = left.iter().flat_map(|&l| [l, 0]).collect();
    write_wav(&src, 48_000, 2, &interleaved);
    let (info, samples) = extract(&src);
    assert_eq!(info.duration_us, 2_000_000);
    let level = db(rms(&samples, 0.5, 1.5) / (0.5 / 2f64.sqrt()));
    assert!((level - db(0.5)).abs() <= 0.5, "{level:.2} dB");
}

#[test]
fn over_full_scale_is_clipped_not_wrapped() {
    // A float WAV's 1 kHz sine at 1.5 times full scale: its tops clip at ±32767, they never wrap round to the other sign.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("loud.wav");
    let loud: Vec<f32> = (0..96_000)
        .map(|i| (1.5 * (2.0 * PI * 1_000.0 * f64::from(i) / 48_000.0).sin()) as f32)
        .collect();
    write_wav_f32(&src, 48_000, &loud);
    let (info, samples) = extract(&src);
    assert_eq!(info.peak_dbfs, 0.0);
    assert_eq!(samples.iter().max(), Some(&32767));
    assert_eq!(samples.iter().min(), Some(&-32767));
    for (j, &s) in samples.iter().enumerate().skip(1_000).take(30_000) {
        let expected = 1.5 * (2.0 * PI * 1_000.0 * j as f64 / 16_000.0).sin();
        if expected > 0.5 {
            assert!(
                s > 16_000,
                "sample {j}: {s}, expected about {expected:.2} of full scale"
            );
        } else if expected < -0.5 {
            assert!(
                s < -16_000,
                "sample {j}: {s}, expected about {expected:.2} of full scale"
            );
        }
    }
}

#[test]
fn silence_reports_the_floor() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("silence.wav");
    write_wav(&src, 48_000, 1, &vec![0; 48_000]);
    let (info, samples) = extract(&src);
    assert!(info.has_audio);
    assert_eq!(info.duration_us, 1_000_000);
    assert_eq!(info.peak_dbfs, audio::SILENCE_DBFS);
    assert!(samples.iter().all(|&s| s == 0));
}

/// A cancel stops the extraction between decoded blocks: an error, and no WAV (nor its temporary file) left.
#[test]
fn a_cancelled_extraction_stops_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("tone.wav");
    write_wav(&src, 48_000, 1, &tone(1_000.0, 0.5, 3.0));
    let out = dir.path().join("sound.wav");
    let cancel = std::sync::atomic::AtomicBool::new(true);

    let stopped = audio::extract_wav_16k_cancellable(&src, &out, &cancel);

    assert!(
        matches!(stopped, Err(audio::AudioError::Stopped)),
        "{stopped:?}"
    );
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, [std::ffi::OsString::from("tone.wav")]);
}

/// A paced extraction calls `between` between decoded blocks, where a wait holds the decode where it is (playback goes
/// first): it goes on afterwards to the very WAV an unpaced one writes. A cancel set while it waits stops it as a cancel
/// does, leaving nothing.
#[test]
fn a_paced_extraction_waits_between_blocks_and_writes_the_same_wav() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("tone.wav");
    write_wav(&src, 48_000, 1, &tone(1_000.0, 0.5, 3.0));
    let (plain, samples) = extract(&src);
    let out = dir.path().join("paced.wav");
    let mut waits = 0;
    let paced = audio::extract_wav_16k_paced(
        &src,
        &out,
        &std::sync::atomic::AtomicBool::new(false),
        &mut || {
            waits += 1;
            std::thread::sleep(std::time::Duration::from_millis(1));
        },
    )
    .unwrap();
    assert!(waits > 1, "between blocks, not once: {waits}");
    assert_eq!(paced, plain);
    let pcm = audio::read_pcm(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(pcm.samples, samples, "the same sound");

    let cancel = std::sync::atomic::AtomicBool::new(false);
    let stopped_out = dir.path().join("stopped.wav");
    let stopped = audio::extract_wav_16k_paced(&src, &stopped_out, &cancel, &mut || {
        cancel.store(true, std::sync::atomic::Ordering::Relaxed)
    });
    assert!(
        matches!(stopped, Err(audio::AudioError::Stopped)),
        "{stopped:?}"
    );
    assert!(!stopped_out.exists());
    let left = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(left, 2, "the source and the paced WAV, no temporary file");
}
