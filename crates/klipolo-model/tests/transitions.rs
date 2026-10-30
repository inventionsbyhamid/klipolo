mod common;
use std::collections::BTreeMap;
use std::path::PathBuf;

use common::*;
use klipolo_model::contrib::builtin;
use klipolo_model::ops::{ClipPatch, Edge, Op, OpRisk, Origin, Transaction};
use klipolo_model::params::ParamValue;
use klipolo_model::query::clip_starts;
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::Transition;
use klipolo_model::transitions::{max_transition_us, windows, TransitionWindow};
use klipolo_model::{apply_ops, ModelError, Project};

fn dissolve(seconds: f64) -> Transition {
    dissolve_us(secs(seconds))
}

fn dissolve_us(duration_us: Us) -> Transition {
    Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us,
        params: Default::default(),
    }
}

/// Clip "b" plays broll 2–10 s, so 2 s of spare footage sit before its in point; "a" (the 60 s screen recording,
/// 0–20 s) has 40 s after its out point.
fn with_handles() -> Project {
    let mut p = fixture();
    p.timeline.main[1].source_in_us = secs(2.0);
    p
}

/// `with_handles`, then "c" playing broll 5–10 s after "b".
fn abc() -> Project {
    let mut p = with_handles();
    let mut c = p.timeline.main[1].clone();
    c.id = "c".into();
    c.source_in_us = secs(5.0);
    p.timeline.main.push(c);
    p
}

/// A batch from chat, so its risk decides whether it is proposed first.
fn from_chat(ops: Vec<Op>) -> Transaction {
    Transaction {
        label: "Edit".into(),
        ops,
        origin: Some(Origin::Chat),
        ..Default::default()
    }
}

fn move_clip(id: &str, index: usize) -> Op {
    Op::MoveClip {
        id: id.into(),
        index,
    }
}

/// Two transitions share clip "b" (broll 2–4 s, so 2 s long; "c" plays broll 5–10 s after it). The 1.000001 s
/// dissolve from "a" takes b's first 0.500001 s and the 2.000001 s dissolve into "c" its last 1 s: each limits the
/// other, and the odd lengths tell the halves before and after a cut apart.
fn shared() -> Project {
    let mut p = with_handles();
    p.timeline.main[1].source_out_us = secs(4.0);
    let mut c = p.timeline.main[1].clone();
    c.id = "c".into();
    c.source_in_us = secs(5.0);
    c.source_out_us = secs(10.0);
    p.timeline.main.push(c);
    let p = set(&p, "a", Some(dissolve_us(1_000_001))).unwrap();
    set(&p, "b", Some(dissolve_us(2_000_001))).unwrap()
}

fn set(p: &Project, clip: &str, t: Option<Transition>) -> Result<Project, ModelError> {
    apply_ops(
        p,
        &[Op::SetTransition {
            clip_id: clip.into(),
            transition: t,
        }],
    )
}

fn length_of(p: &Project, i: usize) -> Option<Us> {
    p.timeline.main[i]
        .transition_out
        .as_ref()
        .map(|t| t.duration_us)
}

#[test]
fn a_dissolve_joins_two_clips_and_replaces_their_fades() {
    let mut p = with_handles();
    p.timeline.main[0].fade_out_us = secs(0.5);
    p.timeline.main[1].fade_in_us = secs(0.5);
    let before = clip_starts(&p.timeline.main);
    let p = set(&p, "a", Some(dissolve(1.0))).unwrap();
    assert_eq!(p.timeline.main[0].transition_out, Some(dissolve(1.0)));
    assert_eq!(
        (
            p.timeline.main[0].fade_out_us,
            p.timeline.main[1].fade_in_us
        ),
        (0, 0),
        "a transition replaces the fades at its cut"
    );
    assert_eq!(
        clip_starts(&p.timeline.main),
        before,
        "clip positions never change"
    );
    assert!(p.requires.contains("transitions@1"));
    let p = set(&p, "a", None).unwrap();
    assert!(
        !p.requires.contains("transitions@1"),
        "with no transition left, older builds open the file normally again"
    );
}

