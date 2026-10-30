//! The export mix's true-peak limiter: a look-ahead brickwall at a dBTP ceiling (−1 dBTP for uploads). Peaks are
//! found between samples with ITU-R BS.1770's 4× interpolation filter, so the encoded file doesn't clip when a
//! player reconstructs the waveform. Gain drops before a peak (look-ahead), never below what the peak needs, and
//! recovers smoothly. Preview doesn't use it (its soft clip is enough for listening); export applies it after the
//! loudness gain. Fixed-size rings only: nothing allocates per block.

/// Interpolation filter phases (BS.1770-4 Annex 2): 4× oversampling, 12 taps each.
const PHASES: [[f32; 12]; 4] = [
    [
        0.001_708_984_4,
        0.010_986_328,
        -0.019_653_32,
        0.033_203_125,
        -0.059_448_242,
        0.137_329_1,
        0.972_167_97,
        -0.102_294_92,
        0.047_607_42,
        -0.026_611_328,
        0.014_892_578,
        -0.008_300_781,
    ],
    [
        -0.029_174_805,
        0.029_296_875,
        -0.051_757_812,
        0.089_111_33,
        -0.166_503_9,
        0.465_087_9,
        0.779_785_16,
        -0.200_317_38,
        0.101_562_5,
        -0.058_227_54,
        0.033_081_055,
        -0.018_920_898,
    ],
    [
        -0.018_920_898,
        0.033_081_055,
        -0.058_227_54,
        0.101_562_5,
        -0.200_317_38,
        0.779_785_16,
        0.465_087_9,
        -0.166_503_9,
        0.089_111_33,
        -0.051_757_812,
        0.029_296_875,
        -0.029_174_805,
    ],
    [
        -0.008_300_781,
        0.014_892_578,
        -0.026_611_328,
        0.047_607_42,
        -0.102_294_92,
        0.137_329_1,
        0.972_167_97,
        -0.059_448_242,
        0.033_203_125,
        -0.019_653_32,
        0.010_986_328,
        0.001_708_984_4,
    ],
];
const TAPS: usize = 12;
/// The interpolated values describe the waveform around the sample this many frames back.
const TP_DELAY: usize = TAPS / 2;
/// Look-ahead: gain reaches what a peak needs over this many frames (2 ms at 48 kHz).
const LOOKAHEAD: usize = 96;
/// Release: gain recovers with this time constant (80 ms at 48 kHz).
const RELEASE_FRAMES: f32 = 3_840.0;

/// A stereo look-ahead limiter (see the module docs). Output is delayed by `LATENCY` frames.
pub struct Limiter {
    ceiling: f32,
    gain: f32,
    release: f32,
    /// The last `TAPS` input frames (for the interpolation filter), newest at `hist_at`.
    history: [[f32; 2]; TAPS],
    hist_at: usize,
    /// Input delayed until its gain is known (`LATENCY + 1` frames).
    delay: [[f32; 2]; DELAY],
    delay_at: usize,
    /// The window's required gains as a monotonic queue (frame, gain), increasing from the front: its front is the
    /// window's minimum (amortised O(1) per frame).
    queue_at: [u64; LOOKAHEAD],
    queue_gain: [f32; LOOKAHEAD],
    queue_head: usize,
    queue_len: usize,
    frame: u64,
    /// The released gain over the window (box average), and their sum.
    held: [f32; LOOKAHEAD],
    held_sum: f32,
    win_at: usize,
    released: f32,
}

const DELAY: usize = TP_DELAY + LOOKAHEAD;

impl Limiter {
    /// Frames between a sample going in and coming out.
    pub const LATENCY: usize = DELAY - 1;

    /// Applies `gain` (linear) and limits true peaks to `ceiling_db` dBTP.
    pub fn new(ceiling_db: f32, gain: f32) -> Self {
        Self {
            ceiling: 10f32.powf(ceiling_db / 20.0),
            gain,
            release: (-1.0 / RELEASE_FRAMES).exp(),
            history: [[0.0; 2]; TAPS],
            hist_at: 0,
            delay: [[0.0; 2]; DELAY],
            delay_at: 0,
            queue_at: [0; LOOKAHEAD],
            queue_gain: [1.0; LOOKAHEAD],
            queue_head: 0,
            queue_len: 0,
            frame: 0,
            held: [1.0; LOOKAHEAD],
            held_sum: LOOKAHEAD as f32,
            win_at: 0,
            released: 1.0,
        }
    }

