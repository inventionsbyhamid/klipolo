//! Integrated loudness of the whole mix (EBU R128) and the gain that reaches the project's target. Sources overlap
//! and sum, so the mix is measured, not derived from each file's own loudness. Both the preview and export level to
//! `effective_target` with the same decision (`level_db`), so the preview sounds like the export; a project whose
//! loudness is Off is left untouched everywhere and never measured.
//!
//! Export measures its rendered mix exactly (the `ebur128` crate, plus true peak). The preview estimates it after
//! every sound-changing edit (`estimate`): each file's 100 ms K-weighted energies, written in
//! the PCM decode pass (`pcm::Energies`), are summed as powers over the timeline at each source's gain and fades,
//! then gated as R128 gates (`integrated`). That costs milliseconds where mixing the whole timeline again cost
//! seconds. A source with an audio effect has its own windows through the effect, rendered once and kept beside the
//! caches (`PcmStore::effected`: its loudness depends on the effect), and a project whose files have no energies is
//! measured exactly (`measure_until`).

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use klipolo_model::project::Loudness;

use super::mix::{Mixdown, RATE, fade_gain, to_frame};
use super::pcm::energy::EnergyWriter;
use super::pcm::{Effected, Energies, Pcm, WINDOW};
use super::plan::{AudioPlan, AudioSource, Crossfade};
use crate::api::ExportQuality;

/// Frames measured per block.
const BLOCK: usize = 4_096;
/// R128's absolute gate (−70 LUFS) as a mean square.
const ABSOLUTE_GATE: f64 = 1.1724653045822963e-7;
/// The quality the preview levels for: the export's default, so what plays is what a default export sounds like.
pub const PREVIEW_QUALITY: ExportQuality = ExportQuality::High;

/// The project's own loudness target (None: Off). New projects aim at −14 LUFS (`AudioSettings::default`).
pub fn target_lufs(loudness: Loudness) -> Option<f64> {
    match loudness {
        Loudness::Off => None,
        Loudness::Youtube => Some(-14.0),
        Loudness::Podcast => Some(-16.0),
    }
}

/// The level the whole mix is brought to (None: left as it is, and not measured). The one rule the export and the
/// preview share (the preview asks for `PREVIEW_QUALITY`):
/// - Off is None everywhere: the preview and every export, Master included, play and write the mix untouched;
/// - Master never levels, whatever the project's target (the loudness target is for the social
///   presets; a master is finished in another editor);
/// - every other quality levels to the project's target.
pub fn effective_target(loudness: Loudness, quality: ExportQuality) -> Option<f64> {
    if quality == ExportQuality::Master {
        return None;
    }
    target_lufs(loudness)
}

/// Gain toward the target, never more than ±12 dB (very quiet or silent mixes stay natural).
pub fn gain_db(measured: f64, target: f64) -> f64 {
    (target - measured).clamp(-12.0, 12.0)
}

/// The gain decision (dB on the whole mix) for a mix measured at `measured` LUFS (None: silent or not measured):
/// toward `effective_target`, none without a target.
pub fn level_db(measured: Option<f64>, loudness: Loudness, quality: ExportQuality) -> f64 {
    match (measured, effective_target(loudness, quality)) {
        (Some(lufs), Some(target)) => gain_db(lufs, target),
        _ => 0.0,
    }
}

/// Integrated LUFS of the mix (unity master gain), or None for silence or a mix whose audio isn't decoded yet.
/// Sources without PCM (files that can't be decoded) count as silent.
pub fn measure(plan: &AudioPlan, pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>) -> Option<f64> {
    measure_until(plan, pcm, &AtomicBool::new(false))
}

/// `measure`, giving up (None) once `cancel` is set (a newer edit wants a new measurement).
pub fn measure_until(
    plan: &AudioPlan,
    pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
    cancel: &AtomicBool,
) -> Option<f64> {
    let end = plan.sources.iter().map(|s| s.end_us).max()?;
    let start = plan.sources.iter().map(|s| s.start_us).min()?.max(0);
    if plan
        .sources
        .iter()
        .any(|s| pcm(s).is_some_and(|p| !p.is_complete()))
    {
        return None;
    }
    let (from, to) = (to_frame(start) as u64, to_frame(end) as u64);
    let mut meter = ebur128::EbuR128::new(2, RATE, ebur128::Mode::I).ok()?;
    let mut mixdown = Mixdown::new(Arc::new(plan.clone()));
    let mut block = vec![0f32; 2 * BLOCK];
    let mut at = from;
    while at < to {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = (to - at).min(BLOCK as u64) as usize;
        mixdown.render(pcm, at, &mut block[..n * 2]);
        meter.add_frames_f32(&block[..n * 2]).ok()?;
        at += n as u64;
    }
    meter.loudness_global().ok().filter(|l| l.is_finite())
}

