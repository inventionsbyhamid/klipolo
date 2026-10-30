//! Where someone speaks on the timeline (`speech_ranges`): what a ducking sound layer (`ItemContent::Audio::duck`) is
//! lowered under. Worked out once per revision (the preview) or per job (export, which freezes them), never per block,
//! and shared by every ducking source of a plan (`Duck::speech`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use klipolo_model::project::Project;
use klipolo_model::query::main_track_duration;
use klipolo_model::speech::silence::{KEEP_US, pauses, threshold};
use klipolo_model::speech::{LEVEL_WINDOW_US, Levels, Speech, WordKind, run_spans, spoken};
use klipolo_model::time::{RangeUs, Us, merge_ranges};
use klipolo_model::timeline::ItemContent;

/// Words closer together than this are one stretch of speech: the music stays down between them.
pub const JOIN_US: Us = 300_000;

/// The timeline ranges where someone speaks on the main track, sorted and at least `JOIN_US` apart: its transcripts'
/// words (pause words are gaps), joined across gaps under `JOIN_US`. Where a file has no transcript, or past where its
/// transcript reaches while it is still being made, its levels stand in: what isn't a pause (`silence::pauses`, each
/// keeping a breath at both ends but none where the file starts or ends). A file whose levels never rise to speech
/// (`silence::threshold`) has nobody speaking in it, and a file with neither has nothing known: no ranges. A clip whose
/// sound is off (muted, or at no volume) speaks nowhere.
///
/// Cost: O(words + clips + pauses of each file with levels), each file's pauses found once.
pub fn speech_ranges(p: &Project, speech: &Speech) -> Arc<[RangeUs]> {
    let silent: HashSet<&str> = p
        .timeline
        .main
        .iter()
        .filter(|c| c.muted || c.volume <= 0.0)
        .map(|c| c.id.as_str())
        .collect();
    let said = spoken(p, speech);
    let mut out: Vec<RangeUs> = said
        .words
        .iter()
        .filter(|w| {
            w.kind != WordKind::Pause && !silent.contains(said.spans[w.span as usize].clip.as_str())
        })
        .map(|w| RangeUs {
            start: w.start_us,
            end: w.end_us,
        })
        .collect();
    let mut quiet: HashMap<&str, Option<Vec<RangeUs>>> = HashMap::new();
    for span in run_spans(p) {
        if silent.contains(span.clip.as_str()) {
            continue;
        }
        // Its transcript's words as far as they reach; its levels from there.
        let from = match speech.transcripts.get(&span.asset) {
            Some(t) if t.complete => continue,
            Some(t) => span.asset_in_us.max(t.covered_us),
            None => span.asset_in_us,
        };
        let Some((asset, levels)) = speech.levels.get_key_value(&span.asset) else {
            continue;
        };
        let to = span.asset_out_us.min(heard_us(levels));
        let Some(gaps) = quiet
            .entry(asset.as_str())
            .or_insert_with(|| file_pauses(levels))
        else {
            continue;
        };
        // `[from, to)` less its pauses, in the file's sound time, put on the timeline.
        let mut at = from;
        let first = gaps.partition_point(|r| r.end <= from);
        for gap in gaps[first..].iter().take_while(|r| r.start < to) {
            if gap.start > at {
                out.push(RangeUs {
                    start: span.at(at),
                    end: span.at(gap.start),
                });
            }
            at = at.max(gap.end);
        }
        if at < to {
            out.push(RangeUs {
                start: span.at(at),
                end: span.at(to),
            });
        }
    }
    joined(merge_ranges(&out, main_track_duration(&p.timeline.main))).into()
}

/// What `audio_plan` ducks under for `p`: `speech_ranges` when one of its sound layers ducks, else none, so a project
/// without ducking never walks its words for it.
pub fn ducking_speech(p: &Project, speech: &Speech) -> Arc<[RangeUs]> {
    if ducks(p) {
        speech_ranges(p, speech)
    } else {
        Arc::default()
    }
}

/// Whether any sound layer ducks (by more than 0 dB).
pub fn ducks(p: &Project) -> bool {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .any(|i| matches!(i.content, ItemContent::Audio { duck: Some(db), .. } if db < 0.0))
}

/// How far into its file `levels` reach.
fn heard_us(levels: &Levels) -> Us {
    levels.db.len() as Us * LEVEL_WINDOW_US
}

/// The file's pauses in its sound time (`silence::pauses`, at their shortest), sorted; one that reaches the file's
/// start or end keeps no breath there, since nothing is said beyond it. None when nobody speaks in the file.
fn file_pauses(levels: &Levels) -> Option<Vec<RangeUs>> {
    threshold(levels)?;
    let end = heard_us(levels);
    let mut gaps = pauses(levels, 0);
    if let Some(first) = gaps.first_mut().filter(|r| r.start == KEEP_US) {
        first.start = 0;
    }
    if let Some(last) = gaps.last_mut().filter(|r| r.end == end - KEEP_US) {
        last.end = end;
    }
    Some(gaps)
}

/// Sorted, merged ranges with the gaps under `JOIN_US` closed.
fn joined(ranges: Vec<RangeUs>) -> Vec<RangeUs> {
    let mut out: Vec<RangeUs> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start - last.end < JOIN_US => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}
