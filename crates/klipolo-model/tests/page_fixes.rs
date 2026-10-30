//! Caption pages over the person's fixes and across formats (the cue tests, against Clean, whose
//! pages behave as the old cues did for short phrases).

use klipolo_model::apply_ops;
use klipolo_model::captions::looks::{Look, WordLook};
use klipolo_model::captions::{captions, captions_in, page_at, Pages};
use klipolo_model::ops::Op;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::speech::{spoken, Speech};

mod common;
use common::secs;
use common::speech::{pause, project_with_words, two_clips, w};

/// Clean's landscape pages.
fn clean(p: &Project, speech: &Speech) -> Pages {
    (*captions(p, speech).pages[&FormatKind::Landscape]).clone()
}

fn shown(pages: &Pages) -> Vec<String> {
    pages.pages.iter().map(|p| pages.shown_text(p)).collect()
}

#[test]
fn each_format_gets_its_own_pages_and_captions_off_have_none() {
    let (mut p, speech) = two_clips("caption.clean", vec![w("Hello", 1.0, 1.3)]);
    let c = captions(&p, &speech);
    assert_eq!(
        c.pages.keys().copied().collect::<Vec<_>>(),
        [FormatKind::Landscape, FormatKind::Portrait]
    );
    assert_eq!(c.style.as_str(), "klipolo.builtin/caption.clean");
    // An export's files may be other formats than the project lists: each still gets its pages.
    assert_eq!(
        captions_in(&p, &speech, &[FormatKind::Square])
            .pages
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [FormatKind::Square]
    );
    p.captions.enabled = false;
    assert!(captions(&p, &speech).pages.is_empty());
}

/// Korean writes spaces between its words: Hangul counts double but never joins.
#[test]
fn korean_words_keep_their_spaces_and_count_double() {
    let (p, speech) = two_clips(
        "caption.clean",
        vec![
            w("오늘은", 1.0, 1.3),
            w("예산에", 1.3, 1.6),
            w("대해", 1.6, 1.8),
            w("이야기합니다", 1.8, 2.4),
        ],
    );
    let c = captions(&p, &speech);
    // 14 syllables count 28, and three spaces: 31 columns, one line at landscape's 42; two lines at portrait's 28.
    assert_eq!(
        shown(&c.pages[&FormatKind::Landscape]),
        ["오늘은 예산에 대해 이야기합니다"]
    );
    let portrait = &c.pages[&FormatKind::Portrait];
    assert_eq!(shown(portrait), ["오늘은 예산에 대해 이야기합니다"]);
    assert!(
        portrait.words.iter().all(|w| !w.line_break),
        "the engine breaks overflow itself"
    );
}

/// A word fix that covers a pause reads from the start of its piece, which can be the pause. Its
/// caption starts when its first word is heard (and ends with its last), never in the silence before it.
#[test]
fn a_fixed_word_is_captioned_when_it_is_heard_not_from_the_pause_it_covers() {
    let (p, speech) = two_clips(
        "caption.clean",
        vec![
            w("I'll", 1.0, 1.3),
            w("show", 1.4, 1.7),
            pause(1.7, 2.6),
            w("you", 2.6, 2.9),
        ],
    );
    let fix = Op::FixWords {
        asset_id: "v".into(),
        start_us: secs(0.9),
        end_us: secs(3.0),
        text: Some("I will show you".into()),
    };
    // Cut "I'll show" (1.0–1.8 s): the piece left starts with the pause the fix covers.
    let p = apply_ops(&p, &[fix, cut(0.9, 1.8)]).unwrap();
    assert_eq!(
        spoken(&p, &speech).words[0].start_us,
        secs(0.9),
        "the speech map reads the fix from its pause"
    );
    let pages = clean(&p, &speech);
    let first = &pages.words[0];
    assert_eq!(
        (shown(&pages), pages.pages[0].start_us, first.start_us),
        (vec!["I will show you".to_owned()], secs(1.7), secs(1.7)),
        "from \"you\" (2.6 s, 1.7 s on the cut timeline)"
    );
    assert_eq!(
        (first.source_start_us, first.source_end_us),
        (secs(1.7), secs(2.9)),
        "the words it covers in its file, the pause included"
    );
}

/// The phrase's page in `p` and its Karaoke word: "I will show you." from `from` s until "you" ends at `you_end` s, up
/// until `end` s (lingered 200 ms, or held to Clean's 833 ms), coloured the whole time, and "Next" on a page of its own.
fn phrase_is_whole(p: &Project, speech: &Speech, from: f64, you_end: f64, end: f64) {
    let pages = clean(p, speech);
    assert_eq!(shown(&pages), ["I will show you.", "Next"]);
    let (phrase, word) = (&pages.pages[0], &pages.words[0]);
    assert_eq!(
        (phrase.start_us, word.start_us, word.end_us, phrase.end_us),
        (secs(from), secs(from), secs(you_end), secs(end)),
        "from its first heard word up to \"you\""
    );
    let saying_you = secs(you_end - 0.1);
    let on = page_at(&pages, saying_you).expect("captioned while \"you\" is said");
    let w = &pages.words[pages.pages[on].words.0 as usize];
    assert!(
        w.start_us <= saying_you && saying_you < w.end_us,
        "Karaoke colours it"
    );
}

