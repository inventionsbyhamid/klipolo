//! The additions, on the additions' working copy (the snapshot as the earlier additions of the answer leave it).
//! - `setFormat`: `SetSettings`, the format made the main one, joining the others or replacing them.
//! - `setCaptions`: `SetCaptions` with the other caption settings kept; no style keeps the current one.
//! - `fixCaptionWords`: one `FixWords` per place the words are shown, matched whole and ignoring case and the
//!   punctuation around them, in the file's sound time (the transcript's: neither file nor timeline time). The
//!   punctuation around the words shown stays around the fix ("Caden," reads "Kaiden,").
//! - `splitAt`: `Split`, the time snapped to the evidence when it is near some (never refused for standing on none),
//!   and mapped to where that moment plays now.
//! - `freezeFrame`: `FreezeFrame`, its time snapped and mapped as a split's, or the playhead's; 2 s unless said.
//!
//! A change that changes nothing (a format the video already has) adds no op.

use std::collections::BTreeSet;

use klipolo_model::Project;
use klipolo_model::captions::styles::BuiltinCaptionStyle;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::Op;
use klipolo_model::project::{FormatKind, Settings};
use klipolo_model::query::main_track_duration;
use klipolo_model::speech::{SpokenWord, WordKind, fix_text_ok, spoken};
use klipolo_model::time::Us;

use super::{Compiler, Edit, Refused, label, to_model, us};
use crate::context::{secs, within_video};
use crate::tools::FREEZE_DEFAULT_US;

/// The format's word, as the tool takes it.
pub(super) fn format_word(format: FormatKind) -> &'static str {
    match format {
        FormatKind::Landscape => "landscape",
        FormatKind::Portrait => "portrait",
        FormatKind::Square => "square",
    }
}

pub(super) fn set_format(work: &Project, format: FormatKind, keep_others: bool) -> Edit {
    let now = &work.settings;
    let formats = if keep_others {
        let mut all = now.formats.clone();
        if !all.contains(&format) {
            all.push(format);
        }
        all
    } else {
        vec![format]
    };
    let settings = Settings {
        fps: now.fps,
        formats,
        primary_format: format,
    };
    let others = if keep_others { "kept" } else { "dropped" };
    Edit {
        ops: if settings == *now {
            Vec::new()
        } else {
            vec![Op::SetSettings { settings }]
        },
        label: label(
            "setFormat",
            vec![
                ("format", ErrorParam::from(format_word(format))),
                ("others", ErrorParam::from(others)),
            ],
        ),
    }
}

pub(super) fn set_captions(work: &Project, on: bool, style: Option<BuiltinCaptionStyle>) -> Edit {
    let mut captions = work.captions.clone();
    captions.enabled = on;
    if let Some(style) = style {
        captions.style = Some(style.id());
    }
    let mut params = vec![("on", ErrorParam::from(if on { "on" } else { "off" }))];
    if let Some(style) = style {
        params.push(("style", ErrorParam::from(style.key())));
    }
    Edit {
        ops: if captions == work.captions {
            Vec::new()
        } else {
            vec![Op::SetCaptions { captions }]
        },
        label: label("setCaptions", params),
    }
}

pub(super) fn split_at(c: &Compiler, work: &Project, at_us: Us) -> Result<Edit, Refused> {
    let at = c.evidence.time(at_us);
    let Some(now) = c.map.time(work, at) else {
        return Err(to_model(format!(
            "splitAt: no clip plays at {} s.",
            secs(at_us)
        )));
    };
    Ok(Edit {
        ops: vec![Op::Split { at_us: now }],
        label: label("splitAt", vec![("atUs", us(at))]),
    })
}

