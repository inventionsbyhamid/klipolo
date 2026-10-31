mod common;
use common::*;
use klipolo_model::history::{
    ByApp, Change, Document, HistoryLimits, Maker, Refused, Share, Whose, HISTORY_LIMIT,
};
use klipolo_model::timeline::ItemContent;
use klipolo_model::{Op, Project, Transaction};

fn rename(label: &str, name: &str) -> Transaction {
    Transaction {
        label: label.into(),
        ops: vec![Op::Rename { name: name.into() }],
        origin: None,
        ..Default::default()
    }
}

#[test]
fn apply_undo_redo() {
    let mut doc = Document::new(fixture()).unwrap();
    assert_eq!(doc.revision(), 0);
    assert_eq!(doc.apply(&rename("Rename", "One")).unwrap(), 1);
    assert_eq!(doc.apply(&rename("Rename again", "Two")).unwrap(), 2);
    let h = doc.history();
    assert!(h.can_undo && !h.can_redo);
    assert_eq!(h.undo_label.as_deref(), Some("Rename again"));
    assert_eq!(doc.undo(), Some(3));
    assert_eq!(doc.project().name, "One");
    assert_eq!(doc.history().redo_label.as_deref(), Some("Rename again"));
    assert_eq!(doc.redo(), Some(4));
    assert_eq!(doc.project().name, "Two");
    assert_eq!(doc.redo(), None);
}

#[test]
fn new_apply_clears_redo() {
    let mut doc = Document::new(fixture()).unwrap();
    doc.apply(&rename("a", "One")).unwrap();
    doc.undo();
    doc.apply(&rename("b", "Other")).unwrap();
    assert!(!doc.history().can_redo);
}

#[test]
fn failures_and_no_ops_record_nothing() {
    let mut doc = Document::new(fixture()).unwrap();
    assert!(doc.apply(&rename("bad", "")).is_err());
    assert_eq!(doc.revision(), 0);
    assert_eq!(doc.apply(&rename("same", "Fixture")).unwrap(), 0);
    assert_eq!(
        doc.apply(&Transaction {
            label: "empty".into(),
            ops: vec![],
            origin: None,
            ..Default::default()
        })
        .unwrap(),
        0
    );
    assert!(!doc.history().can_undo);
}

#[test]
fn history_is_bounded() {
    let mut doc = Document::new(fixture()).unwrap();
    for n in 0..(HISTORY_LIMIT + 5) {
        doc.apply(&rename("r", &format!("Name {n}"))).unwrap();
    }
    let mut undos = 0;
    while doc.undo().is_some() {
        undos += 1;
    }
    assert_eq!(undos, HISTORY_LIMIT);
    assert_eq!(doc.project().name, "Name 4");
}

#[test]
fn replace_resets_history() {
    let mut doc = Document::new(fixture()).unwrap();
    doc.apply(&rename("r", "One")).unwrap();
    let rev = doc.replace(fixture()).unwrap();
    assert_eq!(rev, 2);
    assert!(!doc.history().can_undo);
}

/// `tracks` overlay tracks of `per` still titles each, 0.1 s apiece side by side: a large project.
fn large(tracks: usize, per: usize) -> Project {
    let mut p = fixture();
    p.timeline.tracks = (0..tracks)
        .map(|t| {
            let items = (0..per)
                .map(|n| still_item(&format!("x{t}-{n}"), n as f64 * 0.1, 0.1))
                .collect();
            overlay_track(&format!("t{t}"), items)
        })
        .collect();
    p
}

/// `p` with every title's words changed: a batch whose reverse patch carries every layer whole.
fn every_title(p: &Project, words: &str) -> Project {
    let mut next = p.clone();
    for item in next
        .timeline
        .tracks
        .iter_mut()
        .flat_map(|t| t.items.iter_mut())
    {
        if let ItemContent::Text { text, .. } = &mut item.content {
            *text = words.into();
        }
    }
    next
}

