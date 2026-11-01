//! `refine_edges`: a recognizer's word edges, which sit on its 80 ms encoder steps, move onto the sound's own onsets
//! and offsets, at most one step, never across a neighbour. Synthetic 16 kHz signals only.

use klipolo_speech_wire::words::{refine_edges, RawWord};

const RATE: f64 = 16_000.0;

fn raw(text: &str, start: f64, end: f64) -> RawWord {
    RawWord {
        text: text.into(),
        start_us: (start * 1e6).round() as i64,
        end_us: (end * 1e6).round() as i64,
        confidence: 1.0,
    }
}

fn amplitude(dbfs: f64) -> f64 {
    10f64.powf(dbfs / 20.0) * std::f64::consts::SQRT_2 // a sine's peak for that RMS
}

/// `secs` of 16 kHz digital silence with a tone of `hz` at `dbfs` (RMS) over each range.
fn tones(secs: f64, ranges: &[(f64, f64)], hz: f64, dbfs: f64) -> Vec<f32> {
    let mut x = vec![0.0f32; (secs * RATE) as usize];
    for &(from, to) in ranges {
        let (a, b) = ((from * RATE).round() as usize, (to * RATE).round() as usize);
        for (n, s) in x[a..b].iter_mut().enumerate() {
            *s = (amplitude(dbfs) * (2.0 * std::f64::consts::PI * hz * n as f64 / RATE).sin())
                as f32;
        }
    }
    x
}

/// `secs` of silence with a 300 Hz tone at −20 dBFS over each range.
fn bursts(secs: f64, ranges: &[(f64, f64)]) -> Vec<f32> {
    tones(secs, ranges, 300.0, -20.0)
}

/// `secs` of pink noise at `db` dBFS (RMS), seeded: white noise from a fixed LCG through Paul Kellet's economy filter.
fn noise(secs: f64, db: f64) -> Vec<f32> {
    let mut state: u64 = 0x5eed_1234_abcd_0042;
    let mut white = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    };
    let (mut b0, mut b1, mut b2) = (0.0, 0.0, 0.0);
    let pink: Vec<f64> = (0..(secs * RATE) as usize)
        .map(|_| {
            let w = white();
            b0 = 0.99765 * b0 + w * 0.0990460;
            b1 = 0.96300 * b1 + w * 0.2965164;
            b2 = 0.57000 * b2 + w * 1.0526913;
            b0 + b1 + b2 + w * 0.1848
        })
        .collect();
    let rms = (pink.iter().map(|v| v * v).sum::<f64>() / pink.len() as f64).sqrt();
    let gain = 10f64.powf(db / 20.0) / rms;
    pink.iter().map(|v| (v * gain) as f32).collect()
}

#[test]
fn a_start_moves_to_its_onset_within_one_80_ms_step() {
    let x = bursts(2.0, &[(1.05, 1.60)]);
    let mut w = vec![raw("go", 1.00, 1.60)]; // the recognizer's step put it at 1.00
    refine_edges(&mut w, &x);
    assert!(
        (w[0].start_us - 1_040_000).abs() <= 5_000,
        "{}",
        w[0].start_us
    ); // the onset, less the 10 ms margin
}

#[test]
fn an_end_moves_back_to_its_offset() {
    let x = bursts(2.0, &[(1.05, 1.52)]);
    let mut w = vec![raw("go", 1.04, 1.60)];
    refine_edges(&mut w, &x);
    assert!((w[0].end_us - 1_530_000).abs() <= 5_000, "{}", w[0].end_us); // the offset, plus the 10 ms margin
}

#[test]
fn edges_never_cross_a_neighbour_or_move_more_than_one_step() {
    // sound 0.95–1.30 s and 1.32–1.80 s; the recognizer's steps put "a" at 1.00–1.40 and "b" at 1.40–1.80
    let x = bursts(2.0, &[(0.95, 1.30), (1.32, 1.80)]);
    let before = vec![raw("a", 1.00, 1.40), raw("b", 1.40, 1.80)];
    let mut w = before.clone();
    refine_edges(&mut w, &x);
    for (a, b) in w.iter().zip(&before) {
        assert!(
            (a.start_us - b.start_us).abs() <= 80_000 && (a.end_us - b.end_us).abs() <= 80_000,
            "{a:?}"
        );
        assert!(a.end_us - a.start_us >= 30_000, "{a:?}");
    }
    assert!(w[1].start_us >= w[0].end_us, "{w:?}");
    assert!(
        (w[0].start_us - 940_000).abs() <= 5_000,
        "a's onset, less the 10 ms margin"
    );
}

