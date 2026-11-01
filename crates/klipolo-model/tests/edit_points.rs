//! Roll, slip and slide: footage moves across a cut or through a clip while the video keeps its length. Each is held
//! to the media either side, to the footage its transitions blend, and to layers on locked tracks; layers attached to
//! the footage stay with it, and a recording's separated sound and camera move with their clip's edges.

mod common;
use common::*;
use klipolo_model::contrib::builtin;
use klipolo_model::ops::{op_risk, Op, OpRisk, TrackPatch};
use klipolo_model::query::{anchor_at, main_track_duration};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Item, ItemContent, Transition};
use klipolo_model::{apply_ops, Project};

/// "a" plays the recording 2 to 20 s, "b" broll 2 to 8 s (2 s spare each side), "c" the recording 30 to 40 s: 34 s.
fn abc() -> Project {
    let mut p = fixture();
    p.timeline.main[0].source_in_us = secs(2.0);
    p.timeline.main[1].source_in_us = secs(2.0);
    p.timeline.main[1].source_out_us = secs(8.0);
    let mut c = p.timeline.main[0].clone();
    c.id = "c".into();
    c.source_in_us = secs(30.0);
    c.source_out_us = secs(40.0);
    p.timeline.main.push(c);
    p
}

fn roll(clip: &str, by: f64) -> Op {
    Op::Roll {
        clip_id: clip.into(),
        by_us: secs(by),
    }
}

fn slip(clip: &str, by: f64) -> Op {
    Op::Slip {
        clip_id: clip.into(),
        by_us: secs(by),
    }
}

fn slide(clip: &str, by: f64) -> Op {
    Op::Slide {
        clip_id: clip.into(),
        by_us: secs(by),
    }
}

/// Each clip's in and out points, in seconds.
fn points(p: &Project) -> Vec<(f64, f64)> {
    let s = |us: Us| us as f64 / 1e6;
    (p.timeline.main.iter())
        .map(|c| (s(c.source_in_us), s(c.source_out_us)))
        .collect()
}

fn dissolve(seconds: f64) -> Transition {
    Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(seconds),
        params: Default::default(),
    }
}

fn item<'a>(p: &'a Project, id: &str) -> &'a Item {
    (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no layer {id}"))
}

/// `items` on one overlay track, each attached to the footage at its start.
fn with_titles(mut p: Project, items: Vec<Item>) -> Project {
    let mut track = overlay_track("titles", Vec::new());
    for mut i in items {
        i.attach = anchor_at(&p.timeline.main, i.start_us);
        track.items.push(i);
    }
    p.timeline.tracks.push(track);
    p
}

fn lock(p: &Project, track: &str) -> Project {
    apply_ops(
        p,
        &[Op::UpdateTrack {
            id: track.into(),
            patch: TrackPatch {
                locked: Some(true),
                ..Default::default()
            },
        }],
    )
    .unwrap()
}

fn separated(p: &Project, source: &str) -> Project {
    apply_ops(
        p,
        &[Op::DetachSource {
            clip_id: "a".into(),
            source_id: source.into(),
        }],
    )
    .unwrap()
}

/// (start, length, file time) of a sound or video layer, in seconds.
fn played(i: &Item) -> (f64, f64, f64) {
    let from = match &i.content {
        ItemContent::Audio { source_in_us, .. } | ItemContent::Video { source_in_us, .. } => {
            *source_in_us
        }
        _ => panic!("not a sound or video layer"),
    };
    let s = |us: Us| (us as f64 / 1e3).round() / 1e3;
    (s(i.start_us), s(i.duration_us), s(from))
}

#[test]
fn a_roll_moves_the_cut_and_nothing_else() {
    let p = abc();
    let later = apply_ops(&p, &[roll("a", 1.0)]).unwrap();
    assert_eq!(
        points(&later),
        [(2.0, 21.0), (3.0, 8.0), (30.0, 40.0)],
        "a's out and b's in move together"
    );
    assert_eq!(main_track_duration(&later.timeline.main), secs(34.0));
    let earlier = apply_ops(&p, &[roll("a", -1.5)]).unwrap();
    assert_eq!(points(&earlier), [(2.0, 18.5), (0.5, 8.0), (30.0, 40.0)]);
    assert_eq!(main_track_duration(&earlier.timeline.main), secs(34.0));
}

#[test]
fn a_roll_is_held_to_the_footage_each_side_and_needs_a_cut() {
    let p = abc();
    // b has 2 s before its in point; a can't get shorter than 10 ms, nor b.
    assert_eq!(
        apply_ops(&p, &[roll("a", -2.5)]).unwrap_err(),
        refused("There's no more footage that way.")
    );
    assert!(apply_ops(&p, &[roll("a", -2.0)]).is_ok());
    assert_eq!(
        apply_ops(&p, &[roll("a", 6.0)]).unwrap_err(),
        refused("A clip can't be trimmed shorter than 10 ms.")
    );
    // b's file ends 2 s after its out point.
    assert!(apply_ops(&p, &[roll("b", 2.0)]).is_ok());
    assert_eq!(
        apply_ops(&p, &[roll("b", 2.5)]).unwrap_err(),
        refused("There's no more footage that way.")
    );
    assert_eq!(
        apply_ops(&p, &[roll("c", 1.0)]).unwrap_err(),
        refused("There's no cut after this clip to roll.")
    );
}