/// ITU-R BS.1770's K-weighting at 48 kHz, the filter R128 measures through: a high shelf (+4 dB above about 1.5 kHz),
/// then a high-pass (about 38 Hz). Two biquads with f64 state; it starts at rest.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KWeight {
    z: [[f64; 2]; 2],
}

/// The two stages' (b0, b1, b2, a1, a2) at 48 kHz (BS.1770-4, table 1 and 2).
const K_STAGES: [[f64; 5]; 2] = [
    [
        1.535_124_859_586_97,
        -2.691_696_189_406_38,
        1.198_392_810_852_85,
        -1.690_659_293_182_41,
        0.732_480_774_215_85,
    ],
    [1.0, -2.0, 1.0, -1.990_047_454_833_98, 0.990_072_250_366_21],
];

impl KWeight {
    /// One sample through both stages (direct form II transposed).
    pub(crate) fn run(&mut self, x: f32) -> f64 {
        let mut v = f64::from(x);
        for ([b0, b1, b2, a1, a2], z) in K_STAGES.iter().zip(&mut self.z) {
            let y = b0 * v + z[0];
            z[0] = b1 * v - a1 * y + z[1];
            z[1] = b2 * v - a2 * y;
            v = y;
        }
        v
    }
}

/// Integrated loudness (LUFS) from consecutive 100 ms powers (each the sum over channels of the K-weighted mean
/// square; a mono file counts on both speakers): R128's 400 ms blocks every 100 ms, the absolute gate at −70 LUFS,
/// then the relative gate 10 LU under the mean of the blocks above it. None for silence or under 400 ms. O(n), no
/// allocation.
pub(crate) fn integrated(power: &[f64]) -> Option<f64> {
    let blocks = || power.windows(4).map(|w| (w[0] + w[1] + w[2] + w[3]) / 4.0);
    let mean = |gate: f64| {
        let (sum, n) = blocks()
            .filter(|&z| z >= gate)
            .fold((0.0, 0u64), |(sum, n), z| (sum + z, n + 1));
        (n > 0).then(|| sum / n as f64)
    };
    let relative = mean(ABSOLUTE_GATE)? / 10.0;
    mean(ABSOLUTE_GATE.max(relative)).map(|z| -0.691 + 10.0 * z.log10())
}

/// The preview's estimate of the mix's loudness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Estimate {
    /// Integrated LUFS; None for silence.
    Lufs(Option<f64>),
    /// A decoded file has no energies (they couldn't be written): measure the mix exactly instead.
    Missing,
    Cancelled,
}

/// The integrated loudness of the plan's mix (unity master gain), estimated from each file's 100 ms energies summed
/// as powers (see the module docs): each source adds, per 100 ms block of the timeline (counted from the earliest
/// source, as the exact meter counts), its file's mean energy over the source frames it plays there, times its gain
/// with its fades squared and the share of the block it covers. That assumes different sources don't correlate,
/// which holds for different files and keeps an equal-power crossfade level; sources that play the same samples at
/// The same moment (a dissolve over a split with nothing removed) add their cross term too (`add_same_sound`
/// up to +3 dB in the middle of such a dissolve). A source with an audio effect adds its
/// own windows through the effect the same way (`effected`: `PcmStore::effected`, rendered once and kept, or this
/// time only when they can't be kept). Sources without `pcm` (files that can't be decoded) are silent, as in the mix.
///
/// Cost: O(Σ blocks each source spans + S log S) plus the gating, with one f64 per block of the timeline (576 KB for
/// two hours) and one index per source without an effect (the cross terms' sort); an effected source's first estimate
/// costs a render of its own span. Within about 0.5 LU of `measure` (the tests pin it, dissolves over continuous
/// footage included); the 3 ms edge ramps and a speed change's small change of level are left out.
pub fn estimate_until<'e>(
    plan: &AudioPlan,
    pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
    energies: &dyn Fn(&AudioSource) -> Option<&'e Energies>,
    effected: &dyn Fn(&AudioSource, &Arc<Pcm>, &AtomicBool) -> Effected,
    cancel: &AtomicBool,
) -> Estimate {
    let (Some(start), Some(end)) = (
        plan.sources.iter().map(|s| s.start_us).min(),
        plan.sources.iter().map(|s| s.end_us).max(),
    ) else {
        return Estimate::Lufs(None);
    };
    let (from, to) = (to_frame(start.max(0)), to_frame(end));
    let window = WINDOW as i64;
    // Only whole blocks count, as in the meter.
    let mut power = vec![0f64; ((to - from).max(0) / window) as usize];
    // The sources without an effect added from their file's energies, by index (sized once: one allocation).
    let mut plain = Vec::with_capacity(plan.sources.len());
    for (i, s) in plan.sources.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Estimate::Cancelled;
        }
        if s.gain <= 0.0 {
            continue;
        }
        let Some(decoded) = pcm(s) else { continue };
        let start = to_frame(s.start_us);
        if s.filter.is_some() {
            match effected(s, &decoded, cancel) {
                // Its own windows count from its start, at the timeline's rate (its speed is in them), and are of what
                // the mix plays (a centred channel included).
                Effected::Ready(file) => {
                    add_source(s, &file, from, &mut power, None, |f| (f - start) as f64);
                }
                Effected::Cancelled => return Estimate::Cancelled,
                Effected::Unavailable => {
                    if !render_alone(s, decoded, from, &mut power, cancel) {
                        return Estimate::Cancelled;
                    }
                }
            }
            continue;
        }
        let Some(file) = energies(s) else {
            return Estimate::Missing;
        };
        let source_in = to_frame(s.source_in_us) as f64;
        add_source(s, file, from, &mut power, s.centre_from, |f| {
            source_in + (f - start) as f64 * s.step()
        });
        plain.push(i);
    }
    add_same_sound(plan, &mut plain, energies, from, &mut power);
    Estimate::Lufs(integrated(&power))
}