#[test]
fn steady_noise_moves_nothing() {
    let x = noise(2.0, -30.0);
    let before = vec![raw("a", 0.40, 0.80), raw("b", 0.80, 1.20)];
    let mut w = before.clone();
    refine_edges(&mut w, &x);
    assert_eq!(w, before);
}

/// Two joined words (one shared edge, as a recognizer's pieces give them): the second's onset is earlier than its step
/// said, so the edge they share moves back to it, the first word's end with it.
#[test]
fn a_shared_edge_moves_to_the_second_words_onset() {
    let x = bursts(2.0, &[(0.50, 0.90), (0.92, 1.40)]);
    let mut w = vec![raw("a", 0.50, 0.96), raw("b", 0.96, 1.40)];
    refine_edges(&mut w, &x);
    assert!(
        (w[1].start_us - 910_000).abs() <= 5_000,
        "{}",
        w[1].start_us
    );
    assert_eq!(w[0].end_us, w[1].start_us, "still joined");
}

/// In a packed window, a joined piece's first word follows the 200 ms pad (digital silence) and the span's widened
/// start (noise before the voice). The recognizer's step put it a step early, over that noise. The voice's rise out of
/// noise is no digital silence, so the start leads its step by 10 ms (`LEAD_US`), never into the pad; the remap clamps
/// a start still earlier to its piece's start.
#[test]
fn a_joined_pieces_first_word_over_noise_leads_its_step_and_never_enters_the_pad() {
    // 0–0.6 s the piece before (a voice), 0.6–0.8 s the pad, 0.8–0.98 s noise at −35 dBFS, 0.98–1.5 s the voice.
    let mut x = tones(2.0, &[(0.0, 0.6), (0.98, 1.5)], 1_000.0, -20.0);
    for (s, n) in x[12_800..15_680].iter_mut().zip(noise(0.18, -35.0)) {
        *s += n;
    }
    let mut w = vec![raw("before", 0.10, 0.60), raw("first", 0.90, 1.50)];
    refine_edges(&mut w, &x);
    assert_eq!(w[1].start_us, 890_000);
}

/// In connected speech (no digital silence: a voice over a quiet room), a joined edge leads the recognizer's time by
/// 10 ms instead of moving to the envelope's rise, which on read speech lands after the word's start (a consonant's
/// release, a vowel's onset).
#[test]
fn a_joined_edge_in_connected_speech_leads_the_recognizers_time_not_the_rise() {
    let mut x = noise(2.0, -60.0);
    voice(&mut x, &[(0.30, 0.86), (0.92, 1.50)], -20.0);
    let mut w = vec![raw("a", 0.30, 0.98), raw("b", 0.98, 1.50)];
    refine_edges(&mut w, &x);
    assert_eq!(
        (w[0].end_us, w[1].start_us),
        (970_000, 970_000),
        "led by 10 ms, not the rise's 0.91 s"
    );
}

/// The envelope is one pass over the samples and each edge looks at 33 hops: a minute of sound with 170 words is
/// refined in well under a millisecond a second of sound even in a debug build (the report states the release cost).
#[test]
fn a_minute_with_170_words_is_refined_in_one_pass() {
    let x = noise(60.0, -30.0);
    let mut w: Vec<RawWord> = (0..170)
        .map(|i| raw("w", i as f64 * 0.35, i as f64 * 0.35 + 0.3))
        .collect();
    let started = std::time::Instant::now();
    refine_edges(&mut w, &x);
    let took = started.elapsed();
    assert!(took.as_millis() < 500, "{took:?}");
}