#[test]
fn a_roll_keeps_the_footage_its_transition_blends() {
    let mut p = abc();
    p.timeline.main[0].transition_out = Some(dissolve(1.0));
    // The dissolve takes 0.5 s of b's footage before its in point: b may roll back to 0.5 s, not past it.
    let rolled = apply_ops(&p, &[roll("a", -1.5)]).unwrap();
    assert_eq!(
        rolled.timeline.main[0]
            .transition_out
            .as_ref()
            .unwrap()
            .duration_us,
        secs(1.0)
    );
    assert_eq!(
        apply_ops(&p, &[roll("a", -1.6)]).unwrap_err(),
        refused("A transition needs the footage this edit would use. Shorten or remove it first.")
    );
}

#[test]
fn layers_stay_with_their_footage_and_those_it_hides_keep_their_place() {
    // At 17 s: a's footage at 19 s. At 21 s: b's footage at 5 s, which stays. At 18.5 s: b's footage at 2.5 s, which a
    // roll of 1 s hides.
    let p = with_titles(
        abc(),
        vec![
            text_item("on-a", 17.0, 1.0),
            text_item("head-of-b", 18.5, 1.0),
            text_item("on-b", 21.0, 1.0),
        ],
    );
    let rolled = apply_ops(&p, &[roll("a", 1.0)]).unwrap();
    assert_eq!(item(&rolled, "on-a").start_us, secs(17.0));
    assert_eq!(item(&rolled, "on-b").start_us, secs(21.0));
    let hidden = item(&rolled, "head-of-b");
    assert_eq!(hidden.start_us, secs(18.5), "keeps its place");
    let anchor = hidden.attach.as_ref().unwrap();
    assert_eq!(
        (anchor.clip_id.as_str(), anchor.source_us),
        ("a", secs(20.5)),
        "attached to the footage now there"
    );
}

#[test]
fn a_separated_sound_follows_its_clips_end() {
    let p = separated(&abc(), "mic");
    // The mic's layer plays a's 18 s, from 2 s of its file.
    assert_eq!(played(item(&p, "a-mic")), (0.0, 18.0, 2.0));
    let later = apply_ops(&p, &[roll("a", 1.0)]).unwrap();
    assert_eq!(played(item(&later, "a-mic")), (0.0, 19.0, 2.0));
    let earlier = apply_ops(&p, &[roll("a", -1.0)]).unwrap();
    assert_eq!(played(item(&earlier, "a-mic")), (0.0, 17.0, 2.0));
}

#[test]
fn a_separated_sound_follows_its_clips_start() {
    let mut p = abc();
    p.timeline.main.swap(0, 1);
    // b is 0 to 6 s; a follows, playing the recording 2 to 20 s, and its mic's layer with it.
    let p = separated(&p, "mic");
    assert_eq!(played(item(&p, "a-mic")), (6.0, 18.0, 2.0));
    let later = apply_ops(&p, &[roll("b", 1.0)]).unwrap();
    assert_eq!(played(item(&later, "a-mic")), (7.0, 17.0, 3.0));
    let earlier = apply_ops(&p, &[roll("b", -1.0)]).unwrap();
    assert_eq!(played(item(&earlier, "a-mic")), (5.0, 19.0, 1.0));
}

#[test]
fn a_slip_moves_the_footage_through_the_clip_in_place() {
    let p = abc();
    let later = apply_ops(&p, &[slip("b", 1.0)]).unwrap();
    assert_eq!(points(&later)[1], (3.0, 9.0));
    let earlier = apply_ops(&p, &[slip("b", -2.0)]).unwrap();
    assert_eq!(points(&earlier)[1], (0.0, 6.0));
    assert_eq!(main_track_duration(&earlier.timeline.main), secs(34.0));
    for by in [2.5, -2.5] {
        assert_eq!(
            apply_ops(&p, &[slip("b", by)]).unwrap_err(),
            refused("There's no more footage that way.")
        );
    }
}

#[test]
fn a_slip_keeps_the_footage_its_transitions_blend() {
    let mut p = abc();
    // Into b: 0.5 s of b before its in point; out of b: 1 s of b after its out point.
    p.timeline.main[0].transition_out = Some(dissolve(1.0));
    p.timeline.main[1].transition_out = Some(dissolve(2.0));
    assert!(apply_ops(&p, &[slip("b", -1.5)]).is_ok());
    assert!(apply_ops(&p, &[slip("b", 1.0)]).is_ok());
    let refusal =
        refused("A transition needs the footage this edit would use. Shorten or remove it first.");
    assert_eq!(apply_ops(&p, &[slip("b", -1.6)]).unwrap_err(), refusal);
    assert_eq!(apply_ops(&p, &[slip("b", 1.1)]).unwrap_err(), refusal);
}

