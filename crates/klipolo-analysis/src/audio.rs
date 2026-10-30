//! Any clip's sound as the WAV Meta's Voice Transcribe takes: mono, 16-bit PCM, 16 kHz.
//!
//! The engine's streaming decode (`klipolo_engine::audio::decode`) hands out 48 kHz f32 blocks (mono stays mono, more
//! channels arrive as stereo). Each block is mixed to mono (the channels averaged), low-passed and kept one sample in
//! three (a windowed-sinc FIR, `Decimator`), rounded to 16 bits with clipping and no dither, and written behind a
//! standard 44-byte header. Nothing holds more than one decoded block and the filter's 95 samples of history, so an
//! hour-long clip takes no more memory than a short one. No FFmpeg calls of our own.
//!
//! Times: the engine's decode counts from the sound stream's first sample, so the WAV (and every time measured in it:
//! its chunks, Voice Transcribe's and the aligner's word times) is in the clip's SOUND time. The clip's file time (its
//! picture's) is sound time + `Asset::lead_us()`. A transcript's `Word` times stay in sound time: the model's captions
//! and `speech::voice_runs` expect sound time and add the lead themselves.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;

use klipolo_engine::audio::decode::{self, DecodeError};
use klipolo_engine::render::probe::probe_media;
use klipolo_model::time::Us;
use klipolo_platform::durable::{self, Durability};
use serde::{Deserialize, Serialize};

/// What Voice Transcribe takes.
pub const RATE_16K: u32 = 16_000;

/// `AudioInfo::peak_dbfs` of digital silence (and of a clip with no sound): a finite floor, so it stays a number in
/// JSON.
pub const SILENCE_DBFS: f32 = -120.0;

/// The engine's decode rate.
const RATE_48K: u32 = 48_000;
const FACTOR: usize = (RATE_48K / RATE_16K) as usize;

/// What `extract_wav_16k` wrote. Its times are the clip's sound time (see the module notes).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    /// The WAV's length (its samples at 16 kHz), from the sound's first sample; 0 without sound.
    pub duration_us: Us,
    /// False when the clip has no sound stream: then no WAV is written.
    pub has_audio: bool,
    /// The loudest sample of the WAV, dB below full scale (0 when it clips); `SILENCE_DBFS` when it is all zeros.
    pub peak_dbfs: f32,
}

/// Why a clip's sound couldn't be written or read. The messages carry no file path.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("Could not read the sound: {0}")]
    Decode(String),
    #[error("Could not write the sound file: {0}")]
    Io(#[from] io::Error),
    #[error("The sound is too long for one WAV file")]
    TooLong,
    #[error("This is not a 16-bit mono WAV")]
    NotWav,
}