/// `freezeFrame` at the time given (snapped to the evidence near it, as a split is) or the playhead, mapped to where
/// that moment plays now; at the main track's end, its last frame. Holds for `duration_us`, 2 s when unsaid.
pub(super) fn freeze_frame(
    c: &Compiler,
    work: &Project,
    at_us: Option<Us>,
    duration_us: Option<Us>,
) -> Result<Edit, Refused> {
    let at = match at_us {
        Some(t) => c.evidence.time(t),
        None => within_video(c.snapshot, &c.inputs.view).0,
    };
    let now = match c.map.time(work, at) {
        Some(now) => now,
        None if !work.timeline.main.is_empty()
            && at == main_track_duration(&c.snapshot.timeline.main) =>
        {
            main_track_duration(&work.timeline.main)
        }
        None => {
            return Err(to_model(format!(
                "freezeFrame: no clip plays at {} s.",
                secs(at)
            )));
        }
    };
    let duration = duration_us.unwrap_or(FREEZE_DEFAULT_US);
    Ok(Edit {
        ops: vec![Op::FreezeFrame {
            at_us: now,
            duration_us: duration,
        }],
        label: label(
            "freezeFrame",
            vec![("atUs", us(at)), ("holdDurationUs", us(duration))],
        ),
    })
}

/// A word as matched: lowercase, without the punctuation around it.
fn bare(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// What comes before a word's first letter or digit, and after its last.
fn around(text: &str) -> (&str, &str) {
    let first = text.find(char::is_alphanumeric).unwrap_or(text.len());
    let last = text.rfind(char::is_alphanumeric).map_or(first, |i| {
        i + text[i..].chars().next().map_or(0, char::len_utf8)
    });
    (&text[..first], &text[last.max(first)..])
}

pub(super) fn fix_caption_words(
    c: &Compiler,
    work: &Project,
    find: &str,
    replace: &str,
) -> Result<Edit, Refused> {
    let speech = &c.inputs.speech;
    let spoken = spoken(work, speech);
    // The words shown, in timeline order: no pause words, no later pieces of a fix.
    let shown: Vec<(&SpokenWord, &str)> = (spoken.shown(work, speech))
        .filter(|(w, text)| w.kind != WordKind::Pause && !text.is_empty())
        .collect();
    let wanted: Vec<String> = find.split_whitespace().map(bare).collect();
    let no_match = || {
        to_model(format!(
            "fixCaptionWords: no word in the captions reads {find:?}."
        ))
    };
    if wanted.is_empty() || wanted.iter().any(String::is_empty) {
        return Err(no_match());
    }
    // A word's sound time: its fix's, else its transcript word's.
    let sound = |w: &SpokenWord| -> Option<(Us, Us)> {
        match w.fix {
            Some(f) => work
                .word_fixes
                .get(f as usize)
                .map(|f| (f.start_us, f.end_us)),
            None => {
                let t = speech.transcripts.get(spoken.asset(w))?;
                t.words.get(w.word as usize).map(|x| (x.start_us, x.end_us))
            }
        }
    };
    let n = wanted.len();
    let mut seen = BTreeSet::new();
    let mut ops = Vec::new();
    for at in 0..shown.len().saturating_sub(n - 1) {
        let run = &shown[at..at + n];
        let one_span = run.iter().all(|(w, _)| w.span == run[0].0.span);
        let same = run
            .iter()
            .zip(&wanted)
            .all(|((_, text), want)| bare(text) == *want);
        if !one_span || !same {
            continue;
        }
        let ((first, first_text), (last, last_text)) = (run[0], run[n - 1]);
        let (Some((start_us, _)), Some((_, end_us))) = (sound(first), sound(last)) else {
            continue;
        };
        let asset = spoken.asset(first);
        if !seen.insert((asset.to_owned(), start_us, end_us)) {
            // The same words again where a clip is copied.
            continue;
        }
        let text = format!("{}{replace}{}", around(first_text).0, around(last_text).1);
        if !fix_text_ok(&text) {
            return Err(to_model(format!(
                "fixCaptionWords: {replace:?} can't be shown as a caption word."
            )));
        }
        ops.push(Op::FixWords {
            asset_id: asset.to_owned(),
            start_us,
            end_us,
            text: Some(text),
        });
    }
    if ops.is_empty() {
        return Err(no_match());
    }
    let count = ops.len();
    Ok(Edit {
        ops,
        label: label(
            "fixCaptionWords",
            vec![
                ("find", ErrorParam::from(find)),
                ("replace", ErrorParam::from(replace)),
                ("count", ErrorParam::from(count)),
            ],
        ),
    })
}