/// Adds what summing powers leaves out where sources play the same samples at the same moment: the same file at the
/// same speed and rate, from the same channels, lined up frame for frame (a split with nothing removed, under a dissolve; or the same take stacked).
/// Their signals add, so the mix there holds `2·gA·gB` times the file's power on top of the `gA² + gB²` already added:
/// up to +3 dB in the middle of an equal-power crossfade, where unrelated sounds stay level. `plain`: the sources
/// (by index) added from their file's energies. One in-place sort groups them, O(S log S) with no allocation; then
/// O(blocks each pair shares).
fn add_same_sound<'e>(
    plan: &AudioPlan,
    plain: &mut [usize],
    energies: &dyn Fn(&AudioSource) -> Option<&'e Energies>,
    from: i64,
    power: &mut [f64],
) {
    let sources = &plan.sources;
    // Where the timeline's frame 0 falls in the file: the same for sources lined up frame for frame.
    let origin = |i: usize| {
        let s = &sources[i];
        to_frame(s.source_in_us) as f64 - to_frame(s.start_us) as f64 * s.step()
    };
    // In place (no buffer): the order within a group doesn't matter.
    plain.sort_unstable_by(|&a, &b| {
        let (x, y) = (&sources[a], &sources[b]);
        (x.path.cmp(&y.path))
            .then(x.speed.total_cmp(&y.speed))
            .then(x.rate.total_cmp(&y.rate))
            .then(x.centre_from.cmp(&y.centre_from))
            .then(origin(a).total_cmp(&origin(b)))
    });
    let mut rest = plain;
    while let Some(&first) = rest.first() {
        // Lined up within a frame of the run's first (each frame is rounded from µs).
        let lined_up = |i: &usize| {
            let (s, f) = (&sources[*i], &sources[first]);
            s.path == f.path
                && s.speed == f.speed
                && s.rate == f.rate
                && s.centre_from == f.centre_from
                && (origin(*i) - origin(first)).abs() <= 1.0
        };
        let n = rest.iter().take_while(|i| lined_up(i)).count();
        let (group, tail) = std::mem::take(&mut rest).split_at_mut(n);
        rest = tail;
        if group.len() < 2 {
            continue;
        }
        group.sort_unstable_by_key(|&i| sources[i].start_us);
        for (k, &a) in group.iter().enumerate() {
            let Some(file) = energies(&sources[a]) else {
                continue;
            };
            let end = sources[a].end_us;
            for &b in group[k + 1..]
                .iter()
                .take_while(|&&b| sources[b].start_us < end)
            {
                add_cross(&sources[a], &sources[b], file, from, power);
            }
        }
    }
}

