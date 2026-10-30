//! The timeline mix at 48 kHz stereo. The cpal output (preview), the loudness measure and the export worker all
//! render through a `Mixdown`, which keeps one voice per sounding source: a straight read of the PCM cache at 1×, a
//! resampled read at a drifting recording device's rate (`AudioSource::rate`), or a live `Stretch` (speed change and
//! effect) fed from it. Rendering consecutive blocks streams; a jump re-seats the
//! voices (with pre-roll for the filters). Every clip edge, hard cut and re-seat is ramped (3 ms) so nothing clicks.
//!
//! A block touches only the sources sounding in it: the sources are kept in order of start,
//! and consecutive blocks sweep a cursor through that order, holding an active set of what has started and not yet
//! ended (or still holds a voice to hand on). A block costs O(active + newly started), not O(sources): 3,000
//! sources (1,500 recording clips with mic and system sound) used to cost about 2 s of CPU per full pass just to be
//! skipped. A jump (a seek) finds where it is in that order by binary search, O(log S), then scans the sources that
//! started before it, back to the first whose prefix maximum of ends reaches past it: usually a few, but a bed that
//! runs from the start keeps every earlier source in the scan, O(S) at worst (a few µs for 3,000 sources, once per
//! seek).

#[cfg(test)]
mod tests;
mod voice;

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use super::pcm::Pcm;
use super::plan::{AudioPlan, AudioSource};
pub use voice::read_position_after;
use voice::{Reads, Voice, VoiceKey};

pub const RATE: u32 = 48_000;
/// Soft-clip knee: −1 dBFS.
const KNEE: f32 = 0.891;
/// The shortest fade at a clip's edges and wherever a voice starts mid-clip (a seek): 3 ms.
pub const RAMP: i64 = 144;

/// Timeline µs → frames at `RATE`.
pub fn to_frame(us: i64) -> i64 {
    (us as f64 * f64::from(RATE) / 1e6).round() as i64
}

/// Whether `b` carries on exactly where `a` stops: a clip split with nothing removed (same file, speed, rate, effect and
/// channels, no fades at the cut, the source running on). One voice plays both, with no ramp at the cut: a split of a
/// drifting run too, whose second part starts that much further into its file.
fn continues(a: &AudioSource, b: &AudioSource) -> bool {
    let frames = (to_frame(a.end_us) - to_frame(a.start_us)).max(0) as u64;
    let source_end = read_position_after(a, frames);
    a.path == b.path
        && a.speed == b.speed
        && a.rate == b.rate
        && a.filter == b.filter
        && a.centre_from == b.centre_from
        && a.end_us == b.start_us
        && a.fade_out_us == 0
        && b.fade_in_us == 0
        && (to_frame(b.source_in_us) as f64 - source_end).abs() <= 1.0
}

/// Renders the timeline's sound block by block (see the module docs).
pub struct Mixdown {
    plan: Arc<AudioPlan>,
    voices: Vec<Option<Voice>>,
    /// For each source, the source it continues and the one continuing it (`continues`).
    after: Vec<Option<usize>>,
    before: Vec<Option<usize>>,
    /// Ramp in voices that start mid-clip (playback and export); off for one-off reads.
    seat_ramps: bool,
    scratch: Vec<f32>,
    /// What voices read their files into, shared: they render one after another.
    reads: Reads,
    sweep: Sweep,
}

/// Which sources a block has to look at (see the module docs). Between blocks, `active` holds every source that
/// started before the last block's end and ends after it, and every source holding a voice; the rest have no voice
/// and are silent until the cursor reaches them, so the old walk over every source did nothing for them.
#[derive(Default)]
struct Sweep {
    /// Each source's span in timeline frames.
    spans: Vec<(i64, i64)>,
    /// Source indices by start, and the furthest end within each prefix of that order (non-decreasing).
    order: Vec<u32>,
    reach: Vec<i64>,
    /// How much of `order` has started by the last block's end.
    cursor: usize,
    /// The frame the last block ended at (a block starting there continues the sweep; any other jumps).
    next: Option<i64>,
    /// Sorted, without repeats, while a block mixes.
    active: Vec<usize>,
    /// Tests: look at every source in every block (the walk this replaced), and count the sources looked at.
    #[cfg(test)]
    walk_all: bool,
    #[cfg(test)]
    looked: u64,
}

impl Sweep {
    fn new(plan: &AudioPlan) -> Self {
        let spans: Vec<(i64, i64)> = plan
            .sources
            .iter()
            .map(|s| (to_frame(s.start_us), to_frame(s.end_us)))
            .collect();
        let mut order: Vec<u32> = (0..spans.len() as u32).collect();
        order.sort_by_key(|&i| spans[i as usize].0);
        let mut furthest = i64::MIN;
        let reach = order
            .iter()
            .map(|&i| {
                furthest = furthest.max(spans[i as usize].1);
                furthest
            })
            .collect();
        Self {
            spans,
            order,
            reach,
            ..Self::default()
        }
    }

