//! Chat's follows (`klipolo_app::speaker`), made before a message's edits land so they land with them:
//!
//! - **`followSubject`** ("keep the camera on the woman in red", or the person talking in a short): its phrase is what
//!   SAM looks for (the speaker, the host or "me" is a person). It tracks the footage that plays once the message's
//!   edits are in, cuts waiting for approval included, so a short pays only for its own seconds. The clips that can
//!   follow then follow in every format whose Fill crops them, as part of the message's additions (one undo step), or
//!   of its proposal when it has one (they land when the person applies it). The reply says what it cost.
//! - **A vertical or square version** of camera footage made without asking to follow: footage tracked before follows
//!   at no charge; otherwise the reply offers it with its price.
//!
//! Where two or more people are on screen, `followSubject`'s focus (`keeps_for`) keeps who chat names over its parts
//! of the timeline, as each clip's source time, and whoever speaks elsewhere.
//!
//! Without a follower (tests that don't give one), `followSubject` is said to be coming, as before.

use std::collections::BTreeMap;

use klipolo_analysis::subject::PERSON;
use klipolo_analysis::subject::clips::followable_formats;
use klipolo_chat::{FollowFocus, Job, PlanOut};
use klipolo_model::apply_ops;
use klipolo_model::coded::ErrorParam;
use klipolo_model::fill_focus::{Keep, KeepRange, MAX_KEEP_RANGES, Who};
use klipolo_model::ids::Id;
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::FormatKind;
use klipolo_model::query::{clip_starts, piece_of};
use klipolo_model::time::{Us, unscaled};
use klipolo_model::{Project, Transaction};

use super::project::ProjectNow;
use crate::speaker::{Speaker, about};

/// Said when no format crops the footage: there is nowhere for a crop to follow.
pub const NO_CROP: &str =
    "Following works in a vertical or square version. Ask me to make one first.";
/// Said when the footage that plays has no one in it to follow.
pub const NO_ONE_THERE: &str = "I couldn't find anyone to follow in this video.";
/// Said when vertical or square versions exist but no clip there can follow a person.
pub const NO_FOOTAGE: &str =
    "None of these clips can follow a person. A screen recording follows the cursor instead.";
/// Said when a message names more than one thing to follow: the first is followed.
pub const ONE_AT_A_TIME: &str = "I followed the first one you named. Ask again for another.";

/// Words for the person talking: SAM finds them as a person (it would look for a loudspeaker for "speaker").
const SPEAKER_WORDS: [&str; 12] = [
    "speaker",
    "the speaker",
    "presenter",
    "the presenter",
    "host",
    "the host",
    "me",
    "the person",
    "person talking",
    "the person talking",
    "talking person",
    "whoever is talking",
];

/// The phrase SAM looks for, for `description`: a person for the speaker, and a noun phrase without its article.
pub fn phrase_for(description: &str) -> String {
    let plain = description.trim().to_lowercase();
    if plain.is_empty() || SPEAKER_WORDS.contains(&plain.as_str()) {
        return PERSON.to_owned();
    }
    let bare = ["the ", "a ", "an "]
        .iter()
        .find_map(|article| plain.strip_prefix(article))
        .map(str::trim)
        .filter(|rest| !rest.is_empty());
    bare.map_or(plain.clone(), str::to_owned)
}

/// What the message's follows came to: sentences for the reply, and a line for the conversation's note.
#[derive(Default)]
pub(super) struct Follows {
    pub said: Vec<String>,
    pub note: String,
}

