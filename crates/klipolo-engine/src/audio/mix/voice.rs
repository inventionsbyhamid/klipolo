//! One sounding source in the mix: a straight read of its PCM at 1×, a resampled read at a drifting device's rate, or a
//! live `Stretch` (speed change and effect) fed from it, with pre-roll after every (re)seat. A source played centred
//! from one channel (`AudioSource::centre_from`) reads that channel alone, chosen once per voice, and from there plays
//! exactly as a one-channel file does.

use std::sync::Arc;

use super::{RATE, to_frame};
use crate::audio::pcm::Pcm;
use crate::audio::plan::AudioSource;
use crate::audio::stretch::{Stretch, is_unit};

/// Source fed to a live filter before the first frame heard after a (re)start, so it has settled.
const PREROLL: i64 = 4_800;
/// Source frames per filter push.
const CHUNK: usize = 1_024;

/// Where source `s` reads after `frames` frames of the timeline from its start: its first source frame, plus `frames` at
/// `speed × rate` (`AudioSource::step`). A voice streaming the source from its start reads there.
pub fn read_position_after(s: &AudioSource, frames: u64) -> f64 {
    to_frame(s.source_in_us) as f64 + frames as f64 * s.step()
}

/// What makes a voice the same across plan changes (gain, fades and the end only shape its envelope).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct VoiceKey {
    path: String,
    start_us: i64,
    source_in_us: i64,
    speed_bits: u64,
    rate_bits: u64,
    filter: Option<Arc<str>>,
    centre_from: Option<u8>,
}

impl VoiceKey {
    pub(super) fn of(s: &AudioSource) -> Self {
        Self {
            path: s.path.clone(),
            start_us: s.start_us,
            source_in_us: s.source_in_us,
            speed_bits: s.speed.to_bits(),
            rate_bits: s.rate.to_bits(),
            filter: s.filter.clone(),
            centre_from: s.centre_from,
        }
    }
}

pub(super) struct Voice {
    pub(super) key: VoiceKey,
    pub(super) pcm: Arc<Pcm>,
    /// The channels it plays: the file's, or 1 when it plays one of them centred (`Reader::pick`).
    channels: usize,
    filter: Option<Stretch>,
    /// The timeline frame the next block continues from; any other start re-seats the voice.
    pub(super) next: i64,
    /// Where the voice was last (re)started mid-clip (ramped in from there).
    pub(super) seated: Option<i64>,
    /// The gain at the end of the last block (a change ramps across the next one).
    pub(super) gain: f32,
    reader: Reader,
    /// For a filter: its output waiting (in the channels it plays), and how many frames of it to drop.
    queue: Vec<f32>,
    drop: usize,
}

/// Where a voice reads its file next.
#[derive(Clone, Copy, Debug)]
enum Read {
    /// Frame after frame from source frame `feed`: at 1×, or into a stretch (which plays the speed and the rate).
    Straight { feed: i64 },
    /// `rate` source frames per frame read (a drifting device's clock at 1×), each interpolated between the two
    /// source frames around it: from fractional source frame `origin` at the last (re)seat, `done` frames read since.
    /// Each block starts at `origin + done × rate`, a product rather than a running sum, so an hour of reads adds up
    /// no rounding (it stays within 10⁻⁷ of a frame).
    Resampled { origin: f64, done: u64, rate: f64 },
}

/// The buffers voices read into, one set per `Mixdown` and shared by its voices, which render one after another:
/// sized by the block, never by where a voice reads, so neither a playing voice nor a new one
/// allocates once the mix has rendered a block of that size.
#[derive(Default)]
pub(super) struct Reads {
    /// The source frames under a resampled block (and the one after its last).
    raw: Vec<f32>,
    /// The frames read.
    out: Vec<f32>,
}

/// Reads a voice's file block by block into the channels it plays (`Reads`): O(frames × channels) per block, one
/// multiply-add and a linear interpolation per frame when resampling, and one copy per frame to pick a channel.
struct Reader {
    at: Read,
    /// The file channel it reads alone, for a source played centred from it (`picked`); None reads every channel.
    pick: Option<usize>,
}