    /// Brings `active` up to the block `[from, to)`: sources that start in it join; after a jump, every source
    /// sounding there joins too (those already in `active` stay, so a voice left over from before the jump goes).
    fn enter(&mut self, from: i64, to: i64) {
        #[cfg(test)]
        if self.walk_all {
            // Every source, in order, without allocating (as the walk did).
            self.active.clear();
            self.active.extend(0..self.spans.len());
            self.looked += self.active.len() as u64;
            self.next = Some(to);
            return;
        }
        let start = |i: &u32| self.spans[*i as usize].0;
        if self.next == Some(from) {
            while let Some(i) = self.order.get(self.cursor).filter(|i| start(i) < to) {
                self.active.push(*i as usize);
                self.cursor += 1;
            }
        } else {
            self.cursor = self.order.partition_point(|i| start(i) < to);
            // Every source before `first` in start order ends at or before `from`.
            let first = self.reach[..self.cursor].partition_point(|&end| end <= from);
            let spans = &self.spans;
            self.active.extend(
                self.order[first..self.cursor]
                    .iter()
                    .map(|&i| i as usize)
                    .filter(|&i| spans[i].1 > from),
            );
        }
        self.active.sort_unstable();
        self.active.dedup();
        #[cfg(test)]
        {
            self.looked += self.active.len() as u64;
        }
        self.next = Some(to);
    }
}

/// The `continues` links between a plan's sources.
fn links(plan: &AudioPlan) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let n = plan.sources.len();
    let (mut after, mut before) = (vec![None; n], vec![None; n]);
    let ends: HashMap<(&str, i64), usize> = plan
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| ((s.path.as_str(), s.end_us), i))
        .collect();
    for (i, b) in plan.sources.iter().enumerate() {
        if let Some(&j) = ends.get(&(b.path.as_str(), b.start_us))
            && j != i
            && continues(&plan.sources[j], b)
            && before[j].is_none()
        {
            after[i] = Some(j);
            before[j] = Some(i);
        }
    }
    (after, before)
}

impl Mixdown {
    pub fn new(plan: Arc<AudioPlan>) -> Self {
        let voices = plan.sources.iter().map(|_| None).collect();
        let (after, before) = links(&plan);
        Self {
            sweep: Sweep::new(&plan),
            plan,
            voices,
            after,
            before,
            seat_ramps: true,
            scratch: Vec::new(),
            reads: Reads::default(),
        }
    }

    pub fn plan(&self) -> &Arc<AudioPlan> {
        &self.plan
    }

    /// Tests: look at every source in every block, as the mixer did before the sweep (the benchmark's "before").
    #[cfg(test)]
    pub(crate) fn walk_every_source(&mut self) {
        self.sweep.walk_all = true;
    }

    /// A new plan (an edit): voices whose source is unchanged keep playing without a break.
    pub fn set_plan(&mut self, plan: Arc<AudioPlan>) {
        let mut old: HashMap<VoiceKey, Voice> = self
            .voices
            .drain(..)
            .flatten()
            .map(|v| (v.key.clone(), v))
            .collect();
        self.voices = plan
            .sources
            .iter()
            .map(|s| old.remove(&VoiceKey::of(s)))
            .collect();
        (self.after, self.before) = links(&plan);
        #[cfg(test)]
        let (walk_all, looked) = (self.sweep.walk_all, self.sweep.looked);
        // The next block finds its sources afresh; the kept voices join so they go if nothing sounds for them.
        self.sweep = Sweep::new(&plan);
        self.sweep.active = (0..self.voices.len())
            .filter(|&i| self.voices[i].is_some())
            .collect();
        #[cfg(test)]
        {
            (self.sweep.walk_all, self.sweep.looked) = (walk_all, looked);
        }
        self.plan = plan;
    }

    /// Hands source `i`'s voice to the source continuing it, if that one starts where the voice is.
    fn hand_on(&mut self, i: usize) {
        let Some(k) = self.before[i] else { return };
        let start = to_frame(self.plan.sources[k].start_us);
        if self.voices[k].is_none() && self.voices[i].as_ref().is_some_and(|v| v.next == start) {
            let mut voice = self.voices[i].take();
            if let Some(v) = &mut voice {
                v.key = VoiceKey::of(&self.plan.sources[k]);
            }
            self.voices[k] = voice;
        }
    }

