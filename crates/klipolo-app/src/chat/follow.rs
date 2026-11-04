//! Chat's follows (`klipolo_app::speaker`), made before a message's edits land so they land with them:
//!
//! - **`followSubject`** ("keep the camera on the woman in red", or the person talking in a short): its phrase is what
//!   SAM looks for (the speaker, the host or "me" is a person). It tracks the footage that plays once the message's
//!   edits are in, cuts waiting for approval included, so a short pays only for its own seconds. The clips that can
//!   follow then follow in every format whose Fill crops them, as part of the message's additions (one undo step), or
//!   of its proposal when it has one (they land when the person applies it). The reply says what it cost. The stickers
//!   chat placed beside the speaker (`Compiled::beside`) are placed again beside the head where the path keeps them.
//!   Where the only people SAM found in a clip are a presenter's small corner inset (a face camera over a screen
//!   lesson), that clip shows its footage whole instead, Fit on a blurred background (`Followed::short_ops`), and the
//!   reply says so.
//! - **A vertical or square version** of camera footage made without asking to follow: footage tracked before follows
//!   at no charge; otherwise the reply offers it with its price.
//!
//! A person is followed as a person (`phrase_of`), whatever chat calls them ("the speaker", "woman astronaut", "the
//! person on the left"): SAM finds people, never one person by their looks or place, and footage tracked before (the
//! clip card's follow) is read from the cache at no charge. Where two or more people are on screen, who is kept is
//! chosen among the people the follow found, left to right: who `followSubject`'s focus names over its parts of the
//! timeline (`keeps_for`), as each clip's source time, and whoever speaks elsewhere; else the person its words place
//! ("on the left", `follow_as_asked`); else whoever speaks.
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
use klipolo_model::project::{BackgroundKind, FormatKind};
use klipolo_model::query::{clip_starts, piece_of};
use klipolo_model::time::{Us, unscaled};
use klipolo_model::{Project, Transaction};

use super::project::ProjectNow;
use crate::speaker::{Followed, Speaker, about};

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

/// Words that name a person in a follow's description (and their plurals with an s): a person is tracked for any of
/// them, and told apart from others only by place.
const PERSON_WORDS: [&str; 52] = [
    "person",
    "people",
    "man",
    "men",
    "woman",
    "women",
    "guy",
    "girl",
    "boy",
    "lady",
    "ladies",
    "gentleman",
    "gentlemen",
    "kid",
    "child",
    "children",
    "baby",
    "astronaut",
    "speaker",
    "presenter",
    "host",
    "guest",
    "interviewer",
    "interviewee",
    "student",
    "teacher",
    "narrator",
    "moderator",
    "panelist",
    "reporter",
    "anchor",
    "actor",
    "actress",
    "singer",
    "dancer",
    "player",
    "coach",
    "doctor",
    "nurse",
    "chef",
    "customer",
    "friend",
    "mom",
    "dad",
    "mother",
    "father",
    "me",
    "him",
    "her",
    "them",
    "someone",
    "somebody",
];

/// Words that only say which one, beside a place ("the one on the left").
const WHICH_WORDS: [&str; 16] = [
    "the", "a", "an", "on", "at", "in", "to", "of", "side", "one", "ones", "who", "is", "standing",
    "sitting", "frame",
];

/// Where a follow's words place the person to keep, among the people on screen from the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Left,
    Middle,
    Right,
    Everyone,
}

/// `description`'s words, lower case.
fn words(description: &str) -> Vec<String> {
    (description
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric()))
    .filter(|w| !w.is_empty())
    .map(str::to_owned)
    .collect()
}

/// The place a word names, if it names one.
fn place_word(word: &str) -> Option<Place> {
    match word {
        "left" | "leftmost" => Some(Place::Left),
        "middle" | "center" | "centre" => Some(Place::Middle),
        "right" | "rightmost" => Some(Place::Right),
        "both" | "everyone" | "everybody" | "all" | "together" => Some(Place::Everyone),
        _ => None,
    }
}

/// Whether `word` names a person, as itself or as a plural with an s.
fn person_word(word: &str) -> bool {
    PERSON_WORDS.contains(&word)
        || word
            .strip_suffix('s')
            .is_some_and(|one| PERSON_WORDS.contains(&one))
}