fn fix_phrase(to: f64) -> Op {
    Op::FixWords {
        asset_id: "v".into(),
        start_us: secs(0.9),
        end_us: secs(to),
        text: Some("I will show you.".into()),
    }
}

fn cut(from: f64, to: f64) -> Op {
    Op::DeleteRanges {
        ranges: vec![klipolo_model::RangeUs {
            start: secs(from),
            end: secs(to),
        }],
    }
}

/// A fixed phrase whose words go on into a later piece of the same use right after it (a pause
/// removed inside it, or a split inside it) is captioned until its last word is said, and Karaoke colours it all that
/// time, whatever silence of the fix's own lies between the pieces' words (their spans meet). The later piece reads
/// nothing of its own, so the text still shows once.
#[test]
fn a_fixed_phrase_split_or_with_its_pause_removed_is_captioned_whole() {
    // A 1.3 s pause inside the fix (1.7-3.0 s), so 0.5 s or more of the fix's own silence lies between the pieces' words.
    let (p, speech) = two_clips(
        "caption.clean",
        vec![
            w("I'll", 1.0, 1.3),
            w("show", 1.4, 1.7),
            pause(1.7, 3.0),
            w("you", 3.0, 3.3),
            w("Next", 5.0, 5.3),
        ],
    );
    // Split in the middle of the pause, either side of its middle (2.35 s): the pause goes with "you" (2.3) or with
    // "show" (2.4), 0.6 s from the other piece's words either way.
    for at in [2.3, 2.4] {
        let split = apply_ops(&p, &[fix_phrase(3.5), Op::Split { at_us: secs(at) }]).unwrap();
        phrase_is_whole(&split, &speech, 1.0, 3.3, 3.5);
    }
    // Remove pauses with the loudness cut 0.1 s inside each word edge (1.8-2.9 s) and 150 ms kept at each end: it cuts
    // 1.95-2.75 s, leaving 0.5 s between "show" and "you" ("you" then plays at 2.2-2.5 s).
    let removed = apply_ops(&p, &[fix_phrase(3.5), cut(1.95, 2.75)]).unwrap();
    phrase_is_whole(&removed, &speech, 1.0, 2.5, 2.7);

    let (p, speech) = two_clips(
        "caption.clean",
        vec![
            w("I'll", 1.0, 1.3),
            w("show", 1.4, 1.7),
            pause(1.7, 2.6),
            w("you", 2.6, 2.9),
            w("Next", 4.0, 4.3),
        ],
    );
    // The pause's middle removed (as Remove pauses does): "you" then plays at 1.9-2.2 s.
    let removed = apply_ops(&p, &[fix_phrase(3.0), cut(1.8, 2.5)]).unwrap();
    phrase_is_whole(&removed, &speech, 1.0, 2.2, 2.4);
    // A split inside the pause: "you" stays at 2.6-2.9 s, in the second piece.
    let split = apply_ops(&p, &[fix_phrase(3.0), Op::Split { at_us: secs(2.0) }]).unwrap();
    phrase_is_whole(&split, &speech, 1.0, 2.9, 3.1);
    // The fix's first words cut, then a split just before "you": the piece that reads the text holds only the pause,
    // and the caption starts when "you" is heard (1.7 s), not at the pause (0.9 s). Said in 0.3 s, Clean holds it for
    // its 833 ms.
    let pause_first = apply_ops(
        &p,
        &[
            fix_phrase(3.0),
            cut(0.9, 1.8),
            Op::Split { at_us: secs(1.65) },
        ],
    )
    .unwrap();
    phrase_is_whole(&pause_first, &speech, 1.7, 2.0, 1.7 + 0.833);
}

/// `false` reads as unset for hiding, highlighting and line breaks (no style sets them per
/// word); only emphasis has a "never" (`the_persons_emphasis_wins_both_ways`).
#[test]
fn false_reads_as_unset_except_for_emphasis() {
    let (mut p, sp) = project_with_words(
        "caption.boxed",
        &common::speech::words_at(0.0, "so this is how I budget"),
    );
    let words = &sp.transcripts["v"].words;
    p.word_looks = vec![WordLook {
        asset_id: "v".into(),
        start_us: words[0].start_us,
        end_us: words.last().unwrap().end_us,
        look: Look {
            hidden: Some(false),
            line_break: Some(false),
            highlight: Some(false),
            ..Look::default()
        },
    }]
    .into();
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    assert_eq!(shown(pages), ["so this is how I budget"]);
    assert!(pages.words.iter().all(|w| !w.hidden && !w.line_break));
}