/// A seeded LCG in [0, 1).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Adds a voiced sound over each range: a 150 Hz harmonic series (amplitudes 1/n, up to 3 kHz) at `dbfs` RMS.
fn voice(x: &mut [f32], ranges: &[(f64, f64)], dbfs: f64) {
    for &(from, to) in ranges {
        let (a, b) = ((from * RATE).round() as usize, (to * RATE).round() as usize);
        let v: Vec<f64> = (0..b - a)
            .map(|n| {
                (1..=20)
                    .map(|h| {
                        (2.0 * std::f64::consts::PI * 150.0 * h as f64 * n as f64 / RATE).sin()
                            / h as f64
                    })
                    .sum()
            })
            .collect();
        let rms = (v.iter().map(|s| s * s).sum::<f64>() / v.len() as f64).sqrt();
        let gain = 10f64.powf(dbfs / 20.0) / rms;
        for (s, v) in x[a..b].iter_mut().zip(v) {
            *s += (v * gain) as f32;
        }
    }
}

/// Adds a beat's hits at each time: 150 ms of white noise decaying with time constant `tau` (seconds; a click or a
/// tight hi-hat about 0.005–0.010, a loose hat or a clap about 0.020, a snare 0.040 and more), at `dbfs` RMS over its
/// first 30 ms. A hit is broadband, so pre-emphasis keeps it while it lowers a voice. Its tail runs well past the
/// 10–50 ms a rise must hold for, so the hold sees all of its decay.
fn hits(x: &mut [f32], at: &[f64], dbfs: f64, tau: f64) {
    let mut r = Lcg(0xbea7);
    let n = (0.150 * RATE) as usize;
    let head = (0.030 * RATE) as usize;
    for &t in at {
        let h: Vec<f64> = (0..n)
            .map(|i| (r.next() * 2.0 - 1.0) * (-(i as f64) / (tau * RATE)).exp())
            .collect();
        let rms = (h[..head].iter().map(|s| s * s).sum::<f64>() / head as f64).sqrt();
        let gain = 10f64.powf(dbfs / 20.0) / rms;
        let a = (t * RATE).round() as usize;
        for (s, v) in x[a..].iter_mut().zip(h) {
            *s += (v * gain) as f32;
        }
    }
}

/// A beat under the voice (8th notes at 120 BPM, each hit 15 dB under the voice), over a quiet room and
/// over a steady bed 20 dB under the voice, for a tight hat (an 8 ms decay), a loose hat or clap (20 ms) and a snare
/// (40 ms). The hit in the dip 60 ms before the second word, inside the two words' shared edge's window, never takes
/// the edge: no digital silence there, so the voice's held rise leads the recognizer's time by 10 ms (`LEAD_US`), or
/// the edge stays at its step when the hit's tail hides that rise.
#[test]
fn a_hit_of_a_beat_in_the_dip_between_two_words_is_no_onset() {
    for bed in [-60.0, -40.0] {
        for tau in [0.008, 0.020, 0.040] {
            let mut x = noise(2.0, bed);
            voice(&mut x, &[(0.30, 0.86), (0.95, 1.50)], -20.0);
            hits(
                &mut x,
                &[0.14, 0.39, 0.64, 0.89, 1.14, 1.39, 1.64],
                -35.0,
                tau,
            );
            let mut w = vec![raw("a", 0.30, 0.96), raw("b", 0.96, 1.50)]; // the step put their edge 10 ms late
            refine_edges(&mut w, &x);
            assert!(
                (935_000..=965_000).contains(&w[1].start_us),
                "bed {bed} dBFS, decay {tau} s: led (0.95 s) or the step's (0.96 s), never the hit's (0.88 s): \
                 {w:?}"
            );
            assert_eq!(w[0].end_us, w[1].start_us, "still joined");
        }
    }
}

/// The same words without the beat: a real word's onset after a gap still counts as a held rise (the hold guard never
/// takes a word for a hit), so the joined edge moves: it leads the recognizer's time by 10 ms (`LEAD_US`).
#[test]
fn without_the_beat_the_words_onset_still_moves_the_edge() {
    let mut x = noise(2.0, -60.0);
    voice(&mut x, &[(0.30, 0.86), (0.95, 1.50)], -20.0);
    let mut w = vec![raw("a", 0.30, 0.96), raw("b", 0.96, 1.50)];
    refine_edges(&mut w, &x);
    assert_eq!(w[1].start_us, 950_000, "{w:?}");
}

/// The envelope's floor: one loud clap elsewhere in the window doesn't lift the floor over a quiet
/// word's onset out of digital silence.
#[test]
fn a_clap_elsewhere_in_the_window_never_hides_a_quiet_onset() {
    let mut x = vec![0.0f32; 32_000];
    voice(&mut x, &[(1.05, 1.50)], -45.0);
    hits(&mut x, &[0.30], -3.0, 0.020);
    let mut w = vec![raw("so", 1.00, 1.50)];
    refine_edges(&mut w, &x);
    assert!(
        (w[0].start_us - 1_040_000).abs() <= 5_000,
        "{}",
        w[0].start_us
    );
}