/// The cross term of sources `a` and `b`, which play the same samples, in each block they share:
/// `2 · mean(gA·gB) · power`.
fn add_cross(a: &AudioSource, b: &AudioSource, file: &Energies, from: i64, power: &mut [f64]) {
    let window = WINDOW as i64;
    let (start_a, end_a) = (to_frame(a.start_us), to_frame(a.end_us));
    let (start_b, end_b) = (to_frame(b.start_us), to_frame(b.end_us));
    let (lo, hi) = (start_a.max(start_b).max(from), end_a.min(end_b));
    if lo >= hi {
        return;
    }
    let (fades_a, fades_b) = (Fades::of(a, start_a, end_a), Fades::of(b, start_b, end_b));
    let source_in = to_frame(a.source_in_us) as f64;
    let at = |f: i64| source_in + (f - start_a) as f64 * a.step();
    let first = ((lo - from) / window) as usize;
    for (k, block) in power.iter_mut().enumerate().skip(first) {
        let block_at = from + k as i64 * window;
        if block_at >= hi {
            break;
        }
        let (x, y) = (lo.max(block_at), hi.min(block_at + window));
        if x >= y {
            continue;
        }
        let heard = file.mean(at(x), at(y), a.centre_from);
        if heard > 0.0 {
            *block +=
                2.0 * heard * fades_a.mean_product(&fades_b, x, y) * (y - x) as f64 / window as f64;
        }
    }
}

/// Source `s`'s power in each block of `power` (blocks of `WINDOW` frames from timeline frame `from`), from `file`'s
/// windows (as the mix plays them: centred from channel `centre`, or as they are), frame `f` of the timeline at
/// `file_frame(f)` in them.
fn add_source(
    s: &AudioSource,
    file: &Energies,
    from: i64,
    power: &mut [f64],
    centre: Option<u8>,
    file_frame: impl Fn(i64) -> f64,
) {
    let window = WINDOW as i64;
    let (start, end) = (to_frame(s.start_us), to_frame(s.end_us));
    let fades = Fades::of(s, start, end);
    let first = ((start.max(from) - from) / window) as usize;
    for (k, block) in power.iter_mut().enumerate().skip(first) {
        let block_at = from + k as i64 * window;
        if block_at >= end {
            break;
        }
        let (a, b) = (start.max(block_at), end.min(block_at + window));
        if a >= b {
            continue;
        }
        let heard = file.mean(file_frame(a), file_frame(b), centre);
        if heard > 0.0 {
            *block += heard * fades.mean_square(a, b) * (b - a) as f64 / window as f64;
        }
    }
}

/// A source's gain over time: its volume, then its fades (linear to silence, or a crossfade's equal power), as the
/// mixer applies them (`Mixdown`), without its 3 ms edge ramps.
struct Fades {
    gain: f64,
    start: i64,
    end: i64,
    fade_in: i64,
    fade_out: i64,
    equal_power: (bool, bool),
}

impl Fades {
    fn of(s: &AudioSource, start: i64, end: i64) -> Self {
        Self {
            gain: s.gain,
            start,
            end,
            fade_in: to_frame(s.fade_in_us),
            fade_out: to_frame(s.fade_out_us),
            equal_power: (s.crossfade.fade_in, s.crossfade.fade_out),
        }
    }

    fn at(&self, f: f64) -> f64 {
        let mut g = self.gain;
        if self.fade_in > 0 {
            g *= f64::from(fade_gain(
                ((f - self.start as f64) / self.fade_in as f64) as f32,
                self.equal_power.0,
            ));
        }
        if self.fade_out > 0 {
            g *= f64::from(fade_gain(
                ((self.end as f64 - f) / self.fade_out as f64) as f32,
                self.equal_power.1,
            ));
        }
        g
    }

    /// The mean of this gain times `other`'s over frames `[a, b)`: the mean of four samples.
    fn mean_product(&self, other: &Fades, a: i64, b: i64) -> f64 {
        let step = (b - a) as f64 / 4.0;
        (0..4)
            .map(|k| {
                let t = a as f64 + (k as f64 + 0.5) * step;
                self.at(t) * other.at(t)
            })
            .sum::<f64>()
            / 4.0
    }

    /// The mean of the squared gain over frames `[a, b)`: exact outside the fades, else the mean of four samples.
    fn mean_square(&self, a: i64, b: i64) -> f64 {
        if a >= self.start + self.fade_in && b <= self.end - self.fade_out {
            return self.gain * self.gain;
        }
        let step = (b - a) as f64 / 4.0;
        (0..4)
            .map(|k| self.at(a as f64 + (k as f64 + 0.5) * step).powi(2))
            .sum::<f64>()
            / 4.0
    }
}