/// The file channel a voice of `s` reads alone (`AudioSource::centre_from`): one the file has, in a file of more than
/// one; a one-channel file already plays on both sides.
fn picked(s: &AudioSource, pcm: &Pcm) -> Option<usize> {
    let ch = pcm.channels();
    s.centre_from.map(usize::from).filter(|&k| ch > 1 && k < ch)
}

impl Reader {
    /// The next `n` frames, in `reads`: in the file's channels, or in the one it picks.
    fn fill<'r>(&mut self, pcm: &Pcm, n: usize, reads: &'r mut Reads) -> &'r [f32] {
        let ch = pcm.channels();
        let played = if self.pick.is_some() { 1 } else { ch };
        let Reads { raw, out } = reads;
        match &mut self.at {
            Read::Straight { feed } => {
                out.resize(n * ch, 0.0);
                pcm.read(*feed, out);
                *feed += n as i64;
                if let Some(k) = self.pick {
                    keep(out, ch, k);
                }
            }
            Read::Resampled { origin, done, rate } => {
                let rate = *rate;
                let first = *origin + *done as f64 * rate;
                let base = first.floor();
                // The most source frames `n` reads can span, plus the neighbour after the last: the same every block.
                let span = (n as f64 * rate).ceil() as usize + 2;
                raw.resize(span * ch, 0.0);
                pcm.read(base as i64, raw);
                // A picked channel is interpolated on its own, as a one-channel file's is.
                let from = match self.pick {
                    Some(k) => keep(raw, ch, k),
                    None => &raw[..],
                };
                out.resize(n * played, 0.0);
                lerp(from, played, first - base, rate, out);
                *done += n as u64;
            }
        }
        &out[..n * played]
    }

    /// The source frame it reads next.
    fn next_frame(&self) -> i64 {
        match self.at {
            Read::Straight { feed } => feed,
            Read::Resampled { origin, done, rate } => (origin + done as f64 * rate).floor() as i64,
        }
    }
}

/// Channel `k` of `frames` (interleaved, `ch` channels) on its own, moved to the front: in place, one copy per frame
/// (frame `i`'s sample is read before anything is written over it, as `i ≤ i·ch + k`).
fn keep(frames: &mut [f32], ch: usize, k: usize) -> &[f32] {
    let n = frames.len() / ch;
    for i in 0..n {
        frames[i] = frames[i * ch + k];
    }
    &frames[..n]
}

/// `out`'s frames read from `raw` (frames of `ch` channels) at `rate` frames apart, the first at fractional frame `at`,
/// each interpolated between the two frames around it. Within a block the position steps in 32.32 fixed point (whole
/// frames above, the fraction below): the step's rounding stays under 10⁻⁶ of a frame over a 4,096-frame block, and
/// every block starts again from the voice's exact position. Mono and stereo have their own loops; per frame, a shift,
/// one conversion and a multiply-add per channel.
fn lerp(raw: &[f32], ch: usize, at: f64, rate: f64, out: &mut [f32]) {
    const ONE: f64 = (1u64 << 32) as f64;
    const FRACTION: f32 = 1.0 / (1u64 << 32) as f32;
    // Never past the last frame's neighbour, whatever the rounding.
    let last = raw.len() / ch - 2;
    let (mut pos, step) = ((at * ONE) as u64, (rate * ONE).round() as u64);
    let mut next = || {
        let i = ((pos >> 32) as usize).min(last);
        let t = (pos as u32) as f32 * FRACTION;
        pos += step;
        (i, t)
    };
    match ch {
        1 => {
            for o in out.iter_mut() {
                let (i, t) = next();
                let ab = &raw[i..i + 2];
                *o = ab[0] + (ab[1] - ab[0]) * t;
            }
        }
        2 => {
            for o in out.chunks_exact_mut(2) {
                let (i, t) = next();
                let ab = &raw[2 * i..2 * i + 4];
                o[0] = ab[0] + (ab[2] - ab[0]) * t;
                o[1] = ab[1] + (ab[3] - ab[1]) * t;
            }
        }
        _ => {
            for o in out.chunks_exact_mut(ch) {
                let (i, t) = next();
                let ab = &raw[i * ch..(i + 2) * ch];
                let (a, b) = ab.split_at(ch);
                for ((o, &a), &b) in o.iter_mut().zip(a).zip(b) {
                    *o = a + (b - a) * t;
                }
            }
        }
    }
}

