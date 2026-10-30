use std::sync::Arc;

use klipolo_model::captions::looks::{Look, WordLook};
use klipolo_model::captions::sidecar::{
    cues_of, sidecar_cues, sidecar_pages, srt, vtt, CaptionFile, SideCue,
};
use klipolo_model::captions::{captions_in, Pages};
use klipolo_model::project::FormatKind;

mod common;
use common::speech::{project_with_words, words_at};

#[test]
fn srt_numbers_cues_with_comma_milliseconds_and_blank_lines() {
    let cues = [
        SideCue {
            start_us: 1_000_000,
            end_us: 2_500_000,
            lines: vec!["Hello there.".into()],
        },
        SideCue {
            start_us: 3_723_456_000,
            end_us: 3_725_000_000,
            lines: vec!["Two".into(), "lines".into()],
        },
    ];
    assert_eq!(srt(&cues), "1\n00:00:01,000 --> 00:00:02,500\nHello there.\n\n2\n01:02:03,456 --> 01:02:05,000\nTwo\nlines\n\n");
}

#[test]
fn vtt_starts_with_its_header_and_uses_dots() {
    let cues = [SideCue {
        start_us: 1_000_000,
        end_us: 2_500_000,
        lines: vec!["Hello".into()],
    }];
    assert_eq!(
        vtt(&cues),
        "WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nHello\n\n"
    );
}

#[test]
fn sidecars_use_clean_paging_whatever_is_burned_in_and_never_overlap() {
    let (p, sp) = project_with_words(
        "caption.loud",
        &words_at(
            0.0,
            "So the budget has three columns: date, category and amount. That's it.",
        ),
    );
    let cues = sidecar_cues(&p, &sp);
    assert_eq!(
        cues[0].lines.join(" "),
        "So the budget has three columns:",
        "Clean's case and punctuation, not Loud's"
    );
    assert!(cues.windows(2).all(|w| w[0].end_us <= w[1].start_us));
    assert!(cues
        .iter()
        .all(|c| c.lines.len() <= 2 && c.lines.iter().all(|l| l.chars().count() <= 42)));
}

#[test]
fn hidden_words_stay_out_of_the_sidecar() {
    let (mut p, sp) = project_with_words("caption.bold", &words_at(0.0, "um this is it"));
    let um = &sp.transcripts["v"].words[0];
    p.word_looks = vec![WordLook {
        asset_id: "v".into(),
        start_us: um.start_us,
        end_us: um.end_us,
        look: Look {
            hidden: Some(true),
            ..Look::default()
        },
    }]
    .into();
    assert_eq!(sidecar_cues(&p, &sp)[0].lines.join(" "), "this is it");
}

#[test]
fn the_persons_line_breaks_never_make_a_third_line() {
    let (mut p, sp) = project_with_words("caption.clean", &words_at(0.0, "one two three four"));
    let w = &sp.transcripts["v"].words;
    p.word_looks = [1, 2, 3]
        .iter()
        .map(|&i| WordLook {
            asset_id: "v".into(),
            start_us: w[i].start_us,
            end_us: w[i].end_us,
            look: Look {
                line_break: Some(true),
                ..Look::default()
            },
        })
        .collect::<Vec<_>>()
        .into();
    let cues = sidecar_cues(&p, &sp);
    assert!(
        cues.iter().all(|c| c.lines.len() <= 2),
        "{:?}",
        cues.iter().map(|c| &c.lines).collect::<Vec<_>>()
    );
}

#[test]
fn more_of_the_persons_line_breaks_than_lines_join_the_last_line() {
    // A page whose words each start a line (as a damaged or hand-made captions.json could have it): two lines, the
    // rest joining the second, never a third.
    let mut pages = Pages::one_page(
        "v",
        0,
        2_000_000,
        &[
            ("one", 0, 400_000),
            ("two", 500_000, 900_000),
            ("three", 1_000_000, 1_400_000),
            ("four", 1_500_000, 1_900_000),
        ],
    );
    for w in Arc::make_mut(&mut pages.words).iter_mut().skip(1) {
        w.line_break = true;
    }
    assert_eq!(cues_of(&pages)[0].lines, ["one", "two three four"]);
}

#[test]
fn a_sidecar_is_made_even_with_captions_off_in_the_video() {
    let (mut p, sp) = project_with_words("caption.bold", &words_at(0.0, "this is it"));
    p.captions.enabled = false;
    assert_eq!(sidecar_cues(&p, &sp).len(), 1);
}

#[test]
fn the_caption_file_reads_the_pages_an_export_freezes_when_clean_is_burned_in() {
    // One source of truth: with Clean burned in, the file is made from the very pages frozen in
    // the export's captions.json; with another style, from Clean's pages of the same project and speech.
    let sentence = words_at(
        0.0,
        "So the budget has three columns: date, category and amount.",
    );
    let (clean, sp) = project_with_words("caption.clean", &sentence);
    let frozen = captions_in(&clean, &sp, &[FormatKind::Landscape]);
    let pages = sidecar_pages(&clean, &sp, &frozen).expect("pages");
    assert!(Arc::ptr_eq(&pages, &frozen.pages[&FormatKind::Landscape]));
    let (bold, _) = project_with_words("caption.bold", &sentence);
    let burned = captions_in(&bold, &sp, &[FormatKind::Landscape]);
    let made = sidecar_pages(&bold, &sp, &burned).expect("pages");
    assert!(!Arc::ptr_eq(&made, &burned.pages[&FormatKind::Landscape]));
    assert_eq!(cues_of(&made), cues_of(&pages), "Clean's pages either way");
    assert_eq!(cues_of(&pages), sidecar_cues(&bold, &sp));
}

#[test]
fn a_transcripts_words_are_escaped_for_each_format_and_never_break_a_cue() {
    // Words are untrusted text: WebVTT escapes what would start a tag, an entity or a timing; neither format lets a
    // word end its cue with a blank line; control and direction-override characters go.
    let cues = [
        SideCue {
            start_us: 0,
            end_us: 1_000_000,
            lines: vec![
                "a --> b <i>x</i> & y".into(),
                "one\n\ntwo\r\n".into(),
                "\u{202E}evil\u{7}\u{2066}".into(),
            ],
        },
        // Nothing left to show: no cue (and SRT's numbers stay in order).
        SideCue {
            start_us: 1_000_000,
            end_us: 2_000_000,
            lines: vec!["\u{202D}\u{2069}\u{feff}".into(), " \t ".into()],
        },
        SideCue {
            start_us: 2_000_000,
            end_us: 3_000_000,
            lines: vec!["last".into()],
        },
    ];
    assert_eq!(
        vtt(&cues),
        "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\na --&gt; b &lt;i&gt;x&lt;/i&gt; &amp; y\none two\nevil\n\n\
         00:00:02.000 --> 00:00:03.000\nlast\n\n"
    );
    assert_eq!(
        srt(&cues),
        "1\n00:00:00,000 --> 00:00:01,000\na --> b <i>x</i> & y\none two\nevil\n\n\
         2\n00:00:02,000 --> 00:00:03,000\nlast\n\n"
    );
    assert_eq!(CaptionFile::Srt.write(&cues), srt(&cues));
    assert_eq!(CaptionFile::Vtt.write(&cues), vtt(&cues));
    assert_eq!(
        (CaptionFile::Srt.extension(), CaptionFile::Vtt.extension()),
        ("srt", "vtt")
    );
}
