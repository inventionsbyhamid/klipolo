//! Words on the timeline for one revision. A clip's voice is a list of runs (`voice_runs`): each plays part of the
//! clip's source from one file, with an offset and a rate (clock drift, format 4). A word belongs to a (clip, run)
//! span when its middle is inside the sound time that run plays; its times are clamped to the span.
//!
//! Cost per revision: O(clips × (assets + log words)) to find the spans, then O(words shown + fixes) to list them.

use std::collections::HashMap;

use serde::Serialize;
use ts_rs::TS;

use super::{Speech, Transcript, Word, WordFix, WordKind, CUT_PAD_US};
use crate::ids::Id;
use crate::project::{Project, SourceRole};
use crate::time::{scaled, snap_to_frame, unscaled, RangeUs, Us};
use crate::timeline::{Clip, ClipSource};

/// One stretch of a clip's sound from one file: the clip's source time (the recording's clock) `rec_from..rec_to`
/// plays the file's sound time `(rec − offset_us) × rate + file_in_us`, which its transcript and levels count in.
/// `rate` is positive: 1 plus the file's clock drift. `file_in_us` is where in its file's sound the run starts (a
/// mic that started before the camera); it may be negative: a picture file's sound lead (`Asset::lead_us`) comes off
/// the run's file time, so a run from its picture's start, whose sound starts later, starts before its sound does.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceRun {
    pub asset: Id,
    pub rec_from: Us,
    pub rec_to: Us,
    pub offset_us: Us,
    pub rate: f64,
    pub file_in_us: Us,
}

impl VoiceRun {
    /// The file's sound time at the recording's time `rec`.
    pub fn local_us(&self, rec: Us) -> Us {
        ((rec.saturating_sub(self.offset_us) as f64 * self.rate).round() as Us)
            .saturating_add(self.file_in_us)
    }
}

/// Whose sound carries a recording's voice, best first: the mic, then the sound of the camera, screen or game files,
/// then the game's sound, then the computer's (format 3's mic, camera, screen, system slots).
const VOICE_ROLES: [SourceRole; 6] = [
    SourceRole::Mic,
    SourceRole::Camera,
    SourceRole::Screen,
    SourceRole::Game,
    SourceRole::GameAudio,
    SourceRole::SystemAudio,
];

/// The runs carrying `clip`'s voice, clipped to the part of its source it plays. A recording: the runs of its first
/// source by `VOICE_ROLES` with a file that has sound, each with its drift as the rate and the source's nudge in its
/// offset; an imported file with sound is its own voice (one run, rate 1). A run counts in its file's sound time, as
/// its transcript and levels do: a picture file's sound lead comes off its file time (`Asset::lead_us`).
/// Over a recording clip's true gaps, a picture source whose sound fills them is the voice there.
pub fn voice_runs(p: &Project, clip: &Clip) -> Vec<VoiceRun> {
    let sounding = |id: &Id| p.asset(id).is_some_and(|a| a.has_audio);
    let clipped = |asset: &Id, from: Us, to: Us, offset_us: Us, rate: f64, file_in_us: Us| {
        let (rec_from, rec_to) = (clip.source_in_us.max(from), clip.source_out_us.min(to));
        (rec_from < rec_to).then(|| VoiceRun {
            asset: asset.clone(),
            rec_from,
            rec_to,
            offset_us,
            rate,
            file_in_us,
        })
    };
    match &clip.source {
        // The file covers the clip's source from 0 for its length.
        ClipSource::Asset { id } => p
            .asset(id)
            .filter(|a| a.has_audio)
            .and_then(|a| clipped(&a.id, 0, a.duration_us, 0, 1.0, -a.lead_us()))
            .into_iter()
            .collect(),
        ClipSource::Recording { id } => {
            let Some(r) = p.recording(id) else {
                return Vec::new();
            };
            let voice = VOICE_ROLES.iter().find_map(|&role| {
                r.sources
                    .iter()
                    .find(|s| s.role == role && s.runs.iter().any(|run| sounding(&run.asset_id)))
            });
            let Some(source) = voice else {
                return Vec::new();
            };
            let span = (clip.source_in_us, clip.source_out_us);
            let runs = (source.runs.iter().filter(|run| sounding(&run.asset_id)))
                .filter_map(|run| VoiceRun::of(p, source, run, span));
            // Where the camera's own sound fills a gap the voice didn't record, captions follow it.
            super::fill::with_fills(p, clip, r, runs.collect())
        }
    }
}