impl Voice {
    pub(super) fn new(s: &AudioSource, pcm: Arc<Pcm>) -> Voice {
        let pick = picked(s, &pcm);
        Voice {
            key: VoiceKey::of(s),
            channels: if pick.is_some() { 1 } else { pcm.channels() },
            pcm,
            filter: None,
            next: i64::MIN,
            seated: None,
            gain: s.gain as f32,
            reader: Reader {
                at: Read::Straight { feed: 0 },
                pick,
            },
            queue: Vec::new(),
            drop: 0,
        }
    }

    /// Starts reading at timeline frame `at`. A speed change runs the stretch at the drift-corrected tempo
    /// (`speed × rate`), fed straight from the file; at 1× the read itself is resampled at `rate` (when it isn't 1),
    /// and an effect takes it from there.
    pub(super) fn seat(&mut self, s: &AudioSource, at: i64, ramp: bool) {
        let start = to_frame(s.start_us);
        let source = read_position_after(s, 0) + (at - start) as f64 * s.step();
        self.seated = (ramp && at > start).then_some(at);
        self.next = at;
        self.queue.clear();
        let stretch = !is_unit(s.speed);
        let tempo = if stretch { s.step() } else { 1.0 };
        self.filter = Stretch::new(self.channels, tempo, s.filter.as_deref())
            .map_err(|error| tracing::warn!(%error, "audio filter"))
            .ok()
            .flatten();
        let pre = if self.filter.is_some() { PREROLL } else { 0 };
        self.reader.at = if stretch || s.rate == 1.0 {
            Read::Straight {
                feed: source.round() as i64 - pre,
            }
        } else {
            Read::Resampled {
                origin: source - pre as f64 * s.rate,
                done: 0,
                rate: s.rate,
            }
        };
        self.drop = (pre as f64 / tempo).round() as usize;
        self.pcm
            .will_need(self.reader.next_frame(), RATE as usize * 2);
    }

    /// `out.len() / 2` stereo frames from where the voice is, read through the mix's shared `reads`.
    pub(super) fn render(&mut self, out: &mut [f32], reads: &mut Reads) {
        let ch = self.channels;
        let n = out.len() / 2;
        let Some(filter) = &mut self.filter else {
            let read = self.reader.fill(&self.pcm, n, reads);
            stereo(read, ch, out);
            return;
        };
        // The filter's output keeps the channels it is fed; it is spread to stereo at unity like a straight read.
        while self.queue.len() < n * ch {
            let read = self.reader.fill(&self.pcm, CHUNK, reads);
            let (queue, drop) = (&mut self.queue, &mut self.drop);
            let pushed = filter.push(read, &mut |s: &[f32]| {
                let skip = (*drop).min(s.len() / ch);
                *drop -= skip;
                queue.extend_from_slice(&s[skip * ch..]);
            });
            if let Err(error) = pushed {
                tracing::warn!(%error, "audio filter");
                self.filter = None;
                out.fill(0.0);
                return;
            }
        }
        stereo(&self.queue[..n * ch], ch, out);
        self.queue.drain(..n * ch);
    }
}

/// Mono or stereo samples to stereo: a mono sample (a one-channel file, or the channel a source plays centred from)
/// plays on both sides at its own level.
fn stereo(native: &[f32], channels: usize, out: &mut [f32]) {
    if channels == 1 {
        for (o, &x) in out.chunks_exact_mut(2).zip(native) {
            o[0] = x;
            o[1] = x;
        }
    } else {
        out.copy_from_slice(&native[..out.len()]);
    }
}