#[test]
fn a_slip_carries_the_layers_on_its_footage() {
    // b starts at 18 s with its footage at 2 s: "moment" is on its 5 s, "start" on its 2 s.
    let p = with_titles(
        abc(),
        vec![
            text_item("start", 18.0, 1.0),
            text_item("moment", 21.0, 1.0),
        ],
    );
    let slipped = apply_ops(&p, &[slip("b", 1.0)]).unwrap();
    assert_eq!(
        item(&slipped, "moment").start_us,
        secs(20.0),
        "its moment now plays 1 s earlier"
    );
    let start = item(&slipped, "start");
    assert_eq!(start.start_us, secs(18.0), "its moment is hidden: it stays");
    assert_eq!(start.attach.as_ref().unwrap().source_us, secs(3.0));
}

#[test]
fn a_separated_camera_slips_with_its_clip() {
    let p = separated(&abc(), "camera");
    // The camera's file starts 0.2 s into the take: at a's 2 s it plays 1.8 s of its file.
    assert_eq!(played(item(&p, "a-camera")), (0.0, 18.0, 1.8));
    let slipped = apply_ops(&p, &[slip("a", 1.0)]).unwrap();
    assert_eq!(points(&slipped)[0], (3.0, 21.0));
    assert_eq!(played(item(&slipped, "a-camera")), (0.0, 18.0, 2.8));
    // Its file holds the camera 1.8 s back: the slip is held to that too.
    assert_eq!(
        apply_ops(&p, &[slip("a", -1.9)]).unwrap_err(),
        refused("There's no more footage that way.")
    );
}

#[test]
fn a_layer_on_a_locked_track_holds_a_slip() {
    let p = lock(
        &with_titles(abc(), vec![text_item("moment", 21.0, 1.0)]),
        "titles",
    );
    assert_eq!(
        apply_ops(&p, &[slip("b", 1.0)]).unwrap_err(),
        refused("A layer on a locked track would move. Unlock its track first.")
    );
    // A roll leaves it where it is.
    assert!(apply_ops(&p, &[roll("a", 0.5)]).is_ok());
}

#[test]
fn a_slide_moves_a_clip_between_its_neighbours() {
    let p = with_titles(abc(), vec![text_item("moment", 21.0, 1.0)]);
    let later = apply_ops(&p, &[slide("b", 1.0)]).unwrap();
    assert_eq!(points(&later), [(2.0, 21.0), (2.0, 8.0), (31.0, 40.0)]);
    assert_eq!(main_track_duration(&later.timeline.main), secs(34.0));
    assert_eq!(item(&later, "moment").start_us, secs(22.0), "moves with b");
    let earlier = apply_ops(&p, &[slide("b", -3.0)]).unwrap();
    assert_eq!(points(&earlier), [(2.0, 17.0), (2.0, 8.0), (27.0, 40.0)]);
}

#[test]
fn a_slide_needs_a_clip_each_side_with_room() {
    let p = abc();
    let alone = refused("Only a clip with a clip on each side can slide.");
    assert_eq!(apply_ops(&p, &[slide("a", 1.0)]).unwrap_err(), alone);
    assert_eq!(apply_ops(&p, &[slide("c", -1.0)]).unwrap_err(), alone);
    assert_eq!(
        apply_ops(&p, &[slide("b", 9.995)]).unwrap_err(),
        refused("A clip can't be trimmed shorter than 10 ms.")
    );
    // c has 30 s before its in point; a has 40 s after its out point.
    assert!(apply_ops(&p, &[slide("b", 9.0)]).is_ok());
    assert!(apply_ops(&p, &[slide("b", -17.0)]).is_ok());
}

#[test]
fn a_freeze_frame_has_no_footage_to_slip_and_rolls_past_its_file() {
    // 8 s of b's frame at 4 s of its file, which has only 6 s after it: a freeze frame's range is only its length.
    let freeze = Op::FreezeFrame {
        at_us: secs(20.0),
        duration_us: secs(8.0),
    };
    let p = apply_ops(&abc(), &[freeze]).unwrap();
    let i = p.timeline.main.iter().position(|c| c.is_freeze()).unwrap();
    let id = p.timeline.main[i].id.clone();
    assert_eq!(
        apply_ops(&p, &[slip(&id, 1.0)]).unwrap_err(),
        refused("A freeze frame holds one frame, so it has no footage to slip.")
    );
    let rolled = apply_ops(&p, &[roll(&id, 1.0)]).unwrap();
    assert_eq!(rolled.timeline.main[i].duration_us(), secs(9.0));
}

#[test]
fn nothing_moves_for_a_zero_and_the_edits_are_judged_as_removals() {
    let p = abc();
    for op in [roll("a", 0.0), slip("b", 0.0), slide("b", 0.0)] {
        assert_eq!(apply_ops(&p, std::slice::from_ref(&op)).unwrap(), p);
        assert_eq!(op_risk(&op), OpRisk::Removal);
        assert_eq!(serde_json::to_value(&op).unwrap()["type"], op.tag());
    }
}
