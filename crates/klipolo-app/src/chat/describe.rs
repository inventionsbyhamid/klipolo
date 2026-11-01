//! A proposal in Klipolo's words: its title on the card ("Cut 14 pauses and 9 filler words") and a short tag over each
//! cut ("um", "pause"), read from what it removes on the project it applies to. The title names what goes, never how
//! long: the card says the total once under it ("23.4 s in total"). A proposal that removes no footage (a format
//! dropped with what was placed for it, a transition taken away) is titled by what it drops. "Keep only these" keeps the parts the person chose;
//! another app's keepOnly keeps parts it suggests (the card names the app under its title).

use klipolo_model::Project;
use klipolo_model::project::FormatKind;
use klipolo_model::risk::Removals;
use klipolo_model::speech::{Speech, WordKind, spoken};
use klipolo_model::time::{RangeUs, US_PER_SEC, Us};

/// The tag over a cut that only takes a pause away.
pub const PAUSE_TAG: &str = "pause";
/// The title of a keepOnly the person asked for: they chose the parts.
pub const KEEP_CHOSEN: &str = "Keep only the parts you chose";
/// The title of another app's keepOnly: it chose the parts, and the card says who under the title.
pub const KEEP_SUGGESTED: &str = "Keep only the suggested parts";

/// What a proposal takes away, as the card shows it.
pub(crate) struct Described {
    pub title: String,
    /// A tag over each cut that is a filler word or a pause; other cuts have none.
    pub tags: Vec<(RangeUs, String)>,
}

/// Describes removals made on `base` (the project the proposal applies to, its words in `speech`). `tools` names the
/// tools whose calls made the proposal: a cut with no word in it is a pause only when `removeSilences` asked for one.
/// `from` is the app that made it, None for Klipolo's chat.
pub(crate) fn describe(
    base: &Project,
    speech: &Speech,
    removals: &Removals,
    tools: &[&str],
    from: Option<&str>,
) -> Described {
    let pauses_asked = tools.contains(&"removeSilences");
    let said = spoken(base, speech);
    let shown: Vec<(Us, Us, WordKind, String)> = (said.shown(base, speech))
        .filter(|(w, _)| w.kind != WordKind::Pause)
        .map(|(w, text)| (w.start_us, w.end_us, w.kind, text.to_owned()))
        .collect();
    let mut tags = Vec::new();
    let (mut pauses, mut fillers, mut others) = (0usize, 0usize, 0usize);
    for range in &removals.ranges {
        let inside: Vec<&(Us, Us, WordKind, String)> = (shown.iter())
            .filter(|(start, end, ..)| {
                let middle = start + (end - start) / 2;
                range.start <= middle && middle < range.end
            })
            .collect();
        let filler = inside
            .iter()
            .find(|(.., kind, _)| *kind == WordKind::Filler);
        let words = inside.iter().any(|(.., kind, _)| *kind == WordKind::Word);
        match filler {
            Some((.., text)) if !words => {
                fillers += 1;
                tags.push((*range, bare(text)));
            }
            None if !words && pauses_asked => {
                pauses += 1;
                tags.push((*range, PAUSE_TAG.to_owned()));
            }
            _ => others += 1,
        }
    }
    let title = if removals.ranges.is_empty() {
        dropped(removals)
    } else if tools == ["keepOnly"] {
        (if from.is_some() {
            KEEP_SUGGESTED
        } else {
            KEEP_CHOSEN
        })
        .to_owned()
    } else {
        cut_title(pauses, fillers, others)
    };
    Described { title, tags }
}

/// "Cut 14 pauses and 9 filler words"; "Cut 1 part" for one cut of nothing in particular.
fn cut_title(pauses: usize, fillers: usize, others: usize) -> String {
    let mut parts = Vec::new();
    if pauses > 0 {
        parts.push(count(pauses, "pause", "pauses"));
    }
    if fillers > 0 {
        parts.push(count(fillers, "filler word", "filler words"));
    }
    if others > 0 {
        parts.push(count(others, "part", "parts"));
    }
    format!("Cut {}", list(&parts))
}

/// A proposal with no footage to cut, titled by what it drops.
fn dropped(removals: &Removals) -> String {
    if !removals.formats.is_empty() {
        let names: Vec<String> = removals
            .formats
            .iter()
            .map(|f| format_name(*f).to_owned())
            .collect();
        let noun = if names.len() == 1 {
            "format"
        } else {
            "formats"
        };
        return format!("Drop the {} {noun}", list(&names));
    }
    if !removals.clips.is_empty() {
        return format!("Remove {}", count(removals.clips.len(), "clip", "clips"));
    }
    let layers = removals.items.len() + removals.shortened.len();
    if layers > 0 {
        return format!("Remove or shorten {}", count(layers, "layer", "layers"));
    }
    if !removals.transitions.is_empty() {
        let n = removals.transitions.len();
        return format!("Remove {}", count(n, "transition", "transitions"));
    }
    "Apply these changes".to_owned()
}

