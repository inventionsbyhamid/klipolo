//! A word as an engine heard it, and `snap_to_speech`, which every engine's words go through (moved here from
//! klipolo-speech, so whisper's worker and klipolo-asr share one rule): neither whisper's times nor Parakeet's
//! 80 ms frames see pauses, so a word's start moves past silence to where its sound begins, and its end back to where
//! its sound ends. O(samples) once for the snapping.

use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};

/// A word as the recognizer heard it, in the chunk's own time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawWord {
    pub text: String,
    pub start_us: Us,
    pub end_us: Us,
    pub confidence: f32,
}

/// 10 ms windows of the chunk's 16 kHz samples.
const WINDOW: usize = 160;
const WINDOW_US: i64 = 10_000;
/// A quiet run this long at a word's start is a pause: a soft consonant or a breath never moves it.
const PAUSE_WINDOWS: usize = 10;
/// Inside a word (with sound before it in the word's span) a quiet run is a pause only from this long: a stop closure,
/// a consonant cluster or a geminate ("laptop", "shortcut", Italian or Hindi doubled consonants) lasts 100–250 ms.
const GAP_WINDOWS: usize = 25;
/// Kept before a word's first loud window and after its last (a soft attack, a fading end).
const MARGIN_WINDOWS: usize = 2;
/// Digital silence (every sample 0) this long is a pause even inside a word: the pad a packed window puts between two
/// pieces (klipolo-speech's `plan::PAD_US`, 200 ms, less a window at each edge it may only partly cover; its
/// `tests/tokens.rs` keeps the two in step). No spoken word holds dead silence, so over noise or music, where the word
/// before ends in sound, the next piece's first word still starts after its pad
/// 1). A real closure (150 ms of zeros in a test's "laptop") stays shorter.
pub const DEAD_PAUSE_US: Us = 180_000;
const DEAD_WINDOWS: usize = (DEAD_PAUSE_US / WINDOW_US) as usize;
/// A window's level when every sample in it is 0.
const DEAD_DB: f32 = -120.0;

/// Moves each word's start past a pause at its beginning to where its own sound begins, and its end back from a pause
/// at its end, inside the word's own span (never past the other end). The word's sound is its final loud stretch: from
/// the last quiet run of `GAP_WINDOWS` (250 ms) before its last loud window (or of `DEAD_WINDOWS` of digital silence:
/// a packed window's pad), or from a run of `PAUSE_WINDOWS` (100 ms) at its start. So a click or a breath earlier in a
/// pause is never taken for the word, and a closure inside the word ("laptop") never cuts its first syllable off. A
/// window is quiet below the louder of −50 dBFS and the chunk's loudest window − 30 dB, so steady room tone or music
/// never reads as silence (nothing moves). A word with no loud window keeps its times. O(samples) once, then O(the
/// words' windows).
pub fn snap_to_speech(words: &mut [RawWord], samples: &[f32]) {
    let db: Vec<f32> = samples
        .chunks(WINDOW)
        .map(|w| {
            let ms = w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32;
            if ms > 0.0 {
                10.0 * ms.log10()
            } else {
                DEAD_DB
            }
        })
        .collect();
    let Some(loudest) = db.iter().copied().reduce(f32::max) else {
        return;
    };
    let quiet_below = (loudest - 30.0).max(-50.0);
    let loud = |i: usize| db.get(i).is_some_and(|&d| d >= quiet_below);
    for w in words.iter_mut() {
        let a = (w.start_us.max(0) / WINDOW_US) as usize;
        let b = ((w.end_us.max(0) + WINDOW_US - 1) / WINDOW_US) as usize;
        let Some(last) = (a..b).rev().find(|&i| loud(i)) else {
            continue;
        };
        // Back from the last loud window: the sound starts right after the latest quiet run of `GAP_WINDOWS` (or dead
        // run of `DEAD_WINDOWS`), or after one of `PAUSE_WINDOWS` that reaches the word's start.
        let (mut quiet, mut dead) = (0, 0);
        let inner = (a..=last).rev().find_map(|i| {
            quiet = if loud(i) { 0 } else { quiet + 1 };
            dead = if db[i] <= DEAD_DB { dead + 1 } else { 0 };
            if quiet == GAP_WINDOWS {
                Some(i + GAP_WINDOWS)
            } else {
                (dead == DEAD_WINDOWS).then_some(i + DEAD_WINDOWS)
            }
        });
        let onset = match inner {
            Some(onset) => onset,
            None if quiet >= PAUSE_WINDOWS => a + quiet, // the run the scan ended in reaches `a`
            None => a,
        };
        if onset > a {
            w.start_us = w.start_us.max((onset - MARGIN_WINDOWS) as i64 * WINDOW_US);
        }
        if b - 1 - last >= PAUSE_WINDOWS {
            w.end_us = w.end_us.min((last + 1 + MARGIN_WINDOWS) as i64 * WINDOW_US);
        }
        w.end_us = w.end_us.max(w.start_us + 1);
    }
}

