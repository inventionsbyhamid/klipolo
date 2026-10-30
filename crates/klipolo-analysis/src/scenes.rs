//! A clip's scene log: what happens and when (who is on screen, what they say that matters, actions, reactions, text on
//! screen, changes of topic), for the chat's "find the part where...", highlights, and titles and sound effects placed
//! at moments.
//!
//! A `SceneLogger` (Muse Spark through the Meta adapter) watches each chunk of the clip's small copy (`proxy`) with its
//! sound. Its moments come back timed to about half a second, so each item's start and end are snapped to the clip's own
//! evidence: a hard cut within 1.5 s (`cuts`, to the frame) or a word edge within 1 s (the transcript's words, to tens
//! of milliseconds). Speech, reactions and actions try the words first, the rest the cuts first (`snap`).
//!
//! - **Frame rate:** Spark samples 2 frames a second for a clip up to 5 minutes, 1 above (half the cost; cuts are still
//!   found to the frame here).
//! - **Chunks:** at most 10 minutes, each overlapping the next by 20 s. Spark is unreliable in a chunk's last seconds,
//!   so a chunk's items that start in its last 3 s are dropped unless it is the clip's last chunk; from there on the
//!   next chunk's items are used. The next chunk's items before that point, and its copy of a moment the chunk before
//!   logged (same kind, starting within 2 s; the nearest such), are dropped; such a copy lends its end to the item it
//!   repeats, which the chunk's end may have cut off. An item of another kind lends nothing.
//! - **Calls:** as many chunks at once as the logger takes (one through the relay). The first failure ends the log
//!   with that error (a used-up credit included); nothing is retried here beyond the adapter's busy retries.
//! - **Cuts** come with the copy (`ProxyInfo::cuts`, found in every frame of the source); a copy without any leaves
//!   the log snapped to the words alone.
//!
//! **Times** are the clip's file time (its picture's clock, the first frame at 0), which is the small copy's time plus
//! its chunk's start. Words are in sound time; `word_edges` is the one place they are turned into file time, by the
//! clip's audio lead.

use futures::{StreamExt, TryStreamExt};
use klipolo_model::speech::{Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use serde::{Deserialize, Serialize};

use crate::error::AnalysisError;
use crate::providers::{SceneKind, SceneLogger, SceneMoment};
use crate::proxy::{self, CHUNK_US, OVERLAP_US, ProxyInfo};

/// A chunk's last seconds, where Spark is unreliable: items starting there are dropped unless it is the last chunk.
pub const UNRELIABLE_END_US: Us = 3_000_000;
/// Clips up to this long are sampled at `FPS_SHORT`, longer ones at `FPS_LONG`.
pub const SHORT_CLIP_US: Us = 300_000_000;
pub const FPS_SHORT: f64 = 2.0;
pub const FPS_LONG: f64 = 1.0;
/// How far an item's edge moves to a cut.
pub const CUT_SNAP_US: Us = 1_500_000;
/// How far an item's edge moves to a word's edge (when no cut is near).
pub const WORD_SNAP_US: Us = 1_000_000;
/// Two chunks' items of one kind starting this close together in their overlap are one moment.
pub const SAME_MOMENT_US: Us = 2_000_000;

/// One moment of the clip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneItem {
    /// File time (see the module notes).
    pub start_us: Us,
    /// At or after `start_us` (equal for an instant, such as a cut).
    pub end_us: Us,
    pub kind: SceneKind,
    /// Who it is about; empty when no one in particular.
    pub who: String,
    /// What happens, in one line.
    pub text: String,
    /// The text on screen during it, as written; empty for none.
    pub on_screen_text: String,
}

/// A clip's scene log, as the cache keeps it (`scenes.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneLog {
    /// The model that watched (`SceneLogger::model`).
    pub model: String,
    /// The frames a second it sampled.
    pub fps: f64,
    /// In time order.
    pub items: Vec<SceneItem>,
    /// The clip's hard cuts (file time, each the first frame of a new shot), in order. A lower bound: every one is a
    /// cut, but a cut between shots that look alike (a jump cut) or a shot under a quarter second is not found
    /// (`cuts`).
    pub cuts: Vec<Us>,
    /// What the calls cost, in US dollars.
    pub cost_usd: f64,
}