/// Runs `out`'s follow jobs (and the offer for a new vertical or square version) on the project the message leaves
/// (see the module notes), putting their ops into its additions or its proposal. Without `speaker`, nothing changes:
/// the follow jobs stay for `jobs::start` to say they are coming.
pub(super) async fn follow_jobs(
    speaker: Option<&Speaker>,
    now: &ProjectNow,
    out: &mut PlanOut,
) -> Follows {
    let Some(speaker) = speaker else {
        return Follows::default();
    };
    let mut asked: Vec<(String, Vec<FollowFocus>)> = Vec::new();
    out.compiled.jobs.retain(|job| match job {
        Job::Follow { description, focus } => {
            asked.push((description.clone(), focus.clone()));
            false
        }
        _ => true,
    });
    // The project once the additions are in, and once the proposal's cuts are too: what plays. Additions that don't
    // apply won't land either: nothing is tracked or paid for then.
    let work = match &out.compiled.additions {
        Some(tx) => match apply_ops(&now.project, &tx.ops) {
            Ok(work) => work,
            Err(_) => return Follows::default(),
        },
        None => (*now.project).clone(),
    };
    let target =
        (out.compiled.proposal.as_ref()).map_or_else(|| work.clone(), |p| p.review.after.clone());
    let clip_ids: Vec<String> = target.timeline.main.iter().map(|c| c.id.clone()).collect();
    let formats = target.settings.formats.clone();
    let mut follows = Follows::default();
    if let Some((description, focus)) = asked.first() {
        let phrase = phrase_for(description);
        // Chat's times are the timeline's the message was sent on: each clip's source time stays put, and a piece a
        // cut in the same answer makes keeps what its clip keeps.
        let keeps = on_pieces(&target, &keeps_for(&now.project, focus));
        let estimate = speaker
            .estimate(&target, &clip_ids, &formats, &phrase)
            .await;
        if estimate.clips.is_empty() {
            let cropped = formats.iter().any(|f| *f != FormatKind::Landscape);
            follows
                .said
                .push((if cropped { NO_FOOTAGE } else { NO_CROP }).to_owned());
            follows.note = format!("Nothing to follow {phrase} in: no clip can follow.");
            return follows;
        }
        match (speaker.follow_keeping(&target, &clip_ids, &formats, &phrase, &keeps)).await {
            Err(why) => {
                follows.note = format!("Following {phrase} failed: {why}");
                follows.said.push(why);
            }
            Ok(followed) => {
                let ops = followed.ops(&target);
                let cost = cost_said(followed.paid_usd);
                if ops.is_empty() {
                    follows.said.push(format!("{NO_ONE_THERE} {cost}"));
                    follows.note = format!("No {phrase} was found to follow.");
                } else if !attach(out, &work, ops, &phrase) {
                    // Paid for all the same: the reply says so.
                    follows.said.push(format!(
                        "The follow couldn't be added to these changes. {cost}"
                    ));
                    follows.note = format!("Following {phrase} didn't fit the changes.");
                } else {
                    let when = if out.compiled.proposal.is_some() {
                        "once you apply this"
                    } else {
                        "now"
                    };
                    let kept = kept_in_frame(&target, &formats, &phrase);
                    follows.said.push(format!("{kept} {when}. {cost}"));
                    follows.note = format!(
                        "Followed {phrase} ({when}), for {}.",
                        dollars(followed.paid_usd)
                    );
                }
            }
        }
        if asked.len() > 1 {
            follows.said.push(ONE_AT_A_TIME.to_owned());
        }
        return follows;
    }
    // A new vertical or square version of footage that can follow: offered with its price, or done when free.
    let made: Vec<FormatKind> =
        (formats.iter().copied())
            .filter(|f| *f != FormatKind::Landscape && !now.project.settings.formats.contains(f))
            .chain(Some(work.settings.primary_format).filter(|f| {
                *f != FormatKind::Landscape && *f != now.project.settings.primary_format
            }))
            .collect();
    if made.is_empty() {
        return follows;
    }
    let estimate = speaker.estimate(&target, &clip_ids, &made, PERSON).await;
    if estimate.clips.is_empty() || estimate.refusal.is_some() {
        return follows;
    }
    if estimate.cached
        && let Ok(followed) = speaker.follow(&target, &clip_ids, &made, PERSON).await
    {
        let ops = followed.ops(&target);
        if !ops.is_empty() && attach(out, &work, ops, PERSON) {
            let (which, keep) = versions(&target, &made);
            follows.said.push(format!(
                "The {which} {keep} the speaker in frame too, at no charge: that footage was tracked before."
            ));
            follows.note = "Followed the speaker, tracked before.".to_owned();
            return follows;
        }
    }
    follows.said.push(format!(
        "To keep the speaker in frame, say follow the speaker. It costs {}.",
        about(estimate.usd)
    ));
    follows.note = format!("Offered to follow the speaker for {}.", about(estimate.usd));
    follows
}

