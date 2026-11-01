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
//! Without a follower (tests that don't give one), `followSubject` is said to be coming, as before.

use klipolo_analysis::subject::PERSON;
use klipolo_analysis::subject::clips::followable_formats;
use klipolo_chat::{Job, PlanOut};
use klipolo_model::apply_ops;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::FormatKind;
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
    let mut asked: Vec<String> = Vec::new();
    out.compiled.jobs.retain(|job| match job {
        Job::Follow { description } => {
            asked.push(description.clone());
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
    if let Some(description) = asked.first() {
        let phrase = phrase_for(description);
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
        match speaker.follow(&target, &clip_ids, &formats, &phrase).await {
            Err(why) => {
                follows.note = format!("Following {phrase} failed: {why}");
                follows.said.push(why);
            }
            Ok(followed) => {
                let ops = followed.ops(&target);
                let cost = if followed.paid_usd > 0.0 {
                    format!("Tracking cost {}.", dollars(followed.paid_usd))
                } else {
                    "No charge: that footage was tracked before.".to_owned()
                };
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
                    let (which, keep) = versions(&target, &formats);
                    follows.said.push(format!(
                        "The {which} {keep} the {} in frame {when}. {cost}",
                        shown(&phrase)
                    ));
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