/// The frames a second Spark samples for a clip of `duration_us`.
pub fn fps_for(duration_us: Us) -> f64 {
    if duration_us <= SHORT_CLIP_US {
        FPS_SHORT
    } else {
        FPS_LONG
    }
}

/// What logging a clip of `duration_us` is expected to cost, before any call: each chunk's estimate from the logger.
pub fn estimate_usd(logger: &dyn SceneLogger, duration_us: Us) -> f64 {
    let fps = fps_for(duration_us);
    proxy::plan(duration_us, CHUNK_US, OVERLAP_US)
        .iter()
        .map(|chunk| logger.estimate_usd((chunk.end - chunk.start) as f64 / 1e6, fps))
        .sum()
}

/// The transcript's word edges in file time (sorted), for snapping: where words start and where they end. Pauses have
/// no edges of their own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordEdges {
    pub starts: Vec<Us>,
    pub ends: Vec<Us>,
}

/// `words` (in the clip's sound time) as edges in its file time: sound time + `audio_lead_us` (`ProxyInfo::audio_lead_us`).
/// The one place that turns word times into file time here.
pub fn word_edges(words: &[Word], audio_lead_us: Us) -> WordEdges {
    let spoken = words
        .iter()
        .filter(|w| w.kind != WordKind::Pause && !w.text.trim().is_empty());
    let mut edges = WordEdges::default();
    for word in spoken {
        edges.starts.push(word.start_us + audio_lead_us);
        edges.ends.push(word.end_us + audio_lead_us);
    }
    edges.starts.sort_unstable();
    edges.ends.sort_unstable();
    edges
}

/// Each chunk's moments (timed in the chunk) as one list of items in file time: the drops and joins of the module
/// notes, sorted by start. `chunks` are in order, each overlapping the next; the last one is the clip's end.
pub fn merge(chunks: Vec<(RangeUs, Vec<SceneMoment>)>) -> Vec<SceneItem> {
    let count = chunks.len();
    let mut items: Vec<SceneItem> = Vec::new();
    // The chunk before's items (indices into `items`), and where its items stopped being used.
    let mut before: Vec<usize> = Vec::new();
    let mut seam: Option<Us> = None;
    for (k, (range, moments)) in chunks.into_iter().enumerate() {
        let last = k + 1 == count;
        let mut kept = Vec::new();
        for moment in moments {
            let item = in_clip(moment, range);
            // The nearest item of its kind the chunk before kept, within `SAME_MOMENT_US`.
            let repeats = before
                .iter()
                .copied()
                .filter(|&j| {
                    items[j].kind == item.kind
                        && (items[j].start_us - item.start_us).abs() <= SAME_MOMENT_US
                })
                .min_by_key(|&j| (items[j].start_us - item.start_us).abs());
            if let Some(j) = repeats {
                // The chunk before logged it; this view may go on past where that chunk ended.
                items[j].end_us = items[j].end_us.max(item.end_us);
                continue;
            }
            if seam.is_some_and(|seam| item.start_us < seam) {
                // The chunk before's stretch.
                continue;
            }
            if !last && item.start_us >= range.end - UNRELIABLE_END_US {
                // Unreliable here; the next chunk logs it.
                continue;
            }
            kept.push(items.len());
            items.push(item);
        }
        before = kept;
        seam = Some(range.end - UNRELIABLE_END_US);
    }
    items.sort_by_key(|i| (i.start_us, i.end_us));
    items
}

/// A moment timed in its chunk as an item in file time, kept inside the chunk.
fn in_clip(moment: SceneMoment, range: RangeUs) -> SceneItem {
    let at = |ms: u64| {
        let us = Us::try_from(ms)
            .unwrap_or(Us::MAX / 2_000)
            .saturating_mul(1_000);
        range.start.saturating_add(us).clamp(range.start, range.end)
    };
    let start_us = at(moment.start_ms);
    SceneItem {
        start_us,
        end_us: at(moment.end_ms).max(start_us),
        kind: moment.kind,
        who: moment.who,
        text: moment.text,
        on_screen_text: moment.on_screen_text,
    }
}