/// The envelope's hop: 5 ms at 16 kHz.
pub const EDGE_HOP: usize = 80; // 5 ms at 16 kHz
/// The least rise (or fall) of the envelope, in dB from the 10 ms before an edge to the 10 ms after it, that moves it.
pub const RISE_DB: f32 = 6.0;
/// Parakeet's encoder step: the most an edge moves either way.
const STEP_US: Us = 80_000;
/// Kept before an onset and after an offset (a soft attack, a fading end).
const MARGIN_US: Us = 10_000;
/// A refined word is never shorter.
const MIN_WORD_US: Us = 30_000;
const HOP_US: Us = 5_000;
/// The envelope's floor, under its loud level (`LOUD_SHARE`): it bounds a rise out of digital silence (a packed
/// window's pad) at about 40 dB instead of 100. It doesn't make that rise no onset: room tone louder than the floor
/// still rises out of a pad.
const FLOOR_BELOW_DB: f32 = 40.0;
/// The window's loud level is its hop energy at this share, not its loudest hop: one clap, mic bump or hit (a few
/// hops) never lifts the floor over a quiet word's onset.
const LOUD_SHARE: f32 = 0.95;
/// A rise holds: every 10 ms mean from 10 to 50 ms after its edge is at most `DROP_DB` under
/// its loudest 10 ms at the edge, and still `RISE_DB` over the 10 ms before it. A word's sound stays up; a hit decays.
/// Measured against the hit's own onset, not the dip before it, so a loud hit's tail can't pass for a word.
pub const DROP_DB: f32 = 6.0;
const THROUGH_FROM: usize = 2;
const THROUGH_TO: usize = 8;
/// A fall holds: the hops 30–50 ms after its edge stay `RISE_DB` under the 10 ms before it.
const HELD_FROM: usize = 6;
const HELD_TO: usize = 10;
/// y[n] = x[n] − 0.97·x[n−1]: a consonant's high frequencies count as much as a vowel's low ones.
const PRE_EMPHASIS: f32 = 0.97;
/// Where speech moves but no digital silence is (connected speech, a room, a bed), a start leads Parakeet's own time by
/// this much. On LibriSpeech, Parakeet's starts are 20 ms late at the median and an envelope's rise is no
/// better a start than its frame: the rise is a consonant's release or a vowel's onset, where a forced alignment puts
/// the word's start before it. Leading by 10 ms scored best of the rules tried (median 40 ms, p90 140 ms, against
/// 50 and 140 unrefined and 50 and 150 on the rise).
pub const LEAD_US: Us = 10_000;