/// Writes source `s` alone through its effect and speed, at unity gain and without fades (what the mixer plays of it
/// before its volume and fades), into `writer` as 100 ms K-weighted windows from its start (`PcmStore::effected`).
/// False when cancelled. A block of 100 ms at a time: O(its span), 38 KB of buffer.
pub(crate) fn write_effected(
    s: &AudioSource,
    pcm: &Arc<Pcm>,
    writer: &mut EnergyWriter,
    cancel: &AtomicBool,
) -> io::Result<bool> {
    let unity = AudioSource {
        start_us: 0,
        end_us: s.end_us - s.start_us,
        gain: 1.0,
        fade_in_us: 0,
        fade_out_us: 0,
        crossfade: Crossfade::default(),
        ..s.clone()
    };
    let frames = to_frame(unity.end_us).max(0) as u64;
    let mut mixdown = Mixdown::new(Arc::new(AudioPlan {
        sources: vec![unity],
        loudness: Loudness::Off,
    }));
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let mut block = vec![0f32; 2 * WINDOW];
    writer.begin(0);
    let mut at = 0;
    while at < frames {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let n = (frames - at).min(WINDOW as u64) as usize;
        mixdown.render(&lookup, at, &mut block[..2 * n]);
        writer.push(at, &block[..2 * n])?;
        at += n as u64;
    }
    writer.end_run(true)?;
    Ok(true)
}

/// Renders source `s` alone (its effect, speed, gain and fades, as the mix plays it), K-weights it and adds its power
/// to each block of `power`: when its windows can't be kept (`Effected::Unavailable`). False when cancelled.
fn render_alone(
    s: &AudioSource,
    pcm: Arc<Pcm>,
    from: i64,
    power: &mut [f64],
    cancel: &AtomicBool,
) -> bool {
    let window = WINDOW as i64;
    let alone = Arc::new(AudioPlan {
        sources: vec![s.clone()],
        loudness: Loudness::Off,
    });
    let mut mixdown = Mixdown::new(alone);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let (start, end) = (to_frame(s.start_us), to_frame(s.end_us));
    let first = ((start.max(from) - from) / window) as usize;
    let mut weight = [KWeight::default(); 2];
    let mut block = vec![0f32; 2 * WINDOW];
    for (k, out) in power.iter_mut().enumerate().skip(first) {
        let at = from + k as i64 * window;
        if at >= end {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        mixdown.render(&lookup, at as u64, &mut block);
        let sum: f64 = block
            .chunks_exact(2)
            .map(|lr| {
                let (l, r) = (weight[0].run(lr[0]), weight[1].run(lr[1]));
                l * l + r * r
            })
            .sum();
        *out += sum / window as f64;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{KWeight, integrated};
    use crate::audio::mix::RATE;

    /// Stereo test signal: a voice-like burst pattern (loud, quiet, silent stretches) over a low hum, so both gates
    /// and both K-weighting stages matter.
    fn signal(seconds: usize) -> Vec<f32> {
        let rate = RATE as f32;
        (0..seconds * RATE as usize)
            .flat_map(|n| {
                let t = n as f32 / rate;
                let level = match (t * 2.0) as usize % 7 {
                    0..=3 => 0.5,
                    4 => 0.02,
                    5 => 0.0,
                    _ => 0.2,
                };
                let voice = level * (2.0 * std::f32::consts::PI * 1_800.0 * t).sin();
                let hum = 0.3 * (2.0 * std::f32::consts::PI * 30.0 * t).sin();
                [voice + hum, 0.6 * voice]
            })
            .collect()
    }

    /// The weighting and gating match the `ebur128` crate (libebur128), which export measures with.
    #[test]
    fn integrated_loudness_from_100_ms_powers_matches_ebur128() {
        let samples = signal(20);
        let mut meter = ebur128::EbuR128::new(2, RATE, ebur128::Mode::I).unwrap();
        meter.add_frames_f32(&samples).unwrap();
        let exact = meter.loudness_global().unwrap();
        let mut weight = [KWeight::default(); 2];
        let power: Vec<f64> = samples
            .chunks_exact(2 * 4_800)
            .map(|block| {
                block
                    .chunks_exact(2)
                    .map(|lr| weight[0].run(lr[0]).powi(2) + weight[1].run(lr[1]).powi(2))
                    .sum::<f64>()
                    / 4_800.0
            })
            .collect();
        let ours = integrated(&power).unwrap();
        assert!((ours - exact).abs() < 0.01, "{ours} vs {exact}");
        assert_eq!(integrated(&power[..3]), None, "under 400 ms");
        assert_eq!(integrated(&[0.0; 50]), None, "silence");
    }
}