/// Moves each item's edges to the clip's evidence. Each edge has up to three candidates, in this order:
/// - speech, reactions and actions (heard or done by someone): the nearest word edge within `WORD_SNAP_US` (a word's
///   start for the item's start, its end for the end), then the nearest cut within `CUT_SNAP_US`;
/// - shots, text, topics and the rest (seen): the cut first, then the word edge;
/// - last, the edge as written.
///
/// The start takes its first candidate that leaves the end one, then the end its first that keeps the item in order.
/// An item written with a span keeps one: its end never lands on or before its start, so never on the cut the start
/// took; it falls back to its next candidate (the edges as written always fit). An item written as an instant may sit
/// on one cut.
pub fn snap(items: &mut [SceneItem], cuts: &[Us], edges: &WordEdges) {
    for item in items {
        let words_first = matches!(
            item.kind,
            SceneKind::Speech | SceneKind::Reaction | SceneKind::Action
        );
        let candidates = |at: Us, words: &[Us]| {
            let word = nearest(words, at, WORD_SNAP_US);
            let cut = nearest(cuts, at, CUT_SNAP_US);
            let (first, second) = if words_first {
                (word, cut)
            } else {
                (cut, word)
            };
            [first, second, Some(at)].into_iter().flatten()
        };
        let span = item.end_us > item.start_us;
        let snapped = candidates(item.start_us, &edges.starts).find_map(|start| {
            candidates(item.end_us, &edges.ends)
                .find(|&end| if span { end > start } else { end >= start })
                .map(|end| (start, end))
        });
        // The edges as written always fit (`merge` keeps every end at or after its start).
        let (start, end) = snapped.unwrap_or((item.start_us, item.end_us.max(item.start_us)));
        item.start_us = start;
        item.end_us = end;
    }
}

/// The time in `sorted` nearest `at`, if one is within `within` (the earlier on a tie).
fn nearest(sorted: &[Us], at: Us, within: Us) -> Option<Us> {
    let i = sorted.partition_point(|&t| t < at);
    let before = i.checked_sub(1).map(|j| sorted[j]);
    let after = sorted.get(i).copied();
    [before, after]
        .into_iter()
        .flatten()
        .filter(|t| (t - at).abs() <= within)
        .min_by_key(|t| ((t - at).abs(), *t))
}

/// The scene log of the clip whose small copy is `proxy`, snapped to its cuts (`ProxyInfo::cuts`) and to `words` (its
/// transcript, in sound time; empty for none).
pub async fn log(
    logger: &dyn SceneLogger,
    proxy: &ProxyInfo,
    words: &[Word],
) -> Result<SceneLog, AnalysisError> {
    let fps = fps_for(proxy.duration_us);
    // Chunk by index, not by a borrowed chunk: a stream closure over borrowed items keeps the future from being `Send`.
    let answers = futures::stream::iter(0..proxy.chunks.len())
        .map(|k| logger.watch(&proxy.chunks[k].path, fps))
        .buffered(logger.max_in_flight().max(1))
        .map_err(AnalysisError::from)
        .try_collect::<Vec<_>>()
        .await?;
    let cuts = proxy.cuts.clone();
    let cost_usd = answers.iter().map(|a| a.cost_usd).sum();
    let chunks = proxy
        .chunks
        .iter()
        .zip(answers)
        .map(|(chunk, answer)| (chunk.range, answer.moments))
        .collect();
    let mut items = merge(chunks);
    snap(&mut items, &cuts, &word_edges(words, proxy.audio_lead_us));
    for item in &mut items {
        item.start_us = item.start_us.clamp(0, proxy.duration_us);
        item.end_us = item
            .end_us
            .clamp(item.start_us, proxy.duration_us.max(item.start_us));
    }
    items.sort_by_key(|i| (i.start_us, i.end_us));
    Ok(SceneLog {
        model: logger.model().to_owned(),
        fps,
        items,
        cuts,
        cost_usd,
    })
}