/// Parakeet's word edges sit on 80 ms encoder steps. Each start moves to the strongest rise of the
/// 5 ms log-energy envelope (pre-emphasised, so a consonant's onset counts) within one step either side, if that rise
/// is at least `RISE_DB`; each end to the strongest fall. Never past the word before or after, never shorter than
/// 30 ms. Runs after `snap_to_speech` (which handles pauses of 100 ms and more). O(samples) for the envelope, then
/// O(32) per edge; one Vec of `samples / 80` floats (and one copy of it while the floor is found), and one bool a hop.
///
/// - **Only at digital silence.** A start moves to a rise only out of digital silence (every sample 0: a packed
///   packed window's pad, a noise gate's zeros), and an end to a fall only into it: there the sound's own edge is
///   unambiguous. Elsewhere, where a held rise shows speech moving, a start leads Parakeet's time by `LEAD_US`, and an
///   end stays. LibriSpeech showed an envelope's rise in connected speech is no better than Parakeet's frame.
/// - A rise is the mean of the two hops after an edge less the mean of the two before it, so an abrupt onset peaks at
///   its own hop and a 10 ms attack still counts in full; a fall is the reverse. Ties take the earliest rise and the
///   latest fall.
/// - Two joined words (one's end at or past the next one's start, as Parakeet's pieces give them) share one edge,
///   moved as the second word's start, and both words' edges move with it, each at most a step.
/// - Only a held rise counts (`DROP_DB`): for 10–50 ms after the edge its sound stays within 6 dB of its onset's level
///   and over the dip. A click, a hi-hat, a clap or a snare (decays to about 40 ms) at least 40 ms before a word's
///   onset is no onset; a hit within about 35 ms of the onset merges with it, and one decaying slower than about 50 ms
///   can still take an edge, at most a step. A fall must stay down for 30–50 ms (`HELD_FROM`, `HELD_TO`).
/// - The envelope has a floor `FLOOR_BELOW_DB` under its loud level (`LOUD_SHARE`), which bounds a rise out of digital
///   silence. Room tone louder than that still rises out of a pad, so a start next to a packed window's pad can move
///   onto the pad's end (at most a step; the remap then clamps it to its piece's start).
pub fn refine_edges(words: &mut [RawWord], samples: &[f32]) {
    if words.is_empty() {
        return;
    }
    let (env, dead) = envelope(samples);
    if env.len() < 4 {
        return;
    }
    let len_us = samples.len() as Us * WINDOW_US / WINDOW as Us;
    // Where the word before ends, once its edges are final: nothing moves before it.
    let mut floor = 0;
    // The word before shared its end with this word's start, which moved with it.
    let mut shared = false;
    for i in 0..words.len() {
        if !shared {
            let w = &words[i];
            let (t, lo) = (w.start_us, (w.start_us - STEP_US).max(floor));
            let hi = (t + STEP_US).min(w.end_us - MIN_WORD_US);
            if lo <= hi {
                if let Some(edge) = start_edge(&env, &dead, t, lo, hi) {
                    words[i].start_us = edge;
                }
            }
        }
        let (start, end) = (words[i].start_us, words[i].end_us);
        shared = words.get(i + 1).is_some_and(|next| end >= next.start_us);
        if shared {
            let next = &words[i + 1];
            let t = next.start_us;
            let lo = (t - STEP_US).max(end - STEP_US).max(start + MIN_WORD_US);
            let hi = (t + STEP_US)
                .min(end + STEP_US)
                .min(next.end_us - MIN_WORD_US);
            if lo <= hi {
                if let Some(edge) = start_edge(&env, &dead, t, lo, hi) {
                    words[i].end_us = edge;
                    words[i + 1].start_us = edge;
                }
            }
        } else {
            let next_start = words.get(i + 1).map_or(len_us, |n| n.start_us);
            let lo = (end - STEP_US).max(start + MIN_WORD_US);
            let hi = (end + STEP_US).min(next_start).min(len_us);
            if lo <= hi {
                if let Some(offset) = strongest(&env, &dead, Edge::Fall, lo - MARGIN_US, hi, true) {
                    words[i].end_us = (offset + MARGIN_US).clamp(lo, hi);
                }
            }
        }
        floor = words[i].end_us;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Edge {
    Rise,
    Fall,
}

/// Where a start at `t` (bounds `lo`, `hi`) goes: to a held rise out of digital silence, less the margin; else, when a
/// held rise shows speech moving there, `LEAD_US` before `t`; else nowhere.
fn start_edge(env: &[f32], dead: &[bool], t: Us, lo: Us, hi: Us) -> Option<Us> {
    if let Some(onset) = strongest(env, dead, Edge::Rise, lo, hi + MARGIN_US, true) {
        return Some((onset - MARGIN_US).clamp(lo, hi));
    }
    strongest(env, dead, Edge::Rise, lo, hi + MARGIN_US, false).map(|_| (t - LEAD_US).clamp(lo, hi))
}

/// The time of the hop edge in `[from, to]` with the strongest held rise (or fall) of at least `RISE_DB`, if one is;
/// with `silence`, only one out of (into) a hop of digital silence.
fn strongest(
    env: &[f32],
    dead: &[bool],
    edge: Edge,
    from: Us,
    to: Us,
    silence: bool,
) -> Option<Us> {
    // An edge k needs the two hops before it and the hops to 50 ms after it: 2 ≤ k ≤ len − HELD_TO.
    let first = ((from.max(0) + HOP_US - 1) / HOP_US).max(2) as usize;
    let last = ((to.max(0) / HOP_US) as usize).min(env.len().saturating_sub(HELD_TO));
    // The 10 ms from hop j.
    let ten = |j: usize| (env[j] + env[j + 1]) / 2.0;
    let mut best: Option<(usize, f32)> = None;
    for k in first..=last {
        if silence
            && !(if edge == Edge::Rise {
                dead[k - 1]
            } else {
                dead[k]
            })
        {
            continue;
        }
        let before = ten(k - 2);
        let size = match edge {
            Edge::Rise => {
                let through = (k + THROUGH_FROM..=k + THROUGH_TO)
                    .map(ten)
                    .fold(f32::INFINITY, f32::min);
                if through - before < RISE_DB || ten(k).max(ten(k + 1)) - through > DROP_DB {
                    continue;
                }
                ten(k) - before
            }
            Edge::Fall => {
                let held = env[k + HELD_FROM..k + HELD_TO].iter().sum::<f32>()
                    / (HELD_TO - HELD_FROM) as f32;
                if before - held < RISE_DB {
                    continue;
                }
                before - ten(k)
            }
        };
        let better = match best {
            None => true,
            Some((_, b)) if edge == Edge::Rise => size > b,
            Some((_, b)) => size >= b,
        };
        if better {
            best = Some((k, size));
        }
    }
    best.filter(|&(_, size)| size >= RISE_DB)
        .map(|(k, _)| k as Us * HOP_US)
}

/// The pre-emphasised energy of each `EDGE_HOP`, in dB, floored `FLOOR_BELOW_DB` under the loud level (`LOUD_SHARE`;
/// one copy of the energies for the selection, O(hops)), and whether each hop is digital silence (every sample 0).
fn envelope(samples: &[f32]) -> (Vec<f32>, Vec<bool>) {
    let mut env = Vec::with_capacity(samples.len().div_ceil(EDGE_HOP));
    let mut dead = Vec::with_capacity(env.capacity());
    let mut before = 0.0f32;
    for hop in samples.chunks(EDGE_HOP) {
        let mut energy = 0.0f32;
        for &x in hop {
            let y = x - PRE_EMPHASIS * before;
            before = x;
            energy += y * y;
        }
        env.push(energy / hop.len() as f32);
        dead.push(hop.iter().all(|&x| x == 0.0));
    }
    let loud = if env.is_empty() {
        0.0
    } else {
        let mut sorted = env.clone();
        let at = ((sorted.len() as f32 * LOUD_SHARE) as usize).min(sorted.len() - 1);
        *sorted.select_nth_unstable_by(at, f32::total_cmp).1
    };
    let floor = (loud * 10f32.powf(-FLOOR_BELOW_DB / 10.0)).max(1e-12);
    for e in &mut env {
        *e = 10.0 * e.max(floor).log10();
    }
    (env, dead)
}