/// One (clip, run) stretch of the timeline and the words it plays: `words[first..end]` of `asset`'s transcript. Sent to
/// the UI, which maps word times with the same formula (`spanAt` in `apps/desktop/src/lib/speech.ts`).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeechSpan {
    pub clip: Id,
    pub asset: Id,
    pub first: u32,
    pub end: u32,
    /// The clip's timeline start and source in-point (the recording's time).
    pub start_us: Us,
    pub source_in_us: Us,
    /// The run's mapping: sound time = (recording time − offset) × rate.
    pub offset_us: Us,
    pub rate: f64,
    /// The sound time this span plays (words are chosen by their middle inside it).
    pub asset_in_us: Us,
    pub asset_out_us: Us,
    pub speed: f64,
}

impl SpeechSpan {
    fn unclamped(&self, asset_us: Us) -> Us {
        let rec = ((asset_us as f64 / self.rate).round() as Us).saturating_add(self.offset_us);
        self.start_us
            .saturating_add(scaled(rec.saturating_sub(self.source_in_us), self.speed))
    }

    /// Where the span starts and ends on the timeline.
    pub fn from_us(&self) -> Us {
        self.unclamped(self.asset_in_us)
    }

    pub fn end_us(&self) -> Us {
        self.unclamped(self.asset_out_us)
    }

    /// Where sound time `asset_us` plays: `start + scaled(asset_us / rate + offset − source_in, speed)`, clamped to the
    /// span.
    pub fn at(&self, asset_us: Us) -> Us {
        self.unclamped(asset_us)
            .max(self.from_us())
            .min(self.end_us())
    }

    /// The sound time playing at timeline time `timeline_us` (the inverse of `at` inside the span).
    pub fn local_of(&self, timeline_us: Us) -> Us {
        let rec = unscaled(timeline_us.saturating_sub(self.start_us), self.speed)
            .saturating_add(self.source_in_us);
        (rec.saturating_sub(self.offset_us) as f64 * self.rate).round() as Us
    }
}

fn mid(w: &Word) -> Us {
    w.start_us + (w.end_us - w.start_us) / 2
}

/// Every (clip, run) of the main track, without words (`first = end = 0`): pauses and passages map through these.
pub fn run_spans(p: &Project) -> Vec<SpeechSpan> {
    let mut out = Vec::with_capacity(p.timeline.main.len());
    let mut start_us: Us = 0;
    for clip in &p.timeline.main {
        for run in voice_runs(p, clip) {
            out.push(SpeechSpan {
                clip: clip.id.clone(),
                asset_in_us: run.local_us(run.rec_from),
                asset_out_us: run.local_us(run.rec_to),
                asset: run.asset,
                first: 0,
                end: 0,
                start_us,
                source_in_us: clip.source_in_us,
                // The span's one offset carries the run's start inside its file: file = (rec − offset) × rate.
                offset_us: run
                    .offset_us
                    .saturating_sub((run.file_in_us as f64 / run.rate).round() as Us),
                rate: run.rate,
                speed: clip.speed,
            });
        }
        start_us += clip.duration_us();
    }
    out
}

/// The spans with a transcript and at least one word, their words chosen (two binary searches each).
pub fn spans(p: &Project, speech: &Speech) -> Vec<SpeechSpan> {
    run_spans(p)
        .into_iter()
        .filter_map(|mut s| {
            let t = speech.transcripts.get(&s.asset)?;
            s.first = t.words.partition_point(|w| mid(w) < s.asset_in_us) as u32;
            s.end = t.words.partition_point(|w| mid(w) < s.asset_out_us) as u32;
            (s.end > s.first).then_some(s)
        })
        .collect()
}

/// One word as it plays: `word` indexes its asset's transcript (the first of a fixed run); `fix` indexes
/// `Project::word_fixes` when a fix replaces the run.
///
/// A fix reads once per use of its words. Its pieces are walked in timeline order; a piece that covers words an earlier
/// piece of the current use already covered starts a new use (a copy of the clip, the same range placed again). The
/// first piece of each use carries the text, whichever of the fix's words it holds: a split or a cut of one use reads
/// once, cutting its first word moves the text to the next piece kept, and a copy reads again. Every other piece is
/// `continued`: it keeps its time range, so it can be selected and cut, and shows no text. With every covered word cut,
/// nothing reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpokenWord {
    pub span: u32,
    pub word: u32,
    pub fix: Option<u32>,
    /// A later piece of one use of a fix: no text of its own (captions skip it).
    pub continued: bool,
    pub start_us: Us,
    pub end_us: Us,
    pub kind: WordKind,
}