fn format_name(format: FormatKind) -> &'static str {
    match format {
        FormatKind::Landscape => "landscape",
        FormatKind::Portrait => "portrait",
        FormatKind::Square => "square",
    }
}

/// "1 pause", "14 pauses".
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "a", "a and b", "a, b and c".
fn list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A length in seconds with one decimal: "23.4 s".
pub(crate) fn seconds(us: Us) -> String {
    let tenths = (us.max(0) + US_PER_SEC / 20) / (US_PER_SEC / 10);
    format!("{}.{} s", tenths / 10, tenths % 10)
}

/// A filler as a tag: lowercase, without the punctuation around it ("Um," is "um").
fn bare(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use klipolo_model::Project;
    use klipolo_model::project::FormatKind;
    use klipolo_model::risk::Removals;
    use klipolo_model::speech::Speech;
    use klipolo_model::time::RangeUs;

    use super::*;

    fn titled(removals: Removals, tools: &[&str]) -> String {
        describe(
            &Project::new("P"),
            &Speech::default(),
            &removals,
            tools,
            None,
        )
        .title
    }

    #[test]
    fn a_format_only_proposal_never_reads_zero_seconds() {
        let one = Removals {
            formats: vec![FormatKind::Square],
            ..Removals::default()
        };
        assert_eq!(titled(one, &["setFormat"]), "Drop the square format");
        let two = Removals {
            formats: vec![FormatKind::Square, FormatKind::Portrait],
            ..Removals::default()
        };
        assert_eq!(
            titled(two, &["setFormat"]),
            "Drop the square and portrait formats"
        );
        let layers = Removals {
            items: vec!["a".into()],
            ..Removals::default()
        };
        assert_eq!(titled(layers, &["trimClip"]), "Remove or shorten 1 layer");
    }

    #[test]
    fn cuts_are_counted_and_the_total_is_left_to_the_card() {
        let ranges = |n: i64| Removals {
            ranges: (0..n)
                .map(|k| RangeUs {
                    start: k * 2_000_000,
                    end: k * 2_000_000 + 1_000_000,
                })
                .collect(),
            ..Removals::default()
        };
        // With no words heard, a cut is a pause only when pauses were asked for.
        assert_eq!(titled(ranges(14), &["removeSilences"]), "Cut 14 pauses");
        assert_eq!(titled(ranges(1), &["cutRange"]), "Cut 1 part");
        assert_eq!(titled(ranges(3), &["cutRange"]), "Cut 3 parts");
        assert_eq!(titled(ranges(2), &["keepOnly"]), KEEP_CHOSEN);
        // Another app's keepOnly: the parts are its suggestion, not the person's choice.
        let speech = Speech::default();
        let removals = ranges(2);
        let said = describe(
            &Project::new("P"),
            &speech,
            &removals,
            &["keepOnly"],
            Some("Example Client"),
        );
        assert_eq!(said.title, KEEP_SUGGESTED);
        assert_eq!(KEEP_CHOSEN, "Keep only the parts you chose");
        assert_eq!(KEEP_SUGGESTED, "Keep only the suggested parts");
    }

    #[test]
    fn pauses_and_fillers_are_named_together() {
        use klipolo_model::speech::{Transcript, Word};
        use klipolo_model::timeline::{Clip, ClipSource};
        // "So um we went" with a long pause after "went": the um's cut and the pause's.
        let mut p = Project::new("P");
        p.assets = vec![
            serde_json::from_value(serde_json::json!({
                "id": "talk", "kind": "video", "path": "/m/talk.mov", "name": "talk.mov",
                "durationUs": 10_000_000, "width": 1920, "height": 1080, "fps": 30.0, "hasAudio": true,
            }))
            .unwrap(),
        ];
        p.timeline.main = vec![Clip::new(
            ClipSource::Asset { id: "talk".into() },
            0,
            10_000_000,
        )];
        let word = |text: &str, start: Us, end: Us, kind: WordKind| Word {
            text: text.into(),
            start_us: start,
            end_us: end,
            confidence: 0.9,
            kind,
        };
        let mut speech = Speech::default();
        speech.transcripts.insert(
            "talk".into(),
            std::sync::Arc::new(Transcript {
                key: "k".into(),
                language: "en".into(),
                model: "m".into(),
                covered_us: 10_000_000,
                complete: true,
                words: vec![
                    word("So", 0, 300_000, WordKind::Word),
                    word("um", 400_000, 700_000, WordKind::Filler),
                    word("we", 800_000, 1_000_000, WordKind::Word),
                    word("went.", 1_100_000, 1_400_000, WordKind::Word),
                ],
            }),
        );
        let removals = Removals {
            ranges: vec![
                RangeUs {
                    start: 350_000,
                    end: 750_000,
                },
                RangeUs {
                    start: 2_000_000,
                    end: 5_000_000,
                },
            ],
            ..Removals::default()
        };
        let said = describe(
            &p,
            &speech,
            &removals,
            &["removeFillers", "removeSilences"],
            None,
        );
        assert_eq!(said.title, "Cut 1 pause and 1 filler word");
    }
}