/// Whether `description` names a person: the speaker's words, a word for a person ("woman astronaut"), or only a place
/// ("the one on the left").
pub fn names_a_person(description: &str) -> bool {
    let plain = description.trim().to_lowercase();
    if SPEAKER_WORDS.contains(&plain.as_str()) {
        return true;
    }
    let words = words(description);
    words.iter().any(|w| person_word(w))
        || (words.iter().any(|w| place_word(w).is_some())
            && (words.iter()).all(|w| place_word(w).is_some() || WHICH_WORDS.contains(&w.as_str())))
}

/// Where `description` places the person to keep ("the person on the left"); None when it names no person, or no
/// place.
pub fn place_of(description: &str) -> Option<Place> {
    if !names_a_person(description) {
        return None;
    }
    words(description).iter().find_map(|w| place_word(w))
}

/// What chat's follow tracks: a person whenever its focus names who to keep (People's P1, P2 are the people a follow of
/// a person found) or its description names a person ([`names_a_person`]); else what the description names
/// ([`phrase_for`]).
pub fn phrase_of(description: &str, focus: &[FollowFocus]) -> String {
    if !focus.is_empty() || names_a_person(description) {
        return PERSON.to_owned();
    }
    phrase_for(description)
}

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
    /// A follow was asked for and nothing of it landed (nobody found, the tracking failed, no clip can follow): the
    /// model's words may claim it (`PlanOut::without_follow`).
    pub failed: bool,
    /// Every clip shows its footage whole for a presenter's corner inset: a sentence of the model's words that still
    /// says someone is kept in frame goes (`without_framing_claims`).
    pub differs: bool,
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
        let phrase = phrase_of(description, focus);
        let estimate = speaker
            .estimate(&target, &clip_ids, &formats, &phrase)
            .await;
        if estimate.clips.is_empty() {
            let cropped = formats.iter().any(|f| *f != FormatKind::Landscape);
            follows
                .said
                .push((if cropped { NO_FOOTAGE } else { NO_CROP }).to_owned());
            follows.note = format!("Nothing to follow {phrase} in: no clip can follow.");
            follows.failed = true;
            return follows;
        }
        let asked_for = Asked {
            description,
            focus,
            named_on: &now.project,
        };
        match follow_as_asked(speaker, &target, &clip_ids, &formats, &asked_for).await {
            Err(why) => {
                follows.note = format!("Following {phrase} failed: {why}");
                follows.said.push(why);
                follows.failed = true;
            }
            Ok(followed) => {
                // A clip whose only people are a presenter's corner inset shows its footage whole instead.
                let ops = followed.short_ops(&target);
                // Where the stickers Klipolo placed beside the speaker, and the hook, go once the path lands with these
                // changes.
                let followed_target = (!ops.is_empty())
                    .then(|| apply_ops(&target, &ops).ok())
                    .flatten();
                let cost = cost_said(followed.paid_usd);
                if ops.is_empty() {
                    follows.said.push(format!("{NO_ONE_THERE} {cost}"));
                    follows.note = format!("No {phrase} was found to follow.");
                    follows.failed = true;
                } else if !attach(out, &work, ops, &phrase) {
                    // Paid for all the same: the reply says so.
                    follows.said.push(format!(
                        "The follow couldn't be added to these changes. {cost}"
                    ));
                    follows.note = format!("Following {phrase} didn't fit the changes.");
                    follows.failed = true;
                } else {
                    // Attached: the stickers go beside the head where the path keeps the speaker.
                    if let (Some(followed_target), Some(tx)) =
                        (&followed_target, &mut out.compiled.additions)
                    {
                        klipolo_chat::place_beside_again(
                            followed_target,
                            &mut tx.ops,
                            &out.compiled.beside,
                        );
                        // A clip shown whole for its inset: the hook goes up above the picture.
                        klipolo_chat::place_hook_again(followed_target, &mut tx.ops);
                    }
                    let when = if out.compiled.proposal.is_some() {
                        "once you apply this"
                    } else {
                        "now"
                    };
                    let insets = (followed.paths.iter())
                        .filter(|(id, _)| followed.inset(id))
                        .count();
                    let whole = shown_whole(&target, &formats);
                    follows.differs = insets == followed.paths.len();
                    let said = if follows.differs {
                        format!(
                            "{TRACKER} found only a small corner inset, so {whole} {when}. {cost}"
                        )
                    } else {
                        let kept = kept_in_frame(&target, &formats, &who_kept(&phrase, &followed));
                        let also = if insets > 0 {
                            format!(" Where {TRACKER} found only a small corner inset, {whole}.")
                        } else {
                            String::new()
                        };
                        format!("{kept} {when}.{also} {cost}")
                    };
                    follows.said.push(said);
                    follows.note = format!(
                        "Followed {phrase} ({when}), for {}.",
                        dollars(followed.paid_usd)
                    );
                }
                // Focus chat named that isn't used: the model is told, so it doesn't say otherwise.
                for unused in unused_focus(&now.project, focus, &followed) {
                    follows.note.push(' ');
                    follows.note.push_str(&unused);
                }
                // Who speaks couldn't be told: the person hears why the crop follows the main person.
                if let Some(why) = &followed.note {
                    follows.said.push(why.clone());
                    follows.note.push(' ');
                    follows.note.push_str(why);
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

/// A follow as chat asked for it: what `description` names, who `focus` keeps (timeline ranges on `named_on`, the
/// project the message was sent on).
pub struct Asked<'a> {
    pub description: &'a str,
    pub focus: &'a [FollowFocus],
    pub named_on: &'a Project,
}

/// Follows what `asked` names (`phrase_of`) in `clip_ids` of `target` in `formats`. Where two or more people are on
/// screen: who its focus names, on the clips' source time, so a piece a cut in the same answer makes keeps what its
/// clip keeps (`on_pieces`); else, when its words place the person ("on the left"), that person among the people the
/// follow found, made from what it just tracked and matched at no further charge (`Speaker::keep`); else whoever
/// speaks. Why it couldn't run, in one sentence.
pub async fn follow_as_asked(
    speaker: &Speaker,
    target: &Project,
    clip_ids: &[Id],
    formats: &[FormatKind],
    asked: &Asked<'_>,
) -> Result<Followed, String> {
    let phrase = phrase_of(asked.description, asked.focus);
    let keeps = on_pieces(target, &keeps_for(asked.named_on, asked.focus));
    let mut followed = (speaker.follow_keeping(target, clip_ids, formats, &phrase, &keeps)).await?;
    let place = if asked.focus.is_empty() {
        place_of(asked.description)
    } else {
        None
    };
    let placed = place
        .map(|place| placed_keeps(&followed, place))
        .unwrap_or_default();
    if placed.is_empty() {
        return Ok(followed);
    }
    let ids: Vec<Id> = placed.keys().cloned().collect();
    // Only what was just tracked and matched: it never pays. Should the cache be gone meanwhile, whoever speaks stays.
    match speaker.keep(target, &ids, formats, &placed).await {
        Ok(kept) => {
            for (id, paths) in kept.paths {
                match followed.paths.iter_mut().find(|(own, _)| *own == id) {
                    Some(own) => own.1 = paths,
                    None => followed.paths.push((id, paths)),
                }
            }
            followed.kept.extend(kept.kept);
        }
        Err(why) => tracing::warn!(%why, "chat: who its words place couldn't be kept"),
    }
    Ok(followed)
}

/// Who `place` keeps in each clip `followed` chooses between two or more people in, by their place from the left: the
/// leftmost, the rightmost, the one in the middle of an odd number, or everyone.
fn placed_keeps(followed: &Followed, place: Place) -> BTreeMap<Id, Keep> {
    (followed.kept.iter())
        .filter_map(|(id, (_, people))| {
            let n = u8::try_from(people.len()).ok().filter(|n| *n >= 2)?;
            let keep = match place {
                Place::Left => Keep::Person { person: 1 },
                Place::Right => Keep::Person { person: n },
                Place::Middle if n % 2 == 1 => Keep::Person { person: n / 2 + 1 },
                Place::Middle => return None,
                Place::Everyone => Keep::Both,
            };
            Some((id.clone(), keep))
        })
        .collect()
}

/// Who a follow of `phrase` keeps in frame, as the reply names them: the thing followed; for a person, the speaker, or
/// who chat kept where two or more people are (`Followed::kept`): the person on the left or on the right, everyone,
/// or the people asked for when that is anyone else, or differs from clip to clip.
pub fn who_kept(phrase: &str, followed: &Followed) -> String {
    if phrase != PERSON {
        return phrase.to_owned();
    }
    let mut named: Vec<&str> = Vec::new();
    for (keep, people) in followed
        .kept
        .values()
        .filter(|(_, people)| people.len() >= 2)
    {
        let n = people.len();
        let whos: Vec<Who> = match keep {
            Keep::Speaker => continue,
            Keep::Person { person } => vec![Who::Person { person: *person }],
            Keep::Both => vec![Who::Both],
            Keep::Ranges { ranges } => ranges.iter().map(|r| r.who).collect(),
        };
        for who in whos {
            let name = match who {
                Who::Speaker => continue,
                Who::Person { person: 1 } => "person on the left",
                Who::Person { person } if usize::from(person) == n => "person on the right",
                Who::Person { .. } => "people you asked for",
                Who::Both => "everyone",
            };
            if !named.contains(&name) {
                named.push(name);
            }
        }
    }
    match named.as_slice() {
        [] => "speaker".to_owned(),
        [one] => (*one).to_owned(),
        _ => "people you asked for".to_owned(),
    }
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

/// What of chat's `focus` (timeline ranges on `p`) the follow couldn't use, one sentence each: a person no followed
/// clip has (whoever speaks is kept there), a range that touches no clip, and ranges past what a clip keeps.
fn unused_focus(p: &Project, focus: &[FollowFocus], followed: &Followed) -> Vec<String> {
    let mut out = Vec::new();
    let most = (followed.kept.values())
        .map(|(_, people)| people.len())
        .max()
        .unwrap_or(0);
    let mut missing: Vec<u8> = (focus.iter())
        .filter_map(|f| match f.who {
            Who::Person { person } if usize::from(person) > most => Some(person),
            _ => None,
        })
        .collect();
    missing.sort_unstable();
    missing.dedup();
    for person in missing {
        out.push(format!(
            "P{person} isn't on screen in these clips, so whoever speaks is kept there."
        ));
    }
    let end = p.timeline.main.iter().map(|c| c.duration_us()).sum::<Us>();
    if focus.iter().any(|f| f.range.start >= end) {
        out.push("A focus range outside the video was left out.".to_owned());
    }
    if focus.len() > MAX_KEEP_RANGES {
        out.push(format!(
            "Only the first {MAX_KEEP_RANGES} focus ranges of a clip are kept."
        ));
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

/// "The vertical version keeps the speaker in frame": what a follow in `formats` of `p` did, keeping `who`
/// ([`who_kept`]).
pub fn kept_in_frame(p: &Project, formats: &[FormatKind], who: &str) -> String {
    let (which, keep) = versions(p, formats);
    format!("The {which} {keep} the {who} in frame")
}

/// Words that say how a crop frames someone ("keeps her in frame", "the camera follows him"): a sentence of the model's
/// with one is left out where every clip shows its footage whole instead.
const FRAMING_WORDS: [&str; 7] = [
    "in frame",
    "in the frame",
    "in view",
    "framed",
    "centred",
    "centered",
    "follow",
];

/// `reply` without its sentences that say how someone is framed (`FRAMING_WORDS`); the others as they were.
pub(super) fn without_framing_claims(reply: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut start = 0;
    let bytes = reply.as_bytes();
    for (i, c) in reply.char_indices() {
        let ends = matches!(c, '.' | '!' | '?')
            && bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace());
        if ends {
            kept.push(&reply[start..=i]);
            start = i + 1;
        }
    }
    if start < reply.len() {
        kept.push(&reply[start..]);
    }
    kept.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| {
            let lower = s.to_lowercase();
            !FRAMING_WORDS.iter().any(|w| lower.contains(w))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tracker chat's follows use, as the reply names it.
const TRACKER: &str = "SAM 3.1";

/// What a follow did where the only person it found is a presenter's corner inset (`Followed::short_ops`): "the
/// vertical version shows the whole picture on a blurred background". The background is named only where it is
/// blurred.
fn shown_whole(p: &Project, formats: &[FormatKind]) -> String {
    let (which, keep) = versions(p, formats);
    let show = if keep == "keeps" { "shows" } else { "show" };
    let behind = match p.background.kind {
        BackgroundKind::None | BackgroundKind::Blur => " on a blurred background",
        _ => "",
    };
    format!("the {which} {show} the whole picture{behind}")
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

/// "$0.02", or "less than $0.01".
fn dollars(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "less than $0.01".into()
    } else {
        format!("${usd:.2}")
    }
}
