//! Cutting a long recording into Voice Transcribe requests (at most 600 s each) at its silences, and the WAV of each.
//! The 25-minute recording is made here: bursts of tone (speech) between quiet gaps (room tone).

use std::f64::consts::PI;
use std::ops::Range;
use std::sync::OnceLock;

use klipolo_analysis::audio::{self, AudioError};
use klipolo_analysis::chunks::{self, Plan};
use klipolo_model::time::RangeUs;

const RATE: u32 = 16_000;

/// A small deterministic generator (no rand crate): uniform in [0, 1).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
}

/// 25 minutes at 16 kHz: bursts of 0.8-6 s (a tone of 150-900 Hz at 0.15-0.5 of full scale, its level wobbling four
/// times a second like syllables) between gaps of 0.4-2.5 s of room tone (±4 LSB of noise). Returns the samples and
/// the gaps (sample ranges).
fn talk() -> &'static (Vec<i16>, Vec<Range<usize>>) {
    static TALK: OnceLock<(Vec<i16>, Vec<Range<usize>>)> = OnceLock::new();
    TALK.get_or_init(|| {
        let total = 25 * 60 * RATE as usize;
        let mut rng = Lcg(0x6b6c_6970_6f6c_6f21);
        let mut pcm = Vec::with_capacity(total);
        let mut gaps = Vec::new();
        while pcm.len() < total {
            let n = (rng.between(0.8, 6.0) * f64::from(RATE)) as usize;
            let (hz, level) = (rng.between(150.0, 900.0), rng.between(0.15, 0.5));
            for i in 0..n {
                let t = i as f64 / f64::from(RATE);
                let wobble = 0.65 + 0.35 * (2.0 * PI * 4.0 * t).sin();
                pcm.push((level * wobble * (2.0 * PI * hz * t).sin() * 32767.0).round() as i16);
            }
            let n = (rng.between(0.4, 2.5) * f64::from(RATE)) as usize;
            gaps.push(pcm.len()..pcm.len() + n);
            for _ in 0..n {
                pcm.push((rng.between(-4.0, 4.0)).round() as i16);
            }
        }
        pcm.truncate(total);
        (pcm, gaps)
    })
}

fn sample_at(us: i64) -> usize {
    (us as i128 * i128::from(RATE) / 1_000_000) as usize
}

fn us_of(samples: usize) -> i64 {
    (samples as i128 * 1_000_000 / i128::from(RATE)) as i64
}

/// A mono 16-bit WAV of `pcm`.
fn wav_of(pcm: &[i16]) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + pcm.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    bytes
}

/// The chunks start at 0, each starts where the one before it ends, and the last ends at the end of the file.
fn assert_covers(chunks: &[RangeUs], samples: usize) {
    assert_eq!(chunks.first().map(|c| c.start), Some(0), "{chunks:?}");
    for pair in chunks.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "{chunks:?}");
    }
    assert_eq!(
        chunks.last().map(|c| c.end),
        Some(us_of(samples)),
        "{chunks:?}"
    );
    assert!(chunks.iter().all(|c| !c.is_empty()), "{chunks:?}");
}

#[test]
fn chunks_of_25_minutes_are_at_most_600s_and_cut_in_silence() {
    let (pcm, gaps) = talk();
    let Plan { chunks, loud_cuts } = chunks::plan_with_loud_cuts(pcm, RATE, 600);
    assert_eq!(
        chunks,
        chunks::plan(pcm, RATE, 600),
        "plan is the same plan's chunks"
    );
    assert_eq!(chunks.len(), 3, "{chunks:?}");
    assert!(
        loud_cuts.is_empty(),
        "every cut found room tone: {loud_cuts:?}"
    );
    for chunk in &chunks {
        assert!(chunk.len() <= 600_000_000, "{chunk:?}");
    }
    for chunk in &chunks[..chunks.len() - 1] {
        // Within the last minute before the limit, and the 300 ms around the cut is all room tone.
        assert!(chunk.len() >= 540_000_000, "{chunk:?}");
        let cut = sample_at(chunk.end);
        let half = (RATE * 3 / 20) as usize;
        assert!(
            gaps.iter()
                .any(|g| g.start + half <= cut && cut + half <= g.end),
            "the cut at {} µs is not inside a gap",
            chunk.end
        );
    }
}