/// Who each clip of `p` keeps for chat's `focus` (timeline ranges on `p`): the parts of it they cover, in its source
/// time, in order (a later range wins where two overlap). A clip one range covers whole keeps that range's choice; a
/// clip none touches isn't listed (whoever speaks).
pub fn keeps_for(p: &Project, focus: &[FollowFocus]) -> BTreeMap<Id, Keep> {
    let starts = clip_starts(&p.timeline.main);
    let mut out = BTreeMap::new();
    for (clip, start) in p.timeline.main.iter().zip(starts) {
        let end = start.saturating_add(clip.duration_us());
        let source = |t: Us| clip.source_in_us + unscaled(t - start, clip.speed);
        let mut ranges: Vec<KeepRange> = Vec::new();
        for f in focus {
            let (from, to) = (f.range.start.max(start), f.range.end.min(end));
            if from >= to {
                continue;
            }
            let (a, b) = (source(from), source(to).min(clip.source_out_us));
            if a >= b {
                continue;
            }
            // A later range wins: what it covers is taken from the ones before.
            let mut kept = Vec::new();
            for r in ranges.drain(..) {
                if r.end_us <= a || r.start_us >= b {
                    kept.push(r);
                    continue;
                }
                if r.start_us < a {
                    kept.push(KeepRange { end_us: a, ..r });
                }
                if r.end_us > b {
                    kept.push(KeepRange { start_us: b, ..r });
                }
            }
            kept.push(KeepRange {
                start_us: a,
                end_us: b,
                who: f.who,
            });
            kept.sort_by_key(|r| r.start_us);
            ranges = kept;
        }
        ranges.truncate(MAX_KEEP_RANGES);
        let keep = match ranges.as_slice() {
            [] => continue,
            [one] if one.start_us <= clip.source_in_us && one.end_us >= clip.source_out_us => {
                match one.who {
                    Who::Speaker => Keep::Speaker,
                    Who::Both => Keep::Both,
                    Who::Person { person } => Keep::Person { person },
                }
            }
            _ => Keep::Ranges { ranges },
        };
        out.insert(clip.id.clone(), keep);
    }
    out
}

/// `keeps` (by the clips of the project the message was sent on) for the clips of `target`: a piece that a cut or a
/// split in the same answer made keeps what its clip keeps (`query::piece_of`). Keep ranges are in source time, so
/// they hold on every piece.
pub fn on_pieces(target: &Project, keeps: &BTreeMap<Id, Keep>) -> BTreeMap<Id, Keep> {
    (target.timeline.main.iter())
        .filter_map(|c| {
            let from = piece_of(&c.id, |id| keeps.contains_key(id), |_| false)?;
            Some((c.id.clone(), keeps[from].clone()))
        })
        .collect()
}

/// `ops` into the message's proposal (judged again on `work`, the project its additions leave), else its additions
/// (started as chat's `followSubject` when it has none). False when the proposal no longer applies with them.
fn attach(out: &mut PlanOut, work: &Project, ops: Vec<Op>, phrase: &str) -> bool {
    if let Some(proposal) = &mut out.compiled.proposal {
        let mut tx = proposal.tx.clone();
        tx.ops.extend(ops);
        let Ok(review) = tx.review(work) else {
            return false;
        };
        proposal.tx = tx;
        proposal.review = review;
        return true;
    }
    match &mut out.compiled.additions {
        Some(tx) => tx.ops.extend(ops),
        None => {
            out.compiled.additions = Some(Transaction {
                label: "chat.followSubject".into(),
                params: Some([("description".to_owned(), ErrorParam::from(phrase))].into()),
                ops,
                origin: Some(Origin::Chat),
            })
        }
    }
    true
}

/// "The vertical version keeps the speaker in frame": what a follow of `phrase` in `formats` of `p` did.
pub fn kept_in_frame(p: &Project, formats: &[FormatKind], phrase: &str) -> String {
    let (which, keep) = versions(p, formats);
    format!("The {which} {keep} the {} in frame", shown(phrase))
}

/// "Tracking cost $0.02.", or that it cost nothing.
pub fn cost_said(paid_usd: f64) -> String {
    if paid_usd > 0.0 {
        format!("Tracking cost {}.", dollars(paid_usd))
    } else {
        "No charge: that footage was tracked before.".to_owned()
    }
}

/// "vertical version" with "keeps", or "vertical and square versions" with "keep": the formats of `formats` some clip
/// of `p` follows in, vertical first.
fn versions(p: &Project, formats: &[FormatKind]) -> (String, &'static str) {
    let order = [
        FormatKind::Portrait,
        FormatKind::Square,
        FormatKind::Landscape,
    ];
    let followed: Vec<&str> = (order.iter())
        .filter(|f| formats.contains(f))
        .filter(|f| (p.timeline.main.iter()).any(|c| !followable_formats(p, c, &[**f]).is_empty()))
        .map(|f| match f {
            FormatKind::Portrait => "vertical",
            FormatKind::Square => "square",
            FormatKind::Landscape => "16:9",
        })
        .collect();
    match followed.as_slice() {
        [] => ("video".to_owned(), "keeps"),
        [one] => (format!("{one} version"), "keeps"),
        [first, rest @ ..] => (
            format!("{first} and {} versions", rest.join(" and ")),
            "keep",
        ),
    }
}

/// The phrase as the reply says it: "speaker" for a person.
fn shown(phrase: &str) -> &str {
    if phrase == PERSON { "speaker" } else { phrase }
}

/// "$0.02", or "less than $0.01".
fn dollars(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "less than $0.01".into()
    } else {
        format!("${usd:.2}")
    }
}
