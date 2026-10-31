//! The start gate: writing begins only once every source the person chose sends real samples, so a
//! take never starts recording a black screen or a muted mic as if it were fine. Design from Cap's `start_gate.rs`
//! (sources warm up during the countdown; design only, no code).
//!
//! The session makes the gate once every pipe's `start` has returned (pipes start concurrently, `pipe.rs`), so its 3 s
//! count from then, never from a source's first sample: a source that never sends anything is still asked about. After
//! 3 s with a source still missing, the countdown holds on "{source} isn't sending anything yet." with "Record without
//! it" (`without`) and "Cancel".
//!
//! What counts as real (`GateSample`): a frame for video; for a mic, a block with a sample that isn't exactly zero
//! (a muted or blocked mic sends exact zeros); for game or computer sound, any block, since silence is normal there.
//!
//! Pure: every time is an argument (host ns). Cost: `poll` is O(sources) and allocates only while a source is missing.

use crate::heal::SourceKind;
use crate::media::{PcmBlock, SourceId};

/// How long the gate waits for a missing source before asking.
pub const ASK_AFTER_NS: u64 = 3_000_000_000;

/// A real sample from a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateSample {
    /// A video frame.
    Frame,
    /// Sound: from a mic, a block with any non-zero sample; from game or computer sound, any block (`from_pcm`).
    Sound,
}

impl GateSample {
    /// Whether `block` from a source of `kind` is real sound. Stops at the first non-zero sample.
    pub fn from_pcm(block: &PcmBlock, kind: SourceKind) -> Option<Self> {
        let real = match kind {
            SourceKind::Mic => block.samples.iter().any(|s| *s != 0.0),
            _ => !block.samples.is_empty(),
        };
        real.then_some(Self::Sound)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateState {
    /// Less than 3 s since the pipes started; these haven't sent a real sample yet.
    Waiting { missing: Vec<SourceId> },
    /// 3 s have passed and these still haven't: ask the person.
    Ask { missing: Vec<SourceId> },
    /// Every source is in (or left out by the person): writing may begin.
    Open,
}

#[derive(Clone, Debug)]
pub struct StartGate {
    /// Each requested source, in the order given, and whether it is in: it sent a real sample, or the person chose to
    /// record without it.
    sources: Vec<(SourceId, bool)>,
    started_ns: u64,
}

impl StartGate {
    /// `now_ns`: when every pipe had started.
    pub fn new(sources: &[SourceId], now_ns: u64) -> Self {
        Self {
            sources: sources.iter().map(|s| (s.clone(), false)).collect(),
            started_ns: now_ns,
        }
    }

    /// `source` sent a real sample. Unknown sources are ignored.
    pub fn observe(&mut self, source: &str, _sample: GateSample) {
        self.admit(source);
    }

    /// The person chose "Record without it".
    pub fn without(&mut self, source: &str) {
        self.admit(source);
    }

    /// Ask once 3 s have passed since the pipes started with a source still missing.
    pub fn poll(&self, now_ns: u64) -> GateState {
        if self.sources.iter().all(|(_, ok)| *ok) {
            return GateState::Open;
        }
        let missing = self
            .sources
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(s, _)| s.clone())
            .collect();
        if now_ns.saturating_sub(self.started_ns) >= ASK_AFTER_NS {
            GateState::Ask { missing }
        } else {
            GateState::Waiting { missing }
        }
    }

    fn admit(&mut self, source: &str) {
        for (s, ok) in &mut self.sources {
            if s == source {
                *ok = true;
            }
        }
    }
}