#[derive(Clone, Debug, Default)]
pub struct Spoken {
    pub spans: Vec<SpeechSpan>,
    pub words: Vec<SpokenWord>,
}

impl Spoken {
    /// What `w` shows: its fix's text (nothing for a `continued` piece), else what was heard. `p` and `speech` are the
    /// ones `self` was made from; given others (a newer revision, a transcript made again), a word they don't have
    /// reads as nothing rather than panic.
    pub fn text<'a>(&self, w: &SpokenWord, p: &'a Project, speech: &'a Speech) -> &'a str {
        word_text(w, p, speech.transcripts.get(self.asset(w)).map(|t| &**t))
    }

    /// Every word with what it shows (`text`), in timeline order, each span's transcript looked up once rather than
    /// once per word (captions walk every word of a revision).
    pub fn shown<'a>(
        &'a self,
        p: &'a Project,
        speech: &'a Speech,
    ) -> impl Iterator<Item = (&'a SpokenWord, &'a str)> + 'a {
        let mut cached: Option<(u32, Option<&'a Transcript>)> = None;
        self.words.iter().map(move |w| {
            let t = match cached {
                Some((span, t)) if span == w.span => t,
                _ => {
                    let t = speech.transcripts.get(self.asset(w)).map(|t| &**t);
                    cached = Some((w.span, t));
                    t
                }
            };
            (w, word_text(w, p, t))
        })
    }

    pub fn asset(&self, w: &SpokenWord) -> &str {
        &self.spans[w.span as usize].asset
    }
}

/// What `w` shows given its span's transcript `t`: the one rule behind `Spoken::text` and `Spoken::shown`.
fn word_text<'a>(w: &SpokenWord, p: &'a Project, t: Option<&'a Transcript>) -> &'a str {
    match w.fix {
        Some(_) if w.continued => "",
        Some(i) => p.word_fixes.get(i as usize).map_or("", |f| f.text.as_str()),
        None => t
            .and_then(|t| t.words.get(w.word as usize))
            .map_or("", |word| word.text.as_str()),
    }
}

/// Each fix's current use: the word ranges its pieces so far cover. The first piece's range is kept inline (`NO_USE`
/// before any), so a fix that plays in one piece allocates nothing; only a use split over clips spills its further
/// ranges into `more`.
struct Uses {
    first: Vec<(u32, u32)>,
    more: HashMap<u32, Vec<(u32, u32)>>,
}

/// A range that covers no word.
const NO_USE: (u32, u32) = (u32::MAX, 0);

impl Uses {
    fn new(fixes: usize) -> Self {
        Self {
            first: vec![NO_USE; fixes],
            more: HashMap::new(),
        }
    }

    /// Whether fix `fix`'s piece over its words `first..=last` starts a new use of it (and so reads it): when its use
    /// has no piece yet, or when this piece covers one of their words again. It joins the current use either way.
    /// O(pieces of the use): a handful, at most one per clip its words play in.
    fn starts(&mut self, fix: u32, first: u32, last: u32) -> bool {
        let covers = |&(a, b): &(u32, u32)| a <= last && first <= b;
        let slot = &mut self.first[fix as usize];
        let fresh = *slot == NO_USE;
        let again = covers(&*slot) || self.more.get(&fix).is_some_and(|m| m.iter().any(covers));
        if again {
            *slot = NO_USE;
            self.more.remove(&fix);
        }
        if *slot == NO_USE {
            *slot = (first, last);
        } else {
            self.more.entry(fix).or_default().push((first, last));
        }
        fresh || again
    }
}

/// The words of the main track in timeline order, fixes applied.
pub fn spoken(p: &Project, speech: &Speech) -> Spoken {
    spoken_from(spans(p, speech), &p.word_fixes, speech)
}

