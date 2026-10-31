//! A proposal in Klipolo's words: its title on the card ("Cut 14 pauses and 9 filler words, 23.4 s in total") and a
//! short tag over each cut ("um", "pause"), read from what it removes on the project it applies to. A proposal that
//! removes no footage (a format dropped with what was placed for it) is titled by what it drops: never "0 s".

use klipolo_model::Project;
use klipolo_model::project::FormatKind;
use klipolo_model::risk::Removals;
use klipolo_model::speech::{Speech, WordKind, spoken};
use klipolo_model::time::{RangeUs, US_PER_SEC, Us};

/// The tag over a cut that only takes a pause away.
pub const PAUSE_TAG: &str = "pause";

/// What a proposal takes away, as the card shows it.
pub(crate) struct Described {
    pub title: String,
    /// A tag over each cut that is a filler word or a pause; other cuts have none.
    pub tags: Vec<(RangeUs, String)>,
}

/// Describes removals made on `base` (the project the proposal applies to, its words in `speech`). `tools` names the
/// tools whose calls made the proposal: a cut with no word in it is a pause only when `removeSilences` asked for one.
pub(crate) fn describe(
    base: &Project,
    speech: &Speech,
    removals: &Removals,
    removed_us: Us,
    tools: &[&str],
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
        format!(
            "Keep only the parts you chose, cutting {}",
            seconds(removed_us)
        )
    } else {
        cut_title(pauses, fillers, others, removed_us)
    };
    Described { title, tags }
}

/// "Cut 14 pauses and 9 filler words, 23.4 s in total"; "Cut 6.2 s" for one cut of nothing in particular.
fn cut_title(pauses: usize, fillers: usize, others: usize, removed_us: Us) -> String {
    if pauses == 0 && fillers == 0 && others == 1 {
        return format!("Cut {}", seconds(removed_us));
    }
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
    format!("Cut {}, {} in total", list(&parts), seconds(removed_us))
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

    fn titled(removals: Removals, removed_us: Us, tools: &[&str]) -> String {
        describe(
            &Project::new("P"),
            &Speech::default(),
            &removals,
            removed_us,
            tools,
        )
        .title
    }

    #[test]
    fn a_format_only_proposal_never_reads_zero_seconds() {
        let one = Removals {
            formats: vec![FormatKind::Square],
            ..Removals::default()
        };
        assert_eq!(titled(one, 0, &["setFormat"]), "Drop the square format");
        let two = Removals {
            formats: vec![FormatKind::Square, FormatKind::Portrait],
            ..Removals::default()
        };
        assert_eq!(
            titled(two, 0, &["setFormat"]),
            "Drop the square and portrait formats"
        );
        let layers = Removals {
            items: vec!["a".into()],
            ..Removals::default()
        };
        assert_eq!(
            titled(layers, 0, &["trimClip"]),
            "Remove or shorten 1 layer"
        );
    }

    #[test]
    fn cuts_are_counted_with_their_total() {
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
        assert_eq!(
            titled(ranges(14), 23_440_000, &["removeSilences"]),
            "Cut 14 pauses, 23.4 s in total"
        );
        assert_eq!(titled(ranges(1), 6_200_000, &["cutRange"]), "Cut 6.2 s");
        assert_eq!(
            titled(ranges(3), 3_000_000, &["cutRange"]),
            "Cut 3 parts, 3.0 s in total"
        );
        assert_eq!(
            titled(ranges(2), 41_000_000, &["keepOnly"]),
            "Keep only the parts you chose, cutting 41.0 s"
        );
    }
}
