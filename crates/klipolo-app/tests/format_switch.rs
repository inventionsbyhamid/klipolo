//! The preview's format pills (and the 1, 2, 3 keys) switch the format as one `setSettings` op, built by the page
//! (`features/editor/formats.ts`): every format the project has stays, the one picked joins them and becomes the
//! primary. Switching keeps what was placed: the format left and the format picked look as they did, a hand override
//! stays with its format, a format without one re-derives from the new primary, the captions keep their place in each
//! format, and Undo puts the project back exactly as it was, in one step.

use std::collections::BTreeMap;

use klipolo_model::Transaction;
use klipolo_model::apply_ops;
use klipolo_model::coded::ErrorParam;
use klipolo_model::history::Document;
use klipolo_model::layout::{Anchor, BoxN, Layout, resolve_rect};
use klipolo_model::ops::Op;
use klipolo_model::project::{FormatKind, Project, Settings};
use klipolo_model::timeline::{Item, ItemContent, TextStyle, Track, TrackKind};

const ALL: [FormatKind; 3] = [
    FormatKind::Landscape,
    FormatKind::Portrait,
    FormatKind::Square,
];

/// A 60 s project in all three formats (16:9 primary) with captions on, placed per format, and a title moved by hand
/// in 9:16.
fn placed() -> Project {
    let (mut p, _) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
    p.settings.primary_format = FormatKind::Landscape;
    p.captions.place = BTreeMap::from([
        (FormatKind::Landscape, 0.82),
        (FormatKind::Portrait, 0.64),
        (FormatKind::Square, 0.78),
    ]);
    let mut layout = Layout::new(
        Anchor::TopLeft,
        BoxN {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.2,
        },
    );
    layout.overrides.insert(
        FormatKind::Portrait,
        BoxN {
            x: 0.05,
            y: 0.3,
            w: 0.9,
            h: 0.1,
        },
    );
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    titles.items = vec![Item {
        id: "title".into(),
        start_us: 0,
        duration_us: 2_000_000,
        attach: None,
        layout,
        content: ItemContent::Text {
            text: "A day at the studio".into(),
            style: TextStyle::default(),
        },
        effects: Vec::new(),
    }];
    p.timeline.tracks.push(titles);
    apply_ops(&p, &[]).expect("a checked project")
}

/// The op the page sends for a pill: the formats the project has with `format` joined (in 16:9, 9:16, 1:1 order),
/// `format` the primary.
fn switch_to(p: &Project, format: FormatKind) -> Transaction {
    let formats = ALL
        .into_iter()
        .filter(|f| *f == format || p.settings.formats.contains(f))
        .collect();
    Transaction {
        label: "format.switch.undo".into(),
        params: Some(BTreeMap::from([(
            "ratio".to_string(),
            ErrorParam::from("9:16"),
        )])),
        ops: vec![Op::SetSettings {
            settings: Settings {
                formats,
                primary_format: format,
                ..p.settings.clone()
            },
        }],
        origin: None,
    }
}

fn title(p: &Project) -> &Layout {
    &p.timeline.tracks.last().unwrap().items[0].layout
}

#[test]
fn switching_the_format_moves_nothing_and_undoes_in_one_step() {
    let before = placed();
    let mut doc = Document::new(before.clone()).unwrap();
    let tx = switch_to(&before, FormatKind::Portrait);
    let next = apply_ops(doc.project(), &tx.ops).unwrap();
    doc.commit_tx(&tx, next);
    let after = doc.project().clone();

    assert_eq!(after.settings.primary_format, FormatKind::Portrait);
    assert_eq!(after.settings.formats, ALL, "every format stays");
    for format in [FormatKind::Landscape, FormatKind::Portrait] {
        assert_eq!(
            resolve_rect(title(&after), FormatKind::Portrait, format),
            resolve_rect(title(&before), FormatKind::Landscape, format),
            "the title in {format:?} is where it was"
        );
    }
    let hand = BoxN {
        x: 0.05,
        y: 0.3,
        w: 0.9,
        h: 0.1,
    };
    assert_eq!(
        title(&after).rect,
        hand,
        "the hand override is 9:16's own, now its rect"
    );
    let derived = Layout::new(Anchor::TopLeft, hand);
    assert_eq!(
        resolve_rect(title(&after), FormatKind::Portrait, FormatKind::Square),
        resolve_rect(&derived, FormatKind::Portrait, FormatKind::Square),
        "1:1, with no override of its own, follows the new primary"
    );
    assert_eq!(
        after.captions.place, before.captions.place,
        "captions stay where they were in each format"
    );

    let history = doc.history();
    assert_eq!(
        history.undo_label.as_deref(),
        Some("format.switch.undo"),
        "one step, named"
    );
    assert_eq!(
        history.undo_params.as_ref().and_then(|p| p.get("ratio")),
        Some(&ErrorParam::from("9:16"))
    );
    doc.undo_change().expect("an undo");
    assert_eq!(*doc.project(), before, "Undo puts it back exactly");
}

#[test]
fn a_format_the_project_lacks_joins_it_when_picked() {
    let mut only = placed();
    only.settings.formats = vec![FormatKind::Landscape];
    let only = apply_ops(&only, &[]).unwrap();
    let next = apply_ops(&only, &switch_to(&only, FormatKind::Square).ops).unwrap();
    assert_eq!(
        next.settings.formats,
        [FormatKind::Landscape, FormatKind::Square]
    );
    assert_eq!(next.settings.primary_format, FormatKind::Square);
}