#[test]
fn chunks_cover_the_whole_file_without_gaps_or_overlap() {
    let (pcm, _) = talk();
    let chunks = chunks::plan(pcm, RATE, 600);
    assert_covers(&chunks, pcm.len());
    assert_eq!(chunks.last().unwrap().end, 1_500_000_000);

    // Each chunk's WAV is a request Voice Transcribe takes (mono 16-bit 16 kHz, at most 32 MB), and together they
    // hold every sample once, in order.
    let wav = wav_of(pcm);
    let mut joined = Vec::with_capacity(pcm.len());
    for chunk in &chunks {
        let part = chunks::wav_slice(&wav, *chunk).unwrap();
        assert!(part.len() <= 32 * 1024 * 1024, "{} bytes", part.len());
        let read = audio::read_pcm(&part).unwrap();
        assert_eq!(read.rate, RATE);
        assert_eq!(
            read.samples.len(),
            sample_at(chunk.end) - sample_at(chunk.start)
        );
        assert_eq!(&part[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(part[4..8].try_into().unwrap()) as usize,
            part.len() - 8
        );
        joined.extend_from_slice(&read.samples);
    }
    assert!(joined == *pcm, "the chunks' samples are the file's");
}

#[test]
fn the_cut_goes_in_the_quietest_window_of_the_last_minute() {
    // 200 s of tone with three 1 s pauses: digital silence at 25 s (before the last minute of the first 100 s, so
    // never used), room tone at 55 s and digital silence at 75 s (the quietest in reach).
    let mut pcm: Vec<i16> = (0..200 * RATE as usize)
        .map(|i| {
            (0.3 * (2.0 * PI * 440.0 * i as f64 / f64::from(RATE)).sin() * 32767.0).round() as i16
        })
        .collect();
    let second = RATE as usize;
    let mut rng = Lcg(7);
    pcm[25 * second..26 * second].fill(0);
    for s in &mut pcm[55 * second..56 * second] {
        *s = rng.between(-20.0, 20.0).round() as i16;
    }
    pcm[75 * second..76 * second].fill(0);
    let Plan { chunks, loud_cuts } = chunks::plan_with_loud_cuts(&pcm, RATE, 100);
    assert_covers(&chunks, pcm.len());
    assert!(loud_cuts.is_empty(), "{loud_cuts:?}");
    let cut = chunks[0].end;
    assert!((75_150_000..=75_850_000).contains(&cut), "cut at {cut} µs");
    for chunk in &chunks {
        assert!(chunk.len() <= 100_000_000, "{chunk:?}");
    }
}

#[test]
fn sound_with_no_pause_in_reach_is_still_cut_and_reported() {
    // 30 s of silence, then 170 s of tone with no pause: the quiet start sets the floor, the cuts after it land in
    // sound, within the limit, and each is reported for the log.
    let pcm: Vec<i16> = (0..200 * RATE as usize)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            if t < 30.0 {
                0
            } else {
                (0.3 * (2.0 * PI * 440.0 * t).sin() * 32767.0).round() as i16
            }
        })
        .collect();
    let Plan { chunks, loud_cuts } = chunks::plan_with_loud_cuts(&pcm, RATE, 90);
    assert_covers(&chunks, pcm.len());
    // Every tone window is as quiet as the next, so where each cut lands (and how many there are) is not fixed.
    assert!(chunks.len() >= 3, "{chunks:?}");
    for chunk in &chunks {
        assert!(chunk.len() <= 90_000_000, "{chunk:?}");
    }
    let cuts: Vec<i64> = chunks[..chunks.len() - 1].iter().map(|c| c.end).collect();
    assert_eq!(loud_cuts, cuts);
}

#[test]
fn a_file_under_the_limit_is_one_chunk() {
    let pcm = vec![100i16; 30 * RATE as usize + 7];
    let plan = chunks::plan_with_loud_cuts(&pcm, RATE, 600);
    assert_eq!(
        plan.chunks,
        [RangeUs {
            start: 0,
            end: us_of(pcm.len())
        }]
    );
    assert!(plan.loud_cuts.is_empty());
    assert!(
        chunks::plan(&[], RATE, 600).is_empty(),
        "no sound, no request"
    );
}

#[test]
fn a_slice_past_the_end_stops_at_the_end_and_a_non_wav_is_an_error() {
    let pcm: Vec<i16> = (0..16_000).map(|i| i as i16).collect();
    let wav = wav_of(&pcm);
    let part = chunks::wav_slice(
        &wav,
        RangeUs {
            start: 500_000,
            end: 9_000_000,
        },
    )
    .unwrap();
    assert_eq!(audio::read_pcm(&part).unwrap().samples, &pcm[8_000..]);
    let range = RangeUs {
        start: 0,
        end: 1_000,
    };
    assert!(matches!(
        chunks::wav_slice(b"not a wav at all", range),
        Err(AudioError::NotWav)
    ));
    // A float WAV is a WAV, but not one Voice Transcribe takes.
    let mut float = wav.clone();
    float[20] = 3;
    assert!(matches!(
        chunks::wav_slice(&float, range),
        Err(AudioError::NotWav)
    ));
}