    /// The true peak around the sample `TP_DELAY` frames back, both channels.
    fn true_peak(&self) -> f32 {
        let mut peak = 0f32;
        for phase in &PHASES {
            let (mut l, mut r) = (0f32, 0f32);
            for (j, h) in phase.iter().enumerate() {
                let x = self.history[(self.hist_at + TAPS - j) % TAPS];
                l += h * x[0];
                r += h * x[1];
            }
            peak = peak.max(l.abs()).max(r.abs());
        }
        let centre = self.history[(self.hist_at + TAPS - TP_DELAY) % TAPS];
        peak.max(centre[0].abs()).max(centre[1].abs())
    }

    /// Adds this frame's need to the window and returns the window's minimum.
    fn window_min(&mut self, need: f32) -> f32 {
        // The oldest leaves the window first (so at most `LOOKAHEAD` stay queued).
        if self.queue_len > 0 && self.queue_at[self.queue_head] + LOOKAHEAD as u64 <= self.frame {
            self.queue_head = (self.queue_head + 1) % LOOKAHEAD;
            self.queue_len -= 1;
        }
        // Needs no lower than this one no longer bind.
        while self.queue_len > 0 {
            let back = (self.queue_head + self.queue_len - 1) % LOOKAHEAD;
            if self.queue_gain[back] < need {
                break;
            }
            self.queue_len -= 1;
        }
        let back = (self.queue_head + self.queue_len) % LOOKAHEAD;
        self.queue_at[back] = self.frame;
        self.queue_gain[back] = need;
        self.queue_len += 1;
        self.frame += 1;
        self.queue_gain[self.queue_head]
    }

    /// Limits `io` (interleaved stereo) in place; what comes out is `LATENCY` frames behind what goes in.
    pub fn process(&mut self, io: &mut [f32]) {
        for frame in io.chunks_exact_mut(2) {
            let x = [frame[0] * self.gain, frame[1] * self.gain];
            self.hist_at = (self.hist_at + 1) % TAPS;
            self.history[self.hist_at] = x;
            let peak = self.true_peak();
            let need = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            // The lowest need in the window, with an instant attack and a smooth release.
            let lowest = self.window_min(need);
            self.released = lowest.min(1.0 - (1.0 - self.released) * self.release);
            // Averaged over the window, so the gain ramps down ahead of the peak and is at most its need there.
            self.held_sum += self.released - self.held[self.win_at];
            self.held[self.win_at] = self.released;
            self.win_at = (self.win_at + 1) % LOOKAHEAD;
            let gain = (self.held_sum / LOOKAHEAD as f32).min(1.0);
            // The sample the gain belongs to: `TP_DELAY` (filter) + `LOOKAHEAD - 1` (window) frames ago.
            self.delay[self.delay_at] = x;
            self.delay_at = (self.delay_at + 1) % DELAY;
            let out = self.delay[self.delay_at];
            frame[0] = out[0] * gain;
            frame[1] = out[1] * gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn true_peak_db(samples: &[f32]) -> f64 {
        let mut meter = ebur128::EbuR128::new(2, 48_000, ebur128::Mode::TRUE_PEAK).unwrap();
        meter.add_frames_f32(samples).unwrap();
        let peak = meter.true_peak(0).unwrap().max(meter.true_peak(1).unwrap());
        20.0 * peak.log10()
    }

    #[test]
    fn loud_peaks_stay_under_the_ceiling_and_quiet_audio_passes_unchanged() {
        // A 997 Hz tone pushed 9 dB over full scale, with sharp onsets (the worst case for a look-ahead).
        let n = 48_000;
        let mut loud: Vec<f32> = (0..n)
            .flat_map(|i| {
                let on = (i / 4_800) % 2 == 0;
                let x = if on {
                    2.8 * (i as f32 * 997.0 * std::f32::consts::TAU / 48_000.0).sin()
                } else {
                    0.0
                };
                [x, -x]
            })
            .collect();
        // Export limits at −1.5 so meters (and the AAC encoder after it) read at most −1 dBTP.
        let mut limiter = Limiter::new(-1.5, 1.0);
        limiter.process(&mut loud);
        assert!(true_peak_db(&loud) <= -1.0, "{} dBTP", true_peak_db(&loud));

        // −12 dBFS is untouched, only delayed.
        let quiet: Vec<f32> = (0..n)
            .flat_map(|i| {
                let x = 0.25 * (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin();
                [x, x]
            })
            .collect();
        let mut out = quiet.clone();
        Limiter::new(-1.0, 1.0).process(&mut out);
        let d = Limiter::LATENCY * 2;
        for (a, b) in quiet.iter().zip(&out[d..]) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