#[test]
fn a_cut_without_spare_footage_refuses_plainly() {
    // "b" starts at the very beginning of its file: there is nothing to show before its in point.
    let error = set(&fixture(), "a", Some(dissolve(0.5))).unwrap_err();
    assert_eq!(
        error.to_string(),
        "There isn't enough extra footage at this cut for a transition."
    );
}

#[test]
fn the_last_clip_has_nothing_to_join() {
    let error = set(&with_handles(), "b", Some(dissolve(0.5))).unwrap_err();
    assert_eq!(
        error.to_string(),
        "A transition needs a clip after this one."
    );
}

#[test]
fn too_long_or_too_short_says_what_fits() {
    let p = with_handles();
    // 2 s of "b"'s spare footage before the cut: up to 4 s, plus the odd microsecond, which goes after the cut.
    assert_eq!(max_transition_us(&p, 0), secs(4.0) + 1);
    assert_eq!(
        set(&p, "a", Some(dissolve(4.5))).unwrap_err().to_string(),
        "This transition can be up to 4.0 s long here."
    );
    assert_eq!(
        set(&p, "a", Some(dissolve(0.05))).unwrap_err().to_string(),
        "A transition can't be shorter than 0.1 s."
    );
}

#[test]
fn a_trim_into_the_handle_shortens_the_transition() {
    let p = set(&with_handles(), "a", Some(dissolve(3.0))).unwrap();
    // "b" now starts 1 s into its file: 1 s of spare footage before the cut, so at most 2 s and 1 µs.
    let trim = |p: &Project, at: f64| {
        apply_ops(
            p,
            &[Op::TrimClip {
                id: "b".into(),
                edge: Edge::Start,
                source_us: secs(at),
            }],
        )
        .unwrap()
    };
    let p = trim(&p, 1.0);
    assert_eq!(length_of(&p, 0), Some(secs(2.0) + 1));
    // Back to the file's start nothing is left: the transition goes and the cut stays.
    let p = trim(&p, 0.0);
    assert_eq!(length_of(&p, 0), None);
}