    /// Sums the plan into `out` (`out.len() / 2` stereo frames from timeline frame `start_frame`) with gains, fades
    /// and ramps; no master gain or clipping (`finish`). Sources whose PCM isn't there (yet) are silent.
    pub fn render(
        &mut self,
        pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
        start_frame: u64,
        out: &mut [f32],
    ) {
        out.fill(0.0);
        let (s0, n) = (start_frame as i64, (out.len() / 2) as i64);
        self.sweep.enter(s0, s0 + n);
        let plan = self.plan.clone();
        // In source order, as the walk over every source went: a split's earlier part hands its voice on first.
        let mut active = std::mem::take(&mut self.sweep.active);
        for &i in &active {
            self.mix_source(i, &plan.sources[i], pcm, s0, n, out);
        }
        // Kept: what sounds past this block, and every voice (one handed on goes to a source that may not be here).
        for k in 0..active.len() {
            if let Some(next) = self.before[active[k]].filter(|&j| self.voices[j].is_some()) {
                active.push(next);
            }
        }
        let (spans, voices) = (&self.sweep.spans, &self.voices);
        active.retain(|&i| spans[i].1 > s0 + n || voices[i].is_some());
        self.sweep.active = active;
    }

    /// Source `i`'s share of the block of `n` frames from `s0` (the walk's body, unchanged).
    fn mix_source(
        &mut self,
        i: usize,
        s: &AudioSource,
        pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
        s0: i64,
        n: i64,
        out: &mut [f32],
    ) {
        let (start, end) = self.sweep.spans[i];
        let (from, to) = (s0.max(start), (s0 + n).min(end));
        if from >= to {
            // Done with it, or not there yet: its voice carries on into a split's next part, or goes (with its
            // filter and mapping).
            self.hand_on(i);
            self.voices[i] = None;
            return;
        }
        if let Some(j) = self.after[i] {
            self.hand_on(j);
        }
        // A cache file that was rebuilt or forgotten: read the current one (re-seated, ramped).
        if self.voices[i]
            .as_ref()
            .is_some_and(|v| v.pcm.is_superseded())
        {
            self.voices[i] = None;
        }
        if self.voices[i].is_none() {
            self.voices[i] = pcm(s).map(|p| Voice::new(s, p));
        }
        let Some(voice) = &mut self.voices[i] else {
            return;
        };
        if voice.next != from {
            voice.seat(s, from, self.seat_ramps);
        }
        let len = (to - from) as usize;
        self.scratch.resize(len * 2, 0.0);
        voice.render(&mut self.scratch, &mut self.reads);
        voice.next = to;
        // A split with nothing removed plays straight through its cut: no edge ramp on either side.
        let ramp_in = if self.after[i].is_some() { 0 } else { RAMP };
        let ramp_out = if self.before[i].is_some() { 0 } else { RAMP };
        let fade_in = to_frame(s.fade_in_us).max(ramp_in) as f32;
        let fade_out = to_frame(s.fade_out_us).max(ramp_out) as f32;
        let (g0, g1) = (voice.gain, s.gain as f32);
        voice.gain = g1;
        let dst = &mut out[((from - s0) * 2) as usize..((to - s0) * 2) as usize];
        for (k, (o, x)) in dst
            .chunks_exact_mut(2)
            .zip(self.scratch.chunks_exact(2))
            .enumerate()
        {
            let f = from + k as i64;
            let mut g = g0 + (g1 - g0) * (k + 1) as f32 / len as f32;
            if fade_in > 0.0 {
                g *= fade_gain((f - start) as f32 / fade_in, s.crossfade.fade_in);
            }
            if fade_out > 0.0 {
                g *= fade_gain((end - f) as f32 / fade_out, s.crossfade.fade_out);
            }
            if let Some(seated) = voice.seated {
                g *= ((f - seated) as f32 / RAMP as f32).min(1.0);
            }
            o[0] += x[0] * g;
            o[1] += x[1] * g;
        }
    }
}

/// A fade's gain `x` of the way from silence to full (clamped to 1): linear, or equal power for a transition's
/// crossfade (`Crossfade`; a fade-out is the same curve read from its end, so it falls as cos).
pub(crate) fn fade_gain(x: f32, equal_power: bool) -> f32 {
    if x >= 1.0 {
        1.0
    } else if equal_power {
        (x * FRAC_PI_2).sin()
    } else {
        x
    }
}

/// Applies the master gain (ramped from `from` to `to` across the block) and the soft clip.
pub fn finish(out: &mut [f32], from: f32, to: f32) {
    let frames = (out.len() / 2).max(1) as f32;
    for (k, lr) in out.chunks_exact_mut(2).enumerate() {
        let g = from + (to - from) * (k + 1) as f32 / frames;
        lr[0] = soft_clip(lr[0] * g);
        lr[1] = soft_clip(lr[1] * g);
    }
}

/// Mixes `out.len() / 2` stereo frames from timeline frame `start_frame` in one go (tests and one-off reads; playback
/// and export keep a `Mixdown`). Sources whose PCM isn't ready are silent.
pub fn mix(
    plan: &AudioPlan,
    pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
    start_frame: u64,
    master_gain: f32,
    out: &mut [f32],
) {
    let mut mixdown = Mixdown::new(Arc::new(plan.clone()));
    mixdown.seat_ramps = false;
    mixdown.render(pcm, start_frame, out);
    finish(out, master_gain, master_gain);
}

/// Linear below −1 dBFS; above it, a tanh knee that never reaches full scale.
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}