/// The app's batch, committed as its own step (its level applied it), within `shares`.
fn by_app(doc: &mut Document, next: Project, shares: &[Share<'_>]) -> Result<Change, Refused> {
    let by = ByApp {
        app: "app",
        shares,
        after_point: false,
    };
    doc.commit_tx_keeping(&rename("app", "unused"), next, &by)
}

/// `doc`'s project renamed to `name`.
fn renamed(doc: &Document, name: &str) -> Project {
    let mut next = doc.project().clone();
    next.name = name.into();
    next
}

/// The app renames `doc`'s project to `name`, within `shares`.
fn app_rename(doc: &mut Document, name: &str, shares: &[Share<'_>]) -> Result<Change, Refused> {
    let next = renamed(doc, name);
    by_app(doc, next, shares)
}

/// MCP security: on a large project, an app's batch whose reverse patch is big would push the person's
/// own steps out by the byte budget. `commit_tx_keeping` refuses it, changes nothing and hands the project back, where
/// `commit_tx` would have dropped them.
#[test]
fn an_apps_batch_never_pushes_out_the_persons_steps_by_bytes() {
    let mut doc = Document::new(large(20, 200)).unwrap();
    for n in 0..5 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    // Room for the person's steps and 256 KiB more: far less than a patch carrying 4,000 layers.
    doc.set_limits(HistoryLimits {
        steps: HISTORY_LIMIT,
        bytes: doc.history_bytes() + (256 << 10),
    });
    let next = every_title(doc.project(), "From the app");
    let refused = by_app(&mut doc, next.clone(), &[]);
    let Err(Refused::PushesOut(back)) = refused else {
        panic!("refused: it would push the person's steps out")
    };
    assert_eq!(*back, next, "the project comes back to wait for the person");
    assert_eq!(
        (
            doc.revision(),
            doc.undo_steps(),
            doc.project().name.as_str()
        ),
        (5, 5, "Mine 4"),
        "nothing changed"
    );
    // Without the check, the same commit pushes every one of the person's steps out.
    doc.commit_tx(&rename("app", "unused"), next);
    assert_eq!(doc.undo_steps(), 1);
}

/// M-1 by the step limit: room is made from the app's own steps (folded, or its oldest at the oldest end); never the
/// person's, nor the person's redo steps (any commit drops them).
#[test]
fn an_apps_batch_makes_room_only_from_its_own_steps() {
    let mut doc = Document::new(fixture()).unwrap();
    doc.set_limits(HistoryLimits {
        steps: 3,
        bytes: usize::MAX,
    });
    let commit = |doc: &mut Document, name: &str| app_rename(doc, name, &[]);
    commit(&mut doc, "A1").unwrap();
    commit(&mut doc, "A2").unwrap();
    doc.apply(&rename("person", "Mine")).unwrap();
    assert!(commit(&mut doc, "A3").is_ok(), "the app's two steps fold");
    assert!(commit(&mut doc, "A4").is_ok(), "its oldest goes");
    assert_eq!(doc.undo_steps(), 3);
    assert_eq!(persons_steps(&doc), 1);
    // The app's step alone between the person's: making room would push the person's out.
    doc.apply(&rename("person", "Mine again")).unwrap();
    assert_eq!(persons_steps(&doc), 2);
    assert!(
        matches!(commit(&mut doc, "A5"), Err(Refused::PushesOut(_))),
        "the person's step would go"
    );
    assert_eq!(doc.project().name, "Mine again");
    // The person undoes their step: an app's commit would drop their redo step.
    doc.undo();
    assert!(
        commit(&mut doc, "A6").is_err(),
        "the person's redo step stays"
    );
}

/// How many undo steps the person made.
fn persons_steps(doc: &Document) -> usize {
    doc.undo_steps_where(|m| *m == Maker::Person)
}

/// How many undo steps the app made.
fn apps_steps(doc: &Document) -> usize {
    doc.undo_steps_where(Maker::is_app)
}

/// `p` with a new overlay track `id` of `n` still titles: an addition, whose undo patch holds only their ids while its
/// redo patch holds every title whole.
fn with_titles(p: &Project, id: &str, n: usize) -> Project {
    let mut next = p.clone();
    let items = (0..n)
        .map(|i| still_item(&format!("{id}-{i}"), i as f64 * 0.1, 0.1))
        .collect();
    next.timeline.tracks.push(overlay_track(id, items));
    next
}

/// A document with the person's five renames on a project of 20 tracks of 200 titles.
fn persons_five() -> Document {
    let mut doc = Document::new(large(20, 200)).unwrap();
    for n in 0..5 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    doc
}

/// The app's share: at most `steps` of its steps and `bytes`.
fn share(steps: usize, bytes: usize) -> [Share<'static>; 1] {
    [Share {
        of: Whose::App("app"),
        steps,
        bytes,
    }]
}

/// MCP security (undoing a big addition): an app's step is charged at the larger of its undo and redo
/// patches. An addition whose undo patch is small but whose redo patch is over the app's share is refused; without the
/// share it commits, and undoing it pushes the person's steps out.
#[test]
fn an_apps_addition_is_charged_at_its_redo_patch_too() {
    let mut doc = Document::new(fixture()).unwrap();
    for n in 0..5 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    doc.set_limits(HistoryLimits {
        steps: HISTORY_LIMIT,
        bytes: doc.history_bytes() + (16 << 10),
    });
    let share = share(usize::MAX, 8 << 10);
    let big = with_titles(doc.project(), "added", 200);
    let refused = by_app(&mut doc, big, &share);
    assert!(
        matches!(refused, Err(Refused::OverShare(0, _))),
        "{refused:?}"
    );
    assert_eq!((doc.revision(), persons_steps(&doc)), (5, 5));
    // A small addition fits its share; undoing it keeps every one of the person's steps.
    let small = with_titles(doc.project(), "few", 5);
    by_app(&mut doc, small, &share).unwrap();
    doc.undo();
    assert_eq!(persons_steps(&doc), 5);
    // Without the share, the limits judge it at its redo patch too: it waits, and the person's steps
    // stay.
    let big = with_titles(doc.project(), "added", 200);
    let refused = by_app(&mut doc, big, &[]);
    assert!(matches!(refused, Err(Refused::PushesOut(_))), "{refused:?}");
    assert_eq!(persons_steps(&doc), 5);
}

/// `p` with the words of every title on its track `t` changed: 200 layers no other track's edit touches, so folding
/// such edits together makes nothing smaller.
fn titles_on(p: &Project, t: usize, words: &str) -> Project {
    let mut next = p.clone();
    for item in &mut next.timeline.tracks[t].items {
        if let ItemContent::Text { text, .. } = &mut item.content {
            *text = words.into();
        }
    }
    next
}

/// Memory pressure: an app's share is a part of the history's budget under pressure, so when the limit drops to
/// it, the app's steps and the person's still fit. Without the share, an app fills the normal budget with edits that
/// folding can't shrink, and the drop pushes the person's steps out (its steps fold first, then the oldest end goes,
/// which is the person's).
#[test]
fn memory_pressure_never_pushes_out_the_persons_steps_for_an_app() {
    let (normal, pressure) = (2 << 20, 512 << 10);
    let fill = |shares: &[Share<'_>]| {
        let mut doc = persons_five();
        doc.set_limits(HistoryLimits {
            steps: HISTORY_LIMIT,
            bytes: normal,
        });
        for n in 0..20 {
            let next = titles_on(
                doc.project(),
                1 + n % 19,
                &format!("From the app, take {n}"),
            );
            if by_app(&mut doc, next, shares).is_err() {
                break;
            }
        }
        doc.set_limits(HistoryLimits {
            steps: HISTORY_LIMIT,
            bytes: pressure,
        });
        persons_steps(&doc)
    };
    assert_eq!(fill(&share(usize::MAX, pressure / 4)), 5);
    assert!(fill(&[]) < 5);
}

/// The person's later edits: with the app's steps within its share, the person's own later edits never push their
/// earlier steps out for room the app holds. Without the share, the app fills the budget (with edits folding can't
/// shrink) and the person's next edits do.
#[test]
fn the_persons_later_edits_never_push_out_their_steps_for_an_app() {
    let budget = 1 << 20;
    let edit = |shares: &[Share<'_>]| {
        let mut doc = persons_five();
        doc.set_limits(HistoryLimits {
            steps: HISTORY_LIMIT,
            bytes: budget,
        });
        for n in 0..20 {
            let next = titles_on(
                doc.project(),
                1 + n % 19,
                &format!("From the app, take {n}"),
            );
            if by_app(&mut doc, next, shares).is_err() {
                break;
            }
        }
        // The person's later edits: each rewrites the titles of the first track.
        for n in 0..5 {
            let next = titles_on(doc.project(), 0, &format!("Mine, take {n}"));
            doc.commit_tx(&rename("person", "unused"), next);
        }
        persons_steps(&doc)
    };
    assert_eq!(edit(&share(usize::MAX, budget / 4)), 10);
    assert!(edit(&[]) < 10);
}

/// MCP security: a mark says which steps came since, exactly, while the history reaches back to it: not
/// once its newest step was undone, nor (with none then) once an undo step was dropped.
#[test]
fn a_mark_lists_the_steps_since_only_while_the_history_reaches_back() {
    let mut doc = Document::new(fixture()).unwrap();
    let start = doc.mark();
    doc.apply(&rename("a", "One")).unwrap();
    let after_a = doc.mark();
    doc.apply(&rename("b", "Two")).unwrap();
    doc.apply(&rename("c", "Three")).unwrap();
    let labels = |m| {
        doc.since(m).map(|steps| {
            steps
                .iter()
                .map(|s| s.label.to_string())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        labels(start),
        Some(vec!["a".into(), "b".into(), "c".into()])
    );
    assert_eq!(labels(after_a), Some(vec!["b".into(), "c".into()]));
    // Undone past its step, the history no longer reaches back to `after_a`'s project by undoing.
    let mut doc2 = Document::new(fixture()).unwrap();
    doc2.apply(&rename("a", "One")).unwrap();
    let mark = doc2.mark();
    doc2.undo();
    doc2.apply(&rename("b", "Two")).unwrap();
    assert_eq!(doc2.since(mark), None);
    // With no step at the mark, any step dropped since means it can't be told.
    let mut doc3 = Document::new(fixture()).unwrap();
    doc3.set_limits(HistoryLimits {
        steps: 2,
        bytes: usize::MAX,
    });
    let empty = doc3.mark();
    doc3.apply(&rename("a", "One")).unwrap();
    doc3.apply(&rename("b", "Two")).unwrap();
    assert_eq!(doc3.since(empty).map(|s| s.len()), Some(2));
    doc3.apply(&rename("c", "Three")).unwrap();
    assert_eq!(doc3.since(empty), None);
}

/// MCP security (by step count): an app's steps count against its share by who made them, so it holds
/// at most `steps` of them. Steps split by the person's can't fold, so its next change past the share is refused.
#[test]
fn an_apps_steps_are_counted_against_its_share() {
    let mut doc = Document::new(fixture()).unwrap();
    let share = share(3, usize::MAX);
    for n in 0..3 {
        app_rename(&mut doc, &format!("App {n}"), &share).unwrap();
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    let refused = app_rename(&mut doc, "App 3", &share);
    assert!(matches!(refused, Err(Refused::OverShare(0, _))));
    assert_eq!((apps_steps(&doc), persons_steps(&doc)), (3, 3));
    // A step labelled as an app's but made by the person (a proposal they applied) is theirs: no share counts it.
    doc.commit_tx(&rename("app", "unused"), renamed(&doc, "Approved"));
    assert_eq!(apps_steps(&doc), 3);
}

/// The limits drop: when memory pressure lowers the step limit, an app's steps go first, folded into
/// the same app's step next to them so the history still chains; the person's own steps stay, and undoing everything
/// still gives the project as it was. Undoing one of the person's steps never takes back the app's.
#[test]
fn the_limits_fold_an_apps_steps_before_dropping_the_persons() {
    let start = fixture();
    let mut doc = Document::new(start.clone()).unwrap();
    for n in 0..10 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    for n in 0..15 {
        let next = with_titles(doc.project(), &format!("app{n}"), 3);
        by_app(&mut doc, next, &[]).unwrap();
    }
    for n in 10..20 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    let now = doc.project().clone();
    doc.set_limits(HistoryLimits {
        steps: 25,
        bytes: usize::MAX,
    });
    assert_eq!(persons_steps(&doc), 20, "all of the person's steps stay");
    assert_eq!((doc.undo_steps(), apps_steps(&doc)), (25, 5));
    assert_eq!(*doc.project(), now, "nothing changed now");
    // What is left still chains: undoing everything gives the project as it was, and redoing it all gives it back.
    // Each of the person's steps undoes only a rename.
    while let Some(top) = doc.undo_view().last().map(|s| s.maker.clone()) {
        let before = doc.project().clone();
        doc.undo();
        if top == Maker::Person {
            assert_eq!(before.timeline, doc.project().timeline);
        }
    }
    assert_eq!(*doc.project(), start);
    while doc.redo().is_some() {}
    assert_eq!(*doc.project(), now);
}

/// Under pressure the app's undone steps go before the person's, and the person's redo steps stay while an
/// app's step can go.
#[test]
fn the_limits_take_an_apps_redo_and_undo_steps_before_the_persons() {
    // The person's 45, then the app's 12, undone.
    let mut doc = Document::new(fixture()).unwrap();
    for n in 0..45 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    for n in 0..12 {
        app_rename(&mut doc, &format!("App {n}"), &[]).unwrap();
    }
    let after = doc.project().clone();
    for _ in 0..12 {
        doc.undo();
    }
    doc.set_limits(HistoryLimits::under_pressure());
    assert_eq!((persons_steps(&doc), doc.redo_steps()), (45, 5));
    while doc.redo().is_some() {}
    assert_eq!(*doc.project(), after);
    // The app's 12 in undo, the person's 39 undone.
    let mut doc = Document::new(fixture()).unwrap();
    for n in 0..12 {
        app_rename(&mut doc, &format!("App {n}"), &[]).unwrap();
    }
    for n in 0..39 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    for _ in 0..39 {
        doc.undo();
    }
    doc.set_limits(HistoryLimits::under_pressure());
    assert_eq!((apps_steps(&doc), doc.redo_steps()), (11, 39));
}

/// At its share, an app's own oldest steps that sit next to each other fold into
/// one ("<app>'s earlier changes") rather than make its change wait. 60 of the app's edits after 20 of the person's: all
/// commit within a share of 12, the person's 20 stay, and undoing the folded step gives the project as it was before the
/// app's edits; redo gives it all back. Steps of the app's split by the person's don't fold, so that change waits.
#[test]
fn an_apps_own_oldest_steps_fold_to_make_room_in_its_share() {
    let mut doc = Document::new(fixture()).unwrap();
    doc.set_folded_label(|app| {
        (
            "earlier".into(),
            Some([("client".into(), app.into())].into()),
        )
    });
    for n in 0..20 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    let before = doc.project().clone();
    let share = share(12, usize::MAX);
    // Small edits of three kinds: a rename, a title added, every title's words changed.
    let edit = |p: &Project, n: usize| -> Project {
        let mut next = p.clone();
        match n % 3 {
            0 => next.name = format!("App {n}"),
            1 => {
                if !next.timeline.tracks.iter().any(|t| t.id == "added") {
                    next.timeline
                        .tracks
                        .push(overlay_track("added", Vec::new()));
                }
                let track = next
                    .timeline
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == "added")
                    .unwrap();
                track
                    .items
                    .push(still_item(&format!("a{n}"), n as f64 * 0.1, 0.1));
            }
            _ => next = every_title(&next, &format!("take {n}")),
        }
        next
    };
    for n in 0..60 {
        let next = edit(doc.project(), n);
        by_app(&mut doc, next, &share).unwrap();
    }
    let after = doc.project().clone();
    assert_eq!(persons_steps(&doc), 20);
    assert_eq!(doc.undo_steps(), 32, "the person's 20 and the app's 12");
    let labels: Vec<String> = doc.undo_labels().map(|(l, _)| l.to_owned()).collect();
    assert_eq!(labels[20], "earlier", "the app's oldest steps are one");
    for _ in 0..12 {
        doc.undo();
    }
    assert_eq!(
        *doc.project(),
        before,
        "undoing it gives the project as it was before the app's edits"
    );
    for _ in 0..12 {
        doc.redo();
    }
    assert_eq!(*doc.project(), after);
    // The app's steps split by the person's can't fold: its next change past the share waits.
    let mut split = Document::new(fixture()).unwrap();
    for n in 0..12 {
        let next = edit(split.project(), n);
        by_app(&mut split, next, &share).unwrap();
        split
            .apply(&rename("person", &format!("Mine {n}")))
            .unwrap();
    }
    let next = edit(split.project(), 12);
    let refused = by_app(&mut split, next, &share);
    assert!(matches!(refused, Err(Refused::OverShare(0, _))));
}

/// An app's commit that folded its own steps to fit its share, then was refused (the person's
/// oldest step would go by bytes), leaves the history exactly as it was: no fold is kept.
#[test]
fn a_refused_commit_keeps_none_of_its_folds() {
    let mut doc = Document::new(large(1, 200)).unwrap();
    doc.apply(&rename("person", "Mine")).unwrap();
    let share = share(2, usize::MAX);
    for name in ["App 1", "App 2"] {
        app_rename(&mut doc, name, &share).unwrap();
    }
    doc.set_limits(HistoryLimits {
        steps: HISTORY_LIMIT,
        bytes: doc.history_bytes() + 1024,
    });
    let labels =
        |doc: &Document| -> Vec<String> { doc.undo_labels().map(|(l, _)| l.to_owned()).collect() };
    let before = (labels(&doc), doc.history_bytes(), doc.history());
    let next = every_title(doc.project(), "From the app");
    let refused = by_app(&mut doc, next, &share);
    assert!(matches!(refused, Err(Refused::PushesOut(_))), "{refused:?}");
    assert_eq!((labels(&doc), doc.history_bytes(), doc.history()), before);
    assert_eq!(doc.undo_steps(), 3);
}

/// A fold never crosses a seal (a Keep, or where a restore point was taken). The app's one step, sealed, then two
/// more past a share of two: the later two fold, never the sealed one with the next; and a mark at the sealed step still
/// lists exactly what came since.
#[test]
fn no_fold_crosses_a_seal() {
    let mut doc = Document::new(fixture()).unwrap();
    let (two, three) = (share(2, usize::MAX), share(3, usize::MAX));
    app_rename(&mut doc, "Kept", &two).unwrap();
    let kept = doc.project().clone();
    doc.seal();
    let mark = doc.mark();
    app_rename(&mut doc, "A1", &two).unwrap();
    let refused = app_rename(&mut doc, "A2", &two);
    assert!(
        matches!(refused, Err(Refused::OverShare(0, _))),
        "only the sealed pair could fold"
    );
    // A share of three: the later two fold to make room.
    app_rename(&mut doc, "A2", &three).unwrap();
    app_rename(&mut doc, "A3", &three).unwrap();
    assert_eq!(doc.undo_steps(), 3);
    assert_eq!(doc.since(mark).map(|s| s.len()), Some(2));
    doc.undo();
    doc.undo();
    assert_eq!(*doc.project(), kept, "undo reaches the state kept");
}

/// An app's commit is judged with each app step at its charge, the larger of its undo and redo
/// patches. So an addition whose redo patch wouldn't fit waits. The person then makes an edit and undoes it and what
/// came before: in normal conditions none of the person's steps drop, and undoing everything reaches the start.
#[test]
fn an_apps_commit_reserves_its_redo_patch_so_undoing_drops_none_of_the_persons() {
    let mut doc = Document::new(fixture()).unwrap();
    let start = doc.project().clone();
    for n in 0..5 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    // Room for the addition's undo patch (its ids), not for its redo patch (200 titles whole).
    doc.set_limits(HistoryLimits {
        steps: HISTORY_LIMIT,
        bytes: doc.history_bytes() + (16 << 10),
    });
    let big = with_titles(doc.project(), "added", 200);
    let committed = by_app(&mut doc, big, &[]);
    assert!(matches!(committed, Err(Refused::PushesOut(_))), "it waits");
    doc.apply(&rename("person", "Later")).unwrap();
    doc.undo();
    doc.undo();
    while doc.undo().is_some() {}
    assert_eq!(
        *doc.project(),
        start,
        "every one of the person's steps is still there"
    );
}

/// Over the byte limit alone, an app's adjacent steps fold before any of the person's steps go. The
/// app's six rewrites of the same 200 titles sit between the person's steps; two folds free the room, and every one of
/// the person's steps stays.
#[test]
fn over_the_byte_limit_an_apps_steps_fold_before_the_persons_go() {
    let mut doc = Document::new(large(1, 200)).unwrap();
    for n in 0..5 {
        doc.apply(&rename("person", &format!("Mine {n}"))).unwrap();
    }
    let before = doc.history_bytes();
    for n in 0..6 {
        let next = every_title(doc.project(), &format!("take {n}"));
        by_app(&mut doc, next, &[]).unwrap();
    }
    let each = (doc.history_bytes() - before) / 6;
    doc.apply(&rename("person", "Latest")).unwrap();
    let now = doc.project().clone();
    doc.set_limits(HistoryLimits {
        steps: HISTORY_LIMIT,
        bytes: doc.history_bytes() - 2 * each + each / 2,
    });
    assert_eq!((persons_steps(&doc), apps_steps(&doc)), (6, 4));
    assert_eq!(*doc.project(), now);
}

/// Who made each redo step is counted, as for undo: an app's commit would drop every redo step, so a host tells the
/// person's (or another app's) apart before it commits.
#[test]
fn redo_steps_are_counted_by_who_made_them() {
    let mut doc = Document::new(fixture()).unwrap();
    doc.apply(&rename("mine", "Mine")).unwrap();
    app_rename(&mut doc, "Theirs", &share(10, usize::MAX)).unwrap();
    assert_eq!(doc.redo_steps_where(|_| true), 0);
    doc.undo().unwrap();
    assert_eq!(doc.redo_steps_where(Maker::is_app), 1);
    assert_eq!(doc.redo_steps_where(|m| *m == Maker::Person), 0);
    doc.undo().unwrap();
    assert_eq!(doc.redo_steps(), 2);
    assert_eq!(doc.redo_steps_where(Maker::is_app), 1);
    assert_eq!(doc.redo_steps_where(|m| *m == Maker::Person), 1);
}