#[test]
fn speed_scales_the_spare_footage() {
    let p = set(&with_handles(), "a", Some(dissolve(3.0))).unwrap();
    // At 2× the 2 s before "b"'s in point last 1 s on the timeline.
    let p = apply_ops(
        &p,
        &[Op::UpdateClip {
            id: "b".into(),
            patch: ClipPatch {
                speed: Some(2.0),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(length_of(&p, 0), Some(secs(2.0) + 1));
}

#[test]
fn a_split_keeps_the_transition_at_the_clips_end() {
    let p = set(&with_handles(), "a", Some(dissolve(1.0))).unwrap();
    let p = apply_ops(&p, &[Op::Split { at_us: secs(10.0) }]).unwrap();
    let shape: Vec<_> = p
        .timeline
        .main
        .iter()
        .map(|c| (c.id.as_str(), c.transition_out.is_some()))
        .collect();
    assert_eq!(shape, [("a", false), ("a-2", true), ("b", false)]);
}

#[test]
fn moving_a_clip_refits_its_transition_to_the_new_neighbour() {
    let p = set(&abc(), "a", Some(dissolve(3.0))).unwrap();
    // Last on the track, "a" has no clip to join: its transition goes.
    let last = apply_ops(
        &p,
        &[Op::MoveClip {
            id: "a".into(),
            index: 2,
        }],
    )
    .unwrap();
    assert!(last
        .timeline
        .main
        .iter()
        .all(|c| c.transition_out.is_none()));
    // With "b" moved away "a" leads into "c", whose 5 s of spare footage still fit 3 s.
    let refit = apply_ops(
        &p,
        &[Op::MoveClip {
            id: "b".into(),
            index: 2,
        }],
    )
    .unwrap();
    assert_eq!(length_of(&refit, 0), Some(secs(3.0)));
}

/// A new or longer transition never shortens a neighbour, and when an edit squeezes a shared clip the earlier
/// cut keeps its length.
#[test]
fn neighbours_share_a_clip_and_the_earlier_cut_wins() {
    let p = shared();
    // "b"'s last 1 s belongs to its own transition, its first 0.500001 s to the one from "a".
    let max: Vec<Us> = (0..3).map(|i| max_transition_us(&p, i)).collect();
    assert_eq!(max, [2_000_000, 2_999_999, 0]);
    // Too long is refused, never fitted by shortening the neighbour; the length named always fits.
    assert_eq!(
        set(&p, "a", Some(dissolve(2.1))).unwrap_err().to_string(),
        "This transition can be up to 2.0 s long here."
    );
    assert_eq!(
        set(&p, "b", Some(dissolve(3.0))).unwrap_err().to_string(),
        "This transition can be up to 2.9 s long here."
    );
    let longest = set(&p, "a", Some(dissolve(2.0))).unwrap();
    assert_eq!(
        (length_of(&longest, 0), length_of(&longest, 1)),
        (Some(2_000_000), Some(2_000_001))
    );
    // The windows meet inside "b" (20–22 s) without overlapping.
    let window = |clip, start_us, cut_us, end_us| TransitionWindow {
        clip,
        start_us,
        cut_us,
        end_us,
    };
    assert_eq!(
        windows(&p.timeline.main),
        [
            window(0, 19_500_000, 20_000_000, 20_500_001),
            window(1, 21_000_000, 22_000_000, 23_000_001),
        ]
    );
    // Trimmed to 1.5 s, "b" has 0.999999 s left after a's share: "a" stays whole and "b"'s transition shrinks.
    let trimmed = apply_ops(
        &p,
        &[Op::TrimClip {
            id: "b".into(),
            edge: Edge::End,
            source_us: secs(3.5),
        }],
    )
    .unwrap();
    assert_eq!(
        (length_of(&trimmed, 0), length_of(&trimmed, 1)),
        (Some(1_000_001), Some(1_999_999))
    );
}

/// One gesture is one batch, refitted once at its end: moving "a" and "b" together to the end passes through
/// "b, c, a", where "a" has nothing to join, but ends with "a" leading into "b" again.
#[test]
fn a_batch_keeps_a_transition_its_middle_would_drop() {
    let p = set(&abc(), "a", Some(dissolve(3.0))).unwrap();
    let moved = apply_ops(&p, &[move_clip("a", 2), move_clip("b", 2)]).unwrap();
    let order: Vec<_> = moved.timeline.main.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(order, ["c", "a", "b"]);
    assert_eq!(length_of(&moved, 1), Some(secs(3.0)));
}

/// Removing the end of a clip (filler, silence) keeps its transition on what is left, refitted, as trimming the end
/// does. When too little is left the transition goes, and the delete is proposed first anyway.
#[test]
fn deleting_a_clips_end_keeps_its_transition_while_it_fits() {
    let p = set(&with_handles(), "a", Some(dissolve(1.0))).unwrap();
    let cut_from = |at: f64| {
        from_chat(vec![Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: secs(at),
                end: secs(20.0),
            }],
        }])
    };
    let kept = apply_ops(&p, &cut_from(18.0).ops).unwrap();
    assert_eq!(length_of(&kept, 0), Some(secs(1.0)));
    // 40 ms of "a" left: too short to hold half of 0.1 s.
    let tx = cut_from(0.04);
    assert_eq!(length_of(&apply_ops(&p, &tx.ops).unwrap(), 0), None);
    assert_eq!(tx.risk_against(&p), OpRisk::Removal);
}

/// Principle 3: a batch that loses a transition is proposed first, even when it keeps every frame; carrying one
/// through a split or restyling it isn't a loss.
#[test]
fn losing_a_transition_needs_review() {
    let p = set(&abc(), "a", Some(dissolve(3.0))).unwrap();
    // Last on the track, "a" has nothing to join.
    assert_eq!(
        from_chat(vec![move_clip("a", 2)]).risk_against(&p),
        OpRisk::Removal
    );
    assert_eq!(
        from_chat(vec![Op::Split { at_us: secs(10.0) }]).risk_against(&p),
        OpRisk::Additive
    );
    let wipe = Transition {
        effect: builtin::TRANSITION_WIPE.into(),
        ..dissolve(2.0)
    };
    assert_eq!(
        from_chat(vec![Op::SetTransition {
            clip_id: "a".into(),
            transition: Some(wipe),
        }])
        .risk_against(&p),
        OpRisk::Additive
    );
}

/// Judged op by op (no project to compare against), taking a transition away is a removal, as it is against the
/// project; adding or changing one, or a reorder that keeps it, isn't.
#[test]
fn removing_a_transition_is_a_removal_op_by_op() {
    let p = set(&abc(), "a", Some(dissolve(3.0))).unwrap();
    let to = |t: Option<Transition>| {
        from_chat(vec![Op::SetTransition {
            clip_id: "a".into(),
            transition: t,
        }])
    };
    let remove = to(None);
    assert_eq!(remove.risk(), OpRisk::Removal);
    assert_eq!(remove.risk_against(&p), OpRisk::Removal);
    assert!(remove.needs_review(None));

    // "c" first: "a" still leads into "b".
    let reorder = from_chat(vec![move_clip("c", 0)]);
    assert_eq!(reorder.risk(), OpRisk::Additive);
    assert_eq!(reorder.risk_against(&p), OpRisk::Additive);

    let wipe = Transition {
        effect: builtin::TRANSITION_WIPE.into(),
        ..dissolve(2.0)
    };
    for (before, tx) in [(abc(), to(Some(dissolve(3.0)))), (p, to(Some(wipe)))] {
        assert_eq!(tx.risk(), OpRisk::Additive);
        assert_eq!(tx.risk_against(&before), OpRisk::Additive);
    }
}

#[test]
fn removing_the_next_clip_drops_the_transition() {
    let p = set(&with_handles(), "a", Some(dissolve(1.0))).unwrap();
    let p = apply_ops(&p, &[Op::RemoveClip { id: "b".into() }]).unwrap();
    assert_eq!(length_of(&p, 0), None);
}

#[test]
fn an_uninstalled_transition_is_kept_and_a_wrong_kind_refused() {
    let spin = Transition {
        effect: "com.acme.fx/transition.spin".into(),
        ..dissolve(1.0)
    };
    let p = set(&with_handles(), "a", Some(spin.clone())).unwrap();
    assert_eq!(p.timeline.main[0].transition_out, Some(spin));
    let blur = Transition {
        effect: builtin::EFFECT_BLUR.into(),
        ..dissolve(1.0)
    };
    assert_eq!(
        set(&with_handles(), "a", Some(blur))
            .unwrap_err()
            .to_string(),
        "Blur isn't a transition."
    );
}

#[test]
fn a_bad_setting_names_the_transition() {
    let wipe = Transition {
        effect: builtin::TRANSITION_WIPE.into(),
        params: BTreeMap::from([("glow".into(), ParamValue::Number(f64::NAN))]),
        ..dissolve(1.0)
    };
    assert_eq!(
        set(&with_handles(), "a", Some(wipe))
            .unwrap_err()
            .to_string(),
        "A transition has an invalid setting."
    );
}

#[test]
fn a_cut_writes_nothing_and_a_transition_writes_three_fields() {
    let mut c = with_handles().timeline.main[0].clone();
    assert!(serde_json::to_value(&c)
        .unwrap()
        .get("transitionOut")
        .is_none());
    c.transition_out = Some(dissolve(0.5));
    assert_eq!(
        serde_json::to_value(&c).unwrap()["transitionOut"],
        serde_json::json!({"effect": "klipolo.builtin/transition.dissolve", "durationUs": 500000, "params": {}})
    );
}

/// Each clip's transition length (0: none).
fn lengths(p: &Project) -> Vec<Us> {
    p.timeline
        .main
        .iter()
        .map(|c| c.transition_out.as_ref().map_or(0, |t| t.duration_us))
        .collect()
}

/// `op` done by hand, as the UI previews an edit before the model refits its transitions.
fn unrefitted(p: &Project, op: &Op) -> Project {
    let mut raw = p.clone();
    let main = &mut raw.timeline.main;
    match op {
        Op::MoveClip { id, index } => {
            let from = main.iter().position(|c| &c.id == id).unwrap();
            let clip = main.remove(from);
            main.insert((*index).min(main.len()), clip);
        }
        Op::TrimClip {
            id,
            edge: Edge::End,
            source_us,
        } => main.iter_mut().find(|c| &c.id == id).unwrap().source_out_us = *source_us,
        _ => unreachable!("not previewed"),
    }
    raw
}

/// The UI mirror (`src/lib/transitions.ts`) is checked against these cases: the longest transition at every cut,
/// every transition's window, and the lengths `normalize` refits an edit's transitions to (`fittedTransitionsUs`).
#[test]
fn the_ui_mirror_has_the_same_limits() {
    // "b" plays broll 2–8 s (2 s spare after it), "c" broll 5–10 s; a 2 s dissolve joins "b" and "c".
    let three = {
        let mut p = with_handles();
        p.timeline.main[1].source_out_us = secs(8.0);
        let mut c = p.timeline.main[1].clone();
        c.id = "c".into();
        c.source_in_us = secs(5.0);
        c.source_out_us = secs(10.0);
        p.timeline.main.push(c);
        set(&p, "b", Some(dissolve(2.0))).unwrap()
    };
    let slow = {
        let mut p = with_handles();
        p.timeline.main[1].speed = 0.5;
        p
    };
    // Edits as the UI previews them, before the model refits: "a" moved last loses its transition; with "b" moved
    // away "a" leads into "c" and keeps 3 s; "a"'s end trimmed out to its file's end leaves no footage after the cut;
    // "c" with only 1 s before its in point moved after "a" shortens a's 3 s to what fits (2 s and 1 µs).
    let moves = set(&abc(), "a", Some(dissolve(3.0))).unwrap();
    let short_head = {
        let mut p = abc();
        p.timeline.main[2].source_in_us = secs(1.0);
        set(&p, "a", Some(dissolve(3.0))).unwrap()
    };
    let trims = set(&with_handles(), "a", Some(dissolve(1.0))).unwrap();
    let trim_out = Op::TrimClip {
        id: "a".into(),
        edge: Edge::End,
        source_us: secs(60.0),
    };
    let edits = [
        (moves.clone(), move_clip("a", 2)),
        (moves, move_clip("b", 2)),
        (trims, trim_out),
        (short_head, move_clip("c", 1)),
    ]
    .map(|(p, op)| {
        let refitted = apply_ops(&p, std::slice::from_ref(&op)).unwrap();
        (unrefitted(&p, &op), lengths(&refitted))
    });
    assert_eq!(
        edits.each_ref().map(|(_, fitted)| fitted.clone()),
        [
            vec![0, 0, 0],
            vec![secs(3.0), 0, 0],
            vec![0, 0],
            vec![secs(2.0) + 1, 0, 0]
        ]
    );
    let cases: Vec<serde_json::Value> = [fixture(), with_handles(), three, slow, shared()]
        .into_iter()
        .map(|p| {
            let fitted = lengths(&p);
            (p, fitted)
        })
        .chain(edits)
        .map(|(p, fitted)| {
            let max: Vec<Us> = (0..p.timeline.main.len())
                .map(|i| max_transition_us(&p, i))
                .collect();
            let windows: Vec<_> = windows(&p.timeline.main)
                .into_iter()
                .map(|w| {
                    serde_json::json!({
                        "clipIndex": w.clip,
                        "startUs": w.start_us,
                        "cutUs": w.cut_us,
                        "endUs": w.end_us,
                    })
                })
                .collect();
            sorted_keys(
                serde_json::json!({ "project": p, "max": max, "windows": windows, "fitted": fitted }),
            )
        })
        .collect();
    assert_golden(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/transition-parity.json"),
        &cases,
    );
}