/// `spoken` over given spans in timeline order, as `spans` gives them (the parity fixture adds a drifting one).
/// `fixes` are `Project::word_fixes`. A span without a transcript or words lists none.
pub fn spoken_from(spans: Vec<SpeechSpan>, fixes: &[WordFix], speech: &Speech) -> Spoken {
    let mut by_asset: HashMap<&str, Vec<(u32, &WordFix)>> = HashMap::new();
    for (i, f) in fixes.iter().enumerate() {
        by_asset
            .entry(f.asset_id.as_str())
            .or_default()
            .push((i as u32, f));
    }
    // Each fix's current use: the word ranges its pieces so far cover (no allocation for a fix nothing plays).
    let mut uses = Uses::new(fixes.len());
    let mut words = Vec::with_capacity(
        spans
            .iter()
            .map(|s| s.end.saturating_sub(s.first) as usize)
            .sum(),
    );
    for (si, s) in spans.iter().enumerate() {
        let Some(t) = speech.transcripts.get(&s.asset) else {
            continue;
        };
        let (mut i, end) = (s.first as usize, (s.end as usize).min(t.words.len()));
        if i >= end {
            continue;
        }
        let fx = by_asset
            .get(s.asset.as_str())
            .map_or(&[][..], Vec::as_slice);
        // `SpeechSpan::at` with the span's ends worked out once, not per word.
        let (lo, hi) = (s.from_us(), s.end_us());
        let at = |asset_us: Us| s.unclamped(asset_us).max(lo).min(hi);
        // Fixes are sorted by start and never overlap (validate): one pointer walks them with the words.
        let mut f = fx.partition_point(|(_, x)| x.end_us <= mid(&t.words[i]));
        while i < end {
            let w = &t.words[i];
            let m = mid(w);
            while f < fx.len() && fx[f].1.end_us <= m {
                f += 1;
            }
            if let Some(&(fi, x)) = fx.get(f).filter(|(_, x)| x.start_us <= m) {
                // The fix replaces every word of this span whose middle it covers.
                let mut j = i;
                while j + 1 < end && mid(&t.words[j + 1]) < x.end_us {
                    j += 1;
                }
                let continued = !uses.starts(fi, i as u32, j as u32);
                words.push(SpokenWord {
                    span: si as u32,
                    word: i as u32,
                    fix: Some(fi),
                    continued,
                    start_us: at(w.start_us),
                    end_us: at(t.words[j].end_us),
                    kind: WordKind::Word,
                });
                i = j + 1;
                continue;
            }
            words.push(SpokenWord {
                span: si as u32,
                word: i as u32,
                fix: None,
                continued: false,
                start_us: at(w.start_us),
                end_us: at(w.end_us),
                kind: w.kind,
            });
            i += 1;
        }
    }
    Spoken { spans, words }
}

/// The timeline range that cuts spoken words `from..=to` (`from <= to`): from a little before the first to a little
/// after the last (`CUT_PAD_US`), never more than half the gap to the nearest kept word in the same clip (pauses count
/// as gap, not as words); cutting a pause itself keeps `CUT_PAD_US` (or half of it) at each end, so the words around it
/// don't run together. Clamped to the spans, snapped to frames at `fps`.
pub fn word_cut(spoken: &Spoken, from: usize, to: usize, fps: u32) -> RangeUs {
    let w = &spoken.words;
    let (a, b) = (&w[from], &w[to]);
    let real = |i: usize| w[i].kind != WordKind::Pause;
    let start = if a.kind == WordKind::Pause {
        a.start_us + CUT_PAD_US.min((a.end_us - a.start_us) / 2)
    } else {
        let gap = (0..from)
            .rev()
            .take_while(|&i| w[i].span == a.span)
            .find(|&i| real(i))
            .map_or(CUT_PAD_US * 2, |i| (a.start_us - w[i].end_us).max(0));
        a.start_us - CUT_PAD_US.min(gap / 2)
    };
    let end = if b.kind == WordKind::Pause {
        b.end_us - CUT_PAD_US.min((b.end_us - b.start_us) / 2)
    } else {
        let gap = (to + 1..w.len())
            .take_while(|&i| w[i].span == b.span)
            .find(|&i| real(i))
            .map_or(CUT_PAD_US * 2, |i| (w[i].start_us - b.end_us).max(0));
        b.end_us + CUT_PAD_US.min(gap / 2)
    };
    let start = start.max(spoken.spans[a.span as usize].from_us());
    let end = end.min(spoken.spans[b.span as usize].end_us());
    RangeUs {
        start: snap_to_frame(start, fps),
        end: snap_to_frame(end, fps),
    }
}