/// Writes `src`'s sound to `out` as a mono 16-bit PCM WAV at 16 kHz, its sample 0 the sound stream's first sample (sound
/// time: file time minus `Asset::lead_us()`). A clip with no sound stream writes nothing and reports `has_audio: false`
/// (not an error). The WAV is written beside `out` under a temporary name (`<name>.<pid>.tmp`) and renamed into place
/// once whole, so `out` is never half-written; a failure removes the temporary file. Calls running at once must each
/// have their own `out`: two writing one `out` from one process would share its temporary file. A process killed
/// mid-write leaves the temporary file behind (extract into a temporary folder, or sweep `*.tmp`).
pub fn extract_wav_16k(src: &Path, out: &Path) -> Result<AudioInfo, AudioError> {
    if !probe_media(src).map_err(AudioError::Decode)?.has_audio {
        return Ok(AudioInfo {
            duration_us: 0,
            has_audio: false,
            peak_dbfs: SILENCE_DBFS,
        });
    }
    let channels = usize::from(
        decode::probe(src)
            .map_err(AudioError::Decode)?
            .channels
            .max(1),
    );
    let tmp = durable::temp_path(out);
    let written = write_wav(src, channels, &tmp).and_then(|info| {
        std::fs::rename(&tmp, out)?;
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            durable::fsync_dir(dir)?;
        }
        Ok(info)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Decodes `src` into the WAV at `path` (the temporary file).
fn write_wav(src: &Path, channels: usize, path: &Path) -> Result<AudioInfo, AudioError> {
    let mut sink = WavSink::create(path)?;
    let mut decimator = Decimator::new();
    let mut mono = Vec::new();
    let mut out = Vec::new();
    // The sink's error, kept here: the engine's `emit` can only pass a String back.
    let mut failed: Option<AudioError> = None;
    let mut emit = |_first: u64, samples: &[f32]| -> Result<bool, String> {
        mono.clear();
        if channels == 1 {
            mono.extend_from_slice(samples);
        } else {
            let scale = 1.0 / channels as f32;
            mono.extend(
                samples
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() * scale),
            );
        }
        out.clear();
        decimator.push(&mono, &mut out);
        match sink.write(&out) {
            Ok(()) => Ok(true),
            Err(e) => {
                failed = Some(e);
                Err("write".into())
            }
        }
    };
    let decoded = decode::decode(src, 0, None, &mut emit);
    if let Some(e) = failed {
        return Err(e);
    }
    decoded.map_err(|e| match e {
        DecodeError::Unreadable(e) | DecodeError::Failed(e) => AudioError::Decode(e),
        DecodeError::CantSeek => AudioError::Decode("Seek".into()),
    })?;
    let mut tail = Vec::new();
    decimator.finish(&mut tail);
    sink.write(&tail)?;
    sink.finish()
}

/// Writes 16-bit mono samples behind a header whose sizes are filled in at the end.
struct WavSink {
    file: BufWriter<File>,
    samples: u64,
    peak: u16,
}

impl WavSink {
    fn create(path: &Path) -> Result<Self, AudioError> {
        let mut file = BufWriter::with_capacity(1 << 16, File::create(path)?);
        file.write_all(&wav_header(RATE_16K, 1, 0))?;
        Ok(Self {
            file,
            samples: 0,
            peak: 0,
        })
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        self.samples += samples.len() as u64;
        if 36 + self.samples * 2 > u64::from(u32::MAX) {
            return Err(AudioError::TooLong);
        }
        for &s in samples {
            let q = quantize(s);
            self.peak = self.peak.max(q.unsigned_abs());
            self.file.write_all(&q.to_le_bytes())?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<AudioInfo, AudioError> {
        let data = (self.samples * 2) as u32;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&wav_header(RATE_16K, 1, data))?;
        let file = self.file.into_inner().map_err(|e| e.into_error())?;
        durable::sync(&file, Durability::Ordered)?;
        let peak_dbfs = if self.peak == 0 {
            SILENCE_DBFS
        } else {
            (20.0 * (f64::from(self.peak) / 32767.0).log10()) as f32
        };
        Ok(AudioInfo {
            duration_us: (self.samples * 1_000_000 / u64::from(RATE_16K)) as Us,
            has_audio: true,
            peak_dbfs,
        })
    }
}

/// -1..1 to 16 bits: rounded, clipped at ±32767, no dither.
fn quantize(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// The low-pass FIR's length: odd, so its centre tap sits on an output sample and the output keeps the input's
/// timing (no delay to undo).
const TAPS: usize = 95;
// At least 63 (the stopband the test below checks needs about 90).
const _: () = assert!(TAPS >= 63 && TAPS % 2 == 1);
const HALF: usize = (TAPS - 1) / 2;
/// Where the gain is half (-6 dB): below the 8 kHz Nyquist of 16 kHz, leaving room for the transition.
const CUTOFF_HZ: f64 = 7_000.0;
/// Kaiser β for about 72 dB of stopband attenuation (β = 0.1102 (A - 8.7)); with 95 taps the transition runs from about
/// 5.9 to 8.1 kHz, so nothing above 8.1 kHz folds back by more than -60 dB.
const KAISER_BETA: f64 = 7.0;

/// The windowed-sinc low-pass at 48 kHz, its gain at DC exactly 1.
fn lowpass() -> [f32; TAPS] {
    let fc = CUTOFF_HZ / f64::from(RATE_48K);
    let mut h = [0f64; TAPS];
    for (n, tap) in h.iter_mut().enumerate() {
        let x = n as f64 - HALF as f64;
        let sinc = if x == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
        };
        let r = x / HALF as f64;
        let window =
            bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(KAISER_BETA);
        *tap = sinc * window;
    }
    let sum: f64 = h.iter().sum();
    h.map(|t| (t / sum) as f32)
}

/// The modified Bessel function of the first kind, order 0 (its power series; converges fast for the β used).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}

/// 48 kHz mono in, 16 kHz out: output sample m is the low-passed input centred on input sample 3m (input before the
/// start and after the end counts as silence), so n input samples make ⌈n / 3⌉ output samples.
struct Decimator {
    taps: [f32; TAPS],
    /// Input from absolute index `base` on (the first `HALF` are the silence before the start).
    buf: Vec<f32>,
    base: i64,
    /// The input index the next output is centred on.
    next: i64,
}

impl Decimator {
    fn new() -> Self {
        Self {
            taps: lowpass(),
            buf: vec![0.0; HALF],
            base: -(HALF as i64),
            next: 0,
        }
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        self.drain(out);
    }

    /// The outputs the last input still owes: with `HALF` samples of silence after the end, every output centred on
    /// an input sample (and none past it) has its input.
    fn finish(&mut self, out: &mut Vec<f32>) {
        self.buf.extend(std::iter::repeat_n(0.0, HALF));
        self.drain(out);
    }

    /// Every output whose input is all in the buffer, then drops the input no later output needs.
    fn drain(&mut self, out: &mut Vec<f32>) {
        let end = self.base + self.buf.len() as i64;
        while self.next + (HALF as i64) < end {
            let from = (self.next - HALF as i64 - self.base) as usize;
            let window = &self.buf[from..from + TAPS];
            out.push(window.iter().zip(&self.taps).map(|(x, h)| x * h).sum());
            self.next += FACTOR as i64;
        }
        let keep_from =
            (self.next - HALF as i64 - self.base).clamp(0, self.buf.len() as i64) as usize;
        self.buf.drain(..keep_from);
        self.base += keep_from as i64;
    }
}

/// The 44-byte header of a PCM WAV holding `data_len` bytes of 16-bit samples.
pub(crate) fn wav_header(rate: u32, channels: u16, data_len: u32) -> [u8; 44] {
    let block_align = 2 * channels;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

/// Where a 16-bit PCM WAV's samples are.
pub(crate) struct Layout {
    pub rate: u32,
    pub channels: u16,
    /// The sample bytes (whole frames, within the file).
    pub data: Range<usize>,
}

/// Reads a RIFF WAVE's `fmt ` and `data` chunks (any others are skipped). None unless it is 16-bit integer PCM.
pub(crate) fn layout(wav: &[u8]) -> Option<Layout> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_le_bytes(wav.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(wav.get(i..i + 4)?.try_into().ok()?));
    let mut at = 12;
    let mut format = None;
    while at + 8 <= wav.len() {
        let id = &wav[at..at + 4];
        let len = u32_at(at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            let (tag, channels, rate, bits) = (
                u16_at(body)?,
                u16_at(body + 2)?,
                u32_at(body + 4)?,
                u16_at(body + 14)?,
            );
            if tag != 1 || bits != 16 || channels == 0 || rate == 0 {
                return None;
            }
            format = Some((rate, channels));
        } else if id == b"data" {
            let (rate, channels) = format?;
            let frame = 2 * usize::from(channels);
            // A streamed file may state more than it holds (or 0xFFFFFFFF): only what is there counts.
            let end = body.saturating_add(len).min(wav.len());
            let end = body + (end - body) / frame * frame;
            return Some(Layout {
                rate,
                channels,
                data: body..end,
            });
        }
        // Chunks are padded to an even length.
        at = body.checked_add(len)?.checked_add(len & 1)?;
    }
    None
}

/// A mono 16-bit WAV's samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonoPcm {
    pub rate: u32,
    pub samples: Vec<i16>,
}

/// The samples of a mono 16-bit PCM WAV (what `extract_wav_16k` writes), for `chunks::plan`.
pub fn read_pcm(wav: &[u8]) -> Result<MonoPcm, AudioError> {
    let layout = layout(wav)
        .filter(|l| l.channels == 1)
        .ok_or(AudioError::NotWav)?;
    let samples = wav[layout.data]
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok(MonoPcm {
        rate: layout.rate,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// |H(f)| of the taps at `hz` (48 kHz), in dB.
    fn response_db(taps: &[f32], hz: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * hz / f64::from(RATE_48K);
        let (re, im) = taps
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (n, &h)| {
                let phase = w * n as f64;
                (
                    re + f64::from(h) * phase.cos(),
                    im - f64::from(h) * phase.sin(),
                )
            });
        20.0 * (re * re + im * im).sqrt().log10()
    }

    #[test]
    fn the_filter_passes_speech_and_stops_what_would_fold_back() {
        let taps = lowpass();
        assert!((response_db(&taps, 0.0)).abs() < 1e-4, "unity gain at DC");
        for hz in (100..=5_000).step_by(100) {
            let db = response_db(&taps, f64::from(hz));
            assert!(db.abs() < 0.1, "{hz} Hz: {db:.3} dB");
        }
        let half = response_db(&taps, CUTOFF_HZ);
        assert!((half + 6.02).abs() < 0.5, "cutoff: {half:.2} dB");
        for hz in (8_400..=24_000).step_by(50) {
            let db = response_db(&taps, f64::from(hz));
            assert!(db <= -60.0, "{hz} Hz: {db:.1} dB");
        }
    }

    #[test]
    fn decimation_keeps_timing_and_length() {
        // An impulse at input 300 comes out centred on output 100, and n inputs make ⌈n / 3⌉ outputs.
        for n in [0usize, 1, 2, 3, 299, 300, 301, 1_000] {
            let mut input = vec![0f32; n];
            if n > 300 {
                input[300] = 1.0;
            }
            let mut d = Decimator::new();
            let mut out = Vec::new();
            // In uneven blocks, as the decoder hands them out.
            for block in input.chunks(7) {
                d.push(block, &mut out);
            }
            d.finish(&mut out);
            assert_eq!(out.len(), n.div_ceil(3), "{n} inputs");
            if n > 300 {
                let loudest = (0..out.len())
                    .max_by(|&a, &b| out[a].total_cmp(&out[b]))
                    .unwrap();
                assert_eq!(loudest, 100);
            }
        }
    }

    #[test]
    fn decimating_in_blocks_matches_one_block() {
        // However the decoder splits the sound, the output is the same, bit for bit.
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let input: Vec<f32> = (0..20_000)
            .map(|_| {
                x = x
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
            })
            .collect();
        let run = |block: usize| {
            let mut d = Decimator::new();
            let mut out = Vec::new();
            for part in input.chunks(block) {
                d.push(part, &mut out);
            }
            d.finish(&mut out);
            out.iter().map(|s| s.to_bits()).collect::<Vec<u32>>()
        };
        let whole = run(input.len());
        assert_eq!(whole.len(), input.len().div_ceil(3));
        for block in [1, 2, 7, 4096] {
            assert!(run(block) == whole, "blocks of {block}");
        }
    }

    #[test]
    fn the_buffer_stays_small() {
        let mut d = Decimator::new();
        let mut out = Vec::new();
        for _ in 0..1_000 {
            d.push(&[0.25; 1024], &mut out);
            assert!(d.buf.len() <= TAPS + FACTOR, "{}", d.buf.len());
        }
    }

    #[test]
    fn quantizing_rounds_and_clips() {
        assert_eq!(quantize(0.0), 0);
        assert_eq!(quantize(1.0), 32767);
        assert_eq!(quantize(-1.0), -32767);
        assert_eq!(quantize(3.0), 32767);
        assert_eq!(quantize(-3.0), -32767);
        assert_eq!(quantize(0.5 / 32767.0 + 1e-6), 1);
        assert_eq!(quantize(f32::NAN), 0);
    }

    #[test]
    fn a_wav_with_other_chunks_reads_by_its_data_chunk() {
        let mut wav = wav_header(16_000, 1, 4).to_vec();
        // A LIST chunk (odd length, padded) between fmt and data.
        let list = [b'L', b'I', b'S', b'T', 3, 0, 0, 0, b'a', b'b', b'c', 0];
        wav.splice(36..36, list);
        wav.extend_from_slice(&[1, 0, 2, 0]);
        let pcm = read_pcm(&wav).unwrap();
        assert_eq!(
            pcm,
            MonoPcm {
                rate: 16_000,
                samples: vec![1, 2]
            }
        );
        assert!(
            matches!(read_pcm(&wav_header(16_000, 2, 0)), Err(AudioError::NotWav)),
            "stereo"
        );
        assert!(matches!(read_pcm(b"RIFF"), Err(AudioError::NotWav)));
    }
}