/// The hard case of the packed window above: the recognizer's step puts the first word 150 ms early, so only the pad's
/// end is within a step. It may move there (a rise out of digital silence is a rise), but never into the pad past the
/// margin, and never more than a step; the remap clamps it to its piece's start.
#[test]
fn a_first_word_more_than_a_step_early_moves_at_most_onto_its_pads_end() {
    let mut x = tones(2.0, &[(0.0, 0.6), (0.98, 1.5)], 1_000.0, -20.0);
    for (s, n) in x[12_800..15_680].iter_mut().zip(noise(0.18, -35.0)) {
        *s += n;
    }
    let mut w = vec![raw("before", 0.10, 0.60), raw("first", 0.83, 1.50)];
    refine_edges(&mut w, &x);
    assert!(
        (790_000..=830_000).contains(&w[1].start_us),
        "{}",
        w[1].start_us
    );
}

/// Whatever the words (shorter than 30 ms, overlapping, out of order) and the sound, nothing panics and no
/// edge moves more than a step; well-formed words (in order, apart or joined, each at least 30 ms) stay so. Odd cases
/// crowd 30 such words and 20 bursts into 2 s, where a word's window can close on itself (`lo > hi`: `clamp` would
/// panic without its guard).
#[test]
fn any_words_over_any_sound_stay_within_a_step_and_well_formed_ones_stay_so() {
    // Crafted: "b" ends 55 ms before the word it overlaps does, with an onset where their shared edge's window closes
    // on itself (`lo` 1.02 s, `hi` 1.015 s).
    let x = bursts(2.0, &[(1.02, 1.5)]);
    let before = vec![raw("a", 0.50, 1.10), raw("b", 0.98, 1.045)];
    let mut w = before.clone();
    refine_edges(&mut w, &x);
    for (a, b) in w.iter().zip(&before) {
        assert!((a.start_us - b.start_us).abs() <= 80_000 && (a.end_us - b.end_us).abs() <= 80_000);
    }
    let mut r = Lcg(0x5eed);
    for case in 0..1_000 {
        let well_formed = case % 2 == 0;
        let (secs, bursts, words) = if well_formed {
            (6.0, 8.0, 8.0)
        } else {
            (2.0, 20.0, 30.0)
        };
        let mut x = noise(secs, -40.0 - r.next() * 40.0);
        for _ in 0..(r.next() * bursts) as usize {
            let at = (r.next() * (secs - 0.7) * RATE) as usize;
            let len = ((0.02 + r.next() * 0.6) * RATE) as usize;
            let hz = 100.0 + r.next() * 3_000.0;
            let amp = amplitude(-15.0 - r.next() * 30.0);
            for (n, s) in x[at..at + len].iter_mut().enumerate() {
                *s += (amp * (2.0 * std::f64::consts::PI * hz * n as f64 / RATE).sin()) as f32;
            }
        }
        let mut t = r.next() * 0.2;
        let before: Vec<RawWord> = (0..1 + (r.next() * words) as usize)
            .map(|_| {
                let (start, len) = if well_formed {
                    let gap = if r.next() < 0.5 { 0.0 } else { r.next() * 0.2 };
                    (t + gap, 0.031 + r.next() * 0.4)
                } else {
                    (r.next() * (secs - 0.1), r.next() * 0.2)
                };
                t = start + len;
                raw("w", start, (start + len).min(secs))
            })
            .collect();
        let mut w = before.clone();
        refine_edges(&mut w, &x);
        for (a, b) in w.iter().zip(&before) {
            assert!(
                (a.start_us - b.start_us).abs() <= 80_000 && (a.end_us - b.end_us).abs() <= 80_000,
                "case {case}: {a:?} from {b:?}"
            );
        }
        if well_formed {
            for (i, a) in w.iter().enumerate() {
                assert!(a.end_us - a.start_us >= 30_000, "case {case}: {w:?}");
                assert!(
                    i == 0 || a.start_us >= w[i - 1].end_us,
                    "case {case}: {w:?}"
                );
            }
        }
    }
}
