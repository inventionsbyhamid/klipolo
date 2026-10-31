use std::cell::Cell;

use super::*;
use crate::contrib::builtin;
use crate::layout::{Anchor, BoxN, Layout};
use crate::ops::{ClipPatch, Edge, EffectPatch, EffectTarget, ItemPatch, TrackPatch, TrackTarget};
use crate::project::{
    AssetKind, AudioSettings, Background, CaptionSettings, FormatKind, Loudness, RecordingKind,
    RecordingSource, Settings, SourceMix, SourceRole, SourceRun,
};
use crate::query::anchor_at;
use crate::time::{RangeUs, US_PER_SEC as S};
use crate::timeline::{
    Attach, CameraLayout, ClipSource, EffectInstance, ItemContent, TextStyle, Transition,
};

thread_local! {
    /// Whole-project passes `apply_ops` ran on this thread.
    pub(super) static WHOLE_PASSES: Cell<usize> = const { Cell::new(0) };
    /// Speed runs played on lanes, passes that stopped, sweeps that bumped, lanes added, sweeps of a free layer that
    /// met a moved one, and free layers checked against the moved ones (`speed_lanes`).
    pub(super) static LANES: Cell<[usize; 6]> = const { Cell::new([0; 6]) };
}

/// Counts a `speed_lanes` event: 0 a run played, 1 a pass stopped, 2 a sweep that bumped, 3 a lane added, 4 a sweep of
/// a free layer and an anchored one that collided, 5 a free layer checked against its moved neighbours.
pub(super) fn lane_event(n: usize) {
    LANES.with(|c| {
        let mut v = c.get();
        v[n] += 1;
        c.set(v);
    });
}

/// Ten 40 s clips of one file at 0.75× (so anchors round), with 100 one-second titles attached across them.
fn titled() -> Project {
    let mut p = Project::new("Batch");
    p.assets = vec![Asset {
        id: "file".into(),
        kind: AssetKind::Video,
        path: "/media/file".into(),
        name: "file".into(),
        duration_us: 300 * S,
        width: 1920,
        height: 1080,
        fps: 30.0,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }];
    p.timeline.main = (0..10)
        .map(|k| {
            let source = ClipSource::Asset { id: "file".into() };
            let mut c = Clip::new(source, k * 30 * S, (k + 1) * 30 * S);
            c.id = format!("clip-{k}");
            c.speed = 0.75;
            c
        })
        .collect();
    let mut track = Track::new(TrackKind::Overlay, "Titles");
    track.items = (0..100)
        .map(|k| Item {
            id: format!("title-{k}"),
            start_us: k * 4 * S,
            duration_us: S,
            attach: anchor_at(&p.timeline.main, k * 4 * S),
            layout: Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.1,
                    y: 0.1,
                    w: 0.5,
                    h: 0.1,
                },
            ),
            content: ItemContent::Text {
                text: "Title".into(),
                style: TextStyle::default(),
            },
            effects: Vec::new(),
        })
        .collect();
    p.timeline.tracks = vec![track];
    normalize(&mut p);
    p
}

fn passes() -> usize {
    WHOLE_PASSES.with(Cell::get)
}

/// `apply_ops` as it was before batches settled incrementally: the old whole-project pass (every track sorted and
/// bumped, `super::reference`) after every op.
fn whole_pass_after_every_op(project: &Project, ops: &[Op]) -> Result<Project> {
    let mut next = project.clone();
    for op in ops {
        // A fresh index each op, never settled: every op takes the path that looks at every layer.
        apply_one(&mut next, op, &mut Places::default())?;
        super::reference::normalize_before_pa1(&mut next);
    }
    crate::transitions::normalize(&mut next);
    validate(&next)?;
    Ok(next)
}

fn file(id: &str, kind: AssetKind) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: 300 * S,
        width: 1280,
        height: 720,
        fps: 30.0,
        has_audio: kind == AssetKind::Audio,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }
}

/// Recording "rec": one run per source over its whole file ([`file`]'s 300 s), each file named as its source.
fn take(sources: &[(&str, SourceRole)]) -> Recording {
    Recording {
        id: "rec".into(),
        kind: RecordingKind::Screen,
        sources: sources
            .iter()
            .map(|&(id, role)| RecordingSource {
                id: id.into(),
                role,
                label: id.into(),
                runs: vec![SourceRun {
                    asset_id: id.into(),
                    offset_us: 0,
                    duration_us: 300 * S,
                    drift_ppm: 0.0,
                    file_in_us: 0,
                }],
                nudge_us: 0,
                mix: SourceMix::default(),
                synced: None,
            })
            .collect(),
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }
}

fn title(id: &str, start_us: Us) -> Item {
    let mut item = titled().timeline.tracks[0].items[0].clone();
    item.id = id.into();
    item.start_us = start_us;
    item
}

#[test]
fn every_kind_of_op_settles_as_the_whole_pass_would() {
    let p = titled();
    let move_to = |id: &str, start_us: Us| Op::MoveItem {
        id: id.into(),
        track_id: None,
        start_us,
    };
    // Every kind of op, at odd microseconds through 0.75× clips so anchors round, with layer edits between
    // main-track edits.
    let ops = vec![
        move_to("title-3", 13 * S + 1_333_334),
        Op::AddItem {
            track: TrackTarget::Auto,
            item: title("extra", 2 * S + 500_001),
            attach: true,
        },
        Op::TrimItem {
            id: "title-5".into(),
            edge: Edge::Start,
            at_us: 20 * S + 3,
        },
        Op::TrimItem {
            id: "title-11".into(),
            edge: Edge::End,
            at_us: 44 * S + 500_001,
        },
        Op::SplitItem {
            id: "title-6".into(),
            at_us: 24 * S + 500_001,
        },
        Op::SetAttach {
            id: "title-7".into(),
            attached: false,
        },
        Op::SetAttach {
            id: "title-7".into(),
            attached: true,
        },
        Op::UpdateItem {
            id: "title-8".into(),
            patch: ItemPatch {
                duration_us: Some(2 * S),
                ..Default::default()
            },
        },
        Op::AddEffect {
            target: EffectTarget::Item {
                id: "title-9".into(),
            },
            effect: EffectInstance::new(builtin::EFFECT_BLUR),
            index: None,
        },
        Op::AddEffect {
            target: EffectTarget::Item {
                id: "title-9".into(),
            },
            effect: EffectInstance::new(builtin::EFFECT_VIGNETTE),
            index: None,
        },
        Op::UpdateEffect {
            target: EffectTarget::Item {
                id: "title-9".into(),
            },
            id: "fx".into(),
            patch: EffectPatch {
                enabled: Some(false),
                ..Default::default()
            },
        },
        Op::MoveEffect {
            target: EffectTarget::Item {
                id: "title-9".into(),
            },
            id: "fx".into(),
            index: 1,
        },
        Op::RemoveEffect {
            target: EffectTarget::Item {
                id: "title-9".into(),
            },
            id: "fx-2".into(),
        },
        Op::UpdateClip {
            id: "clip-2".into(),
            patch: ClipPatch {
                volume: Some(0.5),
                ..Default::default()
            },
        },
        // Overlaps title-21 (84–85 s): bumped onto a new track.
        move_to("title-20", 83 * S + 600_001),
        Op::UpdateClip {
            id: "clip-3".into(),
            patch: ClipPatch {
                speed: Some(1.25),
                ..Default::default()
            },
        },
        move_to("title-40", 150 * S + 7),
        Op::MoveClip {
            id: "clip-0".into(),
            index: 5,
        },
        move_to("title-41", 151 * S + 1_333_334),
        Op::TrimClip {
            id: "clip-1".into(),
            edge: Edge::End,
            source_us: 55 * S + 1,
        },
        Op::Split { at_us: 101 * S + 1 },
        Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: 150 * S + 1,
                end: 151 * S + 500_000,
            }],
        },
        move_to("title-60", 200 * S + 3),
        Op::SetTransition {
            clip_id: "clip-4".into(),
            transition: Some(Transition {
                effect: builtin::TRANSITION_DISSOLVE.into(),
                duration_us: S / 2,
                params: Default::default(),
            }),
        },
        Op::AddTrack {
            id: None,
            kind: TrackKind::Overlay,
            index: Some(0),
        },
        Op::UpdateTrack {
            id: "overlay-track".into(),
            patch: TrackPatch {
                hidden: Some(true),
                ..Default::default()
            },
        },
        Op::MoveTrack {
            id: "overlay-track".into(),
            index: 3,
        },
        Op::RemoveItem {
            id: "title-50".into(),
        },
        Op::AddAssets {
            assets: vec![
                file("screen", AssetKind::Video),
                file("camera", AssetKind::Video),
                file("mic", AssetKind::Audio),
                file("spare", AssetKind::Audio),
            ],
        },
        Op::RemoveAsset { id: "spare".into() },
        Op::AddRecording {
            recording: take(&[
                ("screen", SourceRole::Screen),
                ("camera", SourceRole::Camera),
                ("mic", SourceRole::Mic),
            ]),
        },
        Op::InsertClip {
            clip: {
                let source = ClipSource::Recording { id: "rec".into() };
                let mut c = Clip::new(source, 10 * S, 40 * S + 3);
                c.id = "recorded".into();
                c.speed = 0.75;
                c.camera = Some(CameraLayout::default());
                c
            },
            index: Some(2),
        },
        Op::DetachSource {
            source_id: "camera".into(),
            clip_id: "recorded".into(),
        },
        // The recording ops that change no timing.
        Op::UpdateRecordingSource {
            recording_id: "rec".into(),
            source_id: "mic".into(),
            patch: crate::ops::SourcePatch {
                mix: Some(SourceMix {
                    volume: 0.5,
                    ..SourceMix::default()
                }),
                nudge_us: Some(-40_000),
                ..Default::default()
            },
        },
        Op::SetClipSourceMix {
            clip_id: "recorded".into(),
            source_id: "mic".into(),
            mix: Some(SourceMix {
                muted: true,
                ..SourceMix::default()
            }),
        },
        Op::AddMarker {
            recording_id: "rec".into(),
            marker: crate::project::Marker {
                id: "m1".into(),
                at_us: 12 * S + 1,
                kind: crate::project::MarkerKind::Manual,
                label: None,
            },
        },
        Op::AddMarker {
            recording_id: "rec".into(),
            marker: crate::project::Marker {
                id: "m2".into(),
                at_us: 13 * S,
                kind: crate::project::MarkerKind::Hotkey,
                label: Some("Here".into()),
            },
        },
        Op::RemoveMarker {
            recording_id: "rec".into(),
            id: "m1".into(),
        },
        move_to("title-70", 260 * S + 1_333_334),
        Op::RemoveTrack {
            id: "overlay-track".into(),
        },
        Op::RemoveClip {
            id: "clip-9".into(),
        },
        move_to("title-80", 300 * S + 5),
        Op::Rename {
            name: "Renamed".into(),
        },
        Op::SetSettings {
            settings: Settings {
                formats: vec![FormatKind::Landscape, FormatKind::Portrait],
                primary_format: FormatKind::Portrait,
                ..Settings::default()
            },
        },
        Op::SetCaptions {
            captions: CaptionSettings {
                enabled: true,
                ..Default::default()
            },
        },
        Op::SetBackground {
            background: Background::default(),
        },
        Op::SetAudio {
            audio: AudioSettings {
                loudness: Loudness::Off,
            },
        },
        Op::AddLut {
            lut: ColorLut {
                id: "lut".into(),
                name: "Film".into(),
                path: "/luts/film.cube".into(),
            },
        },
        Op::RemoveLut { id: "lut".into() },
    ];
    let settled = apply_ops(&p, &ops).unwrap();
    assert_eq!(settled, whole_pass_after_every_op(&p, &ops).unwrap());
    assert!(settled
        .timeline
        .tracks
        .iter()
        .any(|t| t.id == "title-20-track"));
    assert!(settled.timeline.tracks[0]
        .items
        .iter()
        .any(|item| item.id == "recorded-camera"));
    // An op that fails mid-batch fails it the same way.
    let mut failing = ops.clone();
    failing.insert(5, move_to("missing", S));
    assert_eq!(
        apply_ops(&p, &failing),
        whole_pass_after_every_op(&p, &failing)
    );
}

#[test]
fn a_hundred_moves_in_one_batch_match_a_hundred_batches_for_one_whole_pass() {
    let p = titled();
    let ops: Vec<Op> = (0..100)
        .map(|k| Op::MoveItem {
            id: format!("title-{k}"),
            track_id: None,
            start_us: k * 4 * S + 1_333_334,
        })
        .collect();
    let before = passes();
    let one_by_one = ops.iter().fold(p.clone(), |q, op| {
        apply_ops(&q, std::slice::from_ref(op)).unwrap()
    });
    assert_eq!(
        passes() - before,
        100,
        "a one-op batch normalises all of it"
    );
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(passes() - before, 1, "the batch normalises all of it once");
    assert_eq!(batch, one_by_one);
    // The moves landed, re-anchored through the 0.75× clips: the anchor rounds them 1 µs later.
    let title = &batch.timeline.tracks[0].items[99];
    assert_eq!(title.start_us, 396 * S + 1_333_335);
    assert!(title.attach.is_some());
}

#[test]
fn a_main_track_edit_in_a_batch_resolves_every_anchor_again() {
    let p = titled();
    let ops = [
        Op::MoveItem {
            id: "title-0".into(),
            track_id: None,
            start_us: S,
        },
        Op::MoveClip {
            id: "clip-0".into(),
            index: 9,
        },
        Op::MoveItem {
            id: "title-1".into(),
            track_id: None,
            start_us: 5 * S,
        },
    ];
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        passes() - before,
        2,
        "after the first op and after the clip move"
    );
    let one_by_one = ops.iter().fold(p.clone(), |q, op| {
        apply_ops(&q, std::slice::from_ref(op)).unwrap()
    });
    assert_eq!(batch, one_by_one);
    // Clip 0 now plays last (360–400 s), and the titles still on it went with it: "title-0" (moved to 1 s first)
    // and "title-2" to "title-9". "title-1" moved to 5 s afterwards, which clip 1 plays now.
    let starts: Vec<Us> = batch.timeline.tracks[0]
        .items
        .iter()
        .filter(|item| item.start_us >= 360 * S)
        .map(|item| item.start_us)
        .collect();
    let expected: Vec<Us> = std::iter::once(361 * S)
        .chain((2..10).map(|k| 360 * S + k * 4 * S))
        .collect();
    assert_eq!(starts, expected);
}

/// Six 20 s clips of one recording with a camera (bubbles), and the titles of [`titled`] over the first 120 s.
fn recorded() -> Project {
    let mut p = titled();
    p.assets.extend([
        file("screen", AssetKind::Video),
        file("camera", AssetKind::Video),
    ]);
    p.recordings = vec![take(&[
        ("screen", SourceRole::Screen),
        ("camera", SourceRole::Camera),
    ])];
    p.timeline.main = (0..6)
        .map(|k| {
            let source = ClipSource::Recording { id: "rec".into() };
            let mut c = Clip::new(source, k * 40 * S, k * 40 * S + 20 * S);
            c.id = format!("take-{k}");
            c.camera = Some(CameraLayout::default());
            c
        })
        .collect();
    p.timeline.tracks[0].items.truncate(30);
    for (k, item) in p.timeline.tracks[0].items.iter_mut().enumerate() {
        item.attach = anchor_at(&p.timeline.main, k as Us * 4 * S);
    }
    normalize(&mut p);
    p
}

/// `ops` applied one batch per op: the whole pass after each, the path every deferral must match.
fn one_by_one(p: &Project, ops: &[Op]) -> Project {
    ops.iter().fold(p.clone(), |q, op| {
        apply_ops(&q, std::slice::from_ref(op)).unwrap()
    })
}

fn speed(id: &str, speed: f64) -> Op {
    Op::UpdateClip {
        id: id.into(),
        patch: ClipPatch {
            speed: Some(speed),
            ..Default::default()
        },
    }
}

/// A run of speed changes lays the project out once, with exactly the result of laying it out after
/// each, separated cameras included; a run that would bump a layer on its way goes op by op.
#[test]
fn a_run_of_speed_changes_lays_out_once_as_one_by_one_would() {
    let p = titled();
    let mut ops = vec![Op::MoveItem {
        id: "title-0".into(),
        track_id: None,
        start_us: S,
    }];
    ops.extend((0..10).map(|k| speed(&format!("clip-{k}"), [1.0, 3.0, 0.5, 2.25][k % 4])));
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        passes() - before,
        2,
        "after the first op, and once for the run"
    );
    assert_eq!(batch, one_by_one(&p, &ops));

    // Clip 0 at 4× squeezes its 1 s titles 0.75 s apart, so the whole pass bumps them, and at 0.75× again they stay
    // bumped: laid out once at the end they wouldn't be. The run plays on lanes, each op's pass as the whole pass
    // would run it, without one.
    let ops = [ops[0].clone(), speed("clip-0", 4.0), speed("clip-0", 0.75)];
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(passes() - before, 1, "after the first op only");
    assert_eq!(batch, one_by_one(&p, &ops));
    assert!(
        batch.timeline.tracks.len() > 1,
        "titles were bumped on the way"
    );

    // Separated cameras follow their clips' speed, laid out once too.
    let p = recorded();
    let detach: Vec<Op> = (0..6)
        .map(|k| Op::DetachSource {
            source_id: "camera".into(),
            clip_id: format!("take-{k}"),
        })
        .collect();
    let separated = apply_ops(&p, &detach).unwrap();
    assert_eq!(separated, one_by_one(&p, &detach));
    let mut ops = vec![Op::RemoveItem {
        id: "title-1".into(),
    }];
    ops.extend((0..6).map(|k| speed(&format!("take-{k}"), [2.0, 0.5, 4.0][k % 3])));
    let before = passes();
    let batch = apply_ops(&separated, &ops).unwrap();
    assert_eq!(passes() - before, 2);
    assert_eq!(batch, one_by_one(&separated, &ops));
}

/// A sound layer of `asset` from `start` for `length`, attached to the clip under it; music when `duck` is set.
fn sound_item(
    p: &Project,
    id: &str,
    asset: &str,
    start: Us,
    length: Us,
    duck: Option<f64>,
) -> Item {
    Item {
        id: id.into(),
        start_us: start,
        duration_us: length,
        attach: if duck.is_some() {
            None
        } else {
            anchor_at(&p.timeline.main, start)
        },
        layout: Layout::new(Anchor::TopLeft, BoxN::FULL),
        content: ItemContent::Audio {
            asset_id: asset.into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck,
        },
        effects: Vec::new(),
    }
}

/// Two sound effects that a faster clip squeezes together, under an audio track that holds music with a gap where the
/// second one lands: the bump that moves it up settles alike as one batch, op by op, and through the whole-project
/// reference pass (which `verify-normalize` checks after every op).
#[test]
fn a_bumped_sound_under_music_settles_alike_in_a_batch_and_op_by_op() {
    let mut p = titled();
    p.assets.extend([
        file("pop", AssetKind::Audio),
        file("song", AssetKind::Audio),
    ]);
    let mut sfx = Track::new(TrackKind::Audio, "Sounds");
    sfx.id = "sfx".into();
    sfx.items = vec![
        sound_item(&p, "a", "pop", S, S, None),
        sound_item(&p, "b", "pop", 2 * S + S / 10, S, None),
    ];
    let mut music = Track::new(TrackKind::Audio, "Music");
    music.id = "music".into();
    music.items = vec![sound_item(&p, "bed", "song", 20 * S, 40 * S, Some(-12.0))];
    p.timeline.tracks.extend([sfx, music]);
    normalize(&mut p);
    let ops = vec![
        Op::MoveItem {
            id: "title-0".into(),
            track_id: None,
            start_us: S,
        },
        speed("clip-0", 4.0),
        speed("clip-1", 2.0),
    ];
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(batch, one_by_one(&p, &ops));
    assert_eq!(batch, whole_pass_after_every_op(&p, &ops).unwrap());
    let on = |q: &Project, id: &str| {
        q.timeline.tracks[crate::query::find_item(q, id).unwrap().0]
            .id
            .clone()
    };
    assert_eq!(on(&batch, "a"), "sfx");
    assert_ne!(
        on(&batch, "b"),
        "sfx",
        "squeezed onto the first, it was bumped up"
    );
}

/// What a batch finds through its index, and the layers it updates in their slots, give exactly
/// the op-by-op result: separating every camera, then shaping and fading every camera layer.
#[test]
fn indexed_batches_match_one_op_at_a_time() {
    let p = recorded();
    let detach: Vec<Op> = (0..6)
        .map(|k| Op::DetachSource {
            source_id: "camera".into(),
            clip_id: format!("take-{k}"),
        })
        .collect();
    let separated = apply_ops(&p, &detach).unwrap();
    let cameras: Vec<String> = separated
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .filter(|i| matches!(i.content, ItemContent::Video { .. }))
        .map(|i| i.id.clone())
        .collect();
    assert_eq!(cameras.len(), 6);
    let mut ops = Vec::new();
    for id in &cameras {
        let item = &separated.timeline.tracks[crate::query::find_item(&separated, id).unwrap().0]
            .items[crate::query::find_item(&separated, id).unwrap().1];
        ops.push(Op::AddEffect {
            target: EffectTarget::Item { id: id.clone() },
            effect: EffectInstance::new(builtin::EFFECT_MASK),
            index: None,
        });
        let mut layout = item.layout.clone();
        layout.radius = 0.0;
        ops.push(Op::UpdateItem {
            id: id.clone(),
            patch: ItemPatch {
                layout: Some(layout),
                ..Default::default()
            },
        });
    }
    // A length change in the middle takes a layer out of its slot, and one that overlaps the next title is bumped.
    ops.insert(
        5,
        Op::UpdateItem {
            id: "title-3".into(),
            patch: ItemPatch {
                duration_us: Some(6 * S),
                ..Default::default()
            },
        },
    );
    let batch = apply_ops(&separated, &ops).unwrap();
    assert_eq!(batch, one_by_one(&separated, &ops));
    assert!(batch
        .timeline
        .tracks
        .iter()
        .any(|t| t.id == "title-3-track"));
}

/// A track that mixes free and attached layers plays on lanes too: the free layers stay where they are while
/// the titles move past them, and a title that comes to collide with one is bumped (or the free one is, when it starts
/// later), each op's pass as the whole pass runs it, with no whole pass for the run.
#[test]
fn a_run_on_a_track_mixing_free_and_attached_layers_plays_on_lanes() {
    LANES.with(|c| c.set([0; 6]));
    let mut p = titled();
    for (id, start_us) in [("free-end", 398 * S), ("free-mid", 85 * S / 2)] {
        let mut free = title(id, start_us);
        free.attach = None;
        free.duration_us = 3 * S / 2;
        p.timeline.tracks[0].items.push(free);
    }
    p.timeline.tracks[0].items.sort_by_key(|i| i.start_us);
    let p = apply_ops(&p, &[]).unwrap();
    assert_eq!(
        p.timeline.tracks.len(),
        1,
        "every layer fits on the one track"
    );
    // At 2× clip 0 is 25 s shorter, so the titles after it move past the label at 42.5 s and one lands on it; at 4× a
    // clip's titles come 0.75 s apart and bump.
    let ops = vec![
        Op::MoveItem {
            id: "title-0".into(),
            track_id: None,
            start_us: S,
        },
        speed("clip-0", 2.0),
        speed("clip-1", 2.0),
        speed("clip-2", 4.0),
        speed("clip-3", 2.0),
        speed("clip-4", 4.0),
    ];
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(passes() - before, 1, "after the first op only");
    assert_eq!(batch, one_by_one(&p, &ops));
    assert!(batch.timeline.tracks.len() > 1, "titles were bumped");
    let [runs, stops, bumps, _, mixed, _] = LANES.with(Cell::get);
    assert_eq!((runs, stops), (1, 0), "played on lanes to its end");
    assert!(bumps > 0 && mixed > 0, "a title met a free label: {mixed}");
}

/// A run that may bump is planned once. When the lanes can't hold its project (a separated camera
/// that shows in another clip than its own, so its length follows a clip it isn't in), the ops before the first one a
/// colliding pair depends on are laid out once, and the rest go one by one, with exactly the op-by-op result.
#[test]
fn a_run_the_lanes_cant_hold_lays_out_its_safe_start_once_and_the_rest_op_by_op() {
    LANES.with(|c| c.set([0; 6]));
    let p = recorded();
    let detach: Vec<Op> = (0..6)
        .map(|k| Op::DetachSource {
            source_id: "camera".into(),
            clip_id: format!("take-{k}"),
        })
        .collect();
    let mut q = apply_ops(&p, &detach).unwrap();
    // take-0's camera layer, still naming take-0, at a moment take-1 plays (take-k plays 40k–40k+20 s of the
    // recording): it shows in take-1.
    let camera = q
        .timeline
        .tracks
        .iter_mut()
        .flat_map(|t| t.items.iter_mut())
        .find(|i| {
            matches!(i.content, ItemContent::Video { .. })
                && i.attach.as_ref().is_some_and(|a| a.clip_id == "take-0")
        })
        .expect("take-0's camera layer");
    camera.attach.as_mut().unwrap().source_us = 45 * S;
    camera.duration_us = 5 * S;
    // 2 s titles every 4 s: at 4× they collide, at 2× they touch.
    for title in q
        .timeline
        .tracks
        .iter_mut()
        .flat_map(|t| t.items.iter_mut())
        .filter(|i| i.id.starts_with("title-"))
    {
        title.duration_us = 2 * S;
    }
    let q = apply_ops(&q, &[]).unwrap();
    let ops = vec![
        Op::MoveItem {
            id: "title-0".into(),
            track_id: None,
            start_us: S,
        },
        speed("take-3", 2.0),
        speed("take-0", 4.0),
        speed("take-4", 2.0),
    ];
    let before = passes();
    let batch = apply_ops(&q, &ops).unwrap();
    assert_eq!(LANES.with(Cell::get)[0], 0, "the lanes refused the project");
    assert_eq!(
        passes() - before,
        4,
        "after the first op, once for take-3 (no pair that may collide depends on it), then one per op from take-0 on"
    );
    assert_eq!(batch, one_by_one(&q, &ops));
}

/// A layer can't be added under an id the project already has, so no later op in the batch can act
/// on the wrong copy.
#[test]
fn a_layer_added_under_a_taken_id_is_refused() {
    let p = titled();
    let ops = [
        Op::MoveItem {
            id: "title-0".into(),
            track_id: None,
            start_us: S,
        },
        Op::AddItem {
            track: TrackTarget::Auto,
            item: title("title-5", 200 * S),
            attach: true,
        },
        Op::RemoveItem {
            id: "title-5".into(),
        },
    ];
    let e = apply_ops(&p, &ops).unwrap_err();
    assert_eq!(e.coded().code, "model.project.duplicateId");
    assert_eq!(apply_ops(&p, &ops), whole_pass_after_every_op(&p, &ops));
}

/// A run played on lanes stops where the lanes can't follow the whole pass: here a title
/// comes to start at the very moment a free label starts, and the whole pass keeps the two in the order they were in
/// before. That op is laid out by the whole pass and the rest go one by one, with exactly the op-by-op result.
#[test]
fn a_run_on_lanes_stops_where_the_lanes_cant_follow() {
    LANES.with(|c| c.set([0; 6]));
    let mut p = titled();
    let mut free = title("free", 3 * S);
    free.attach = None;
    free.duration_us = S / 2;
    p.timeline.tracks[0].items.push(free);
    p.timeline.tracks[0].items.sort_by_key(|i| i.start_us);
    let p = apply_ops(&p, &[]).unwrap();
    // title-1 shows 3 s of footage in: at 1× it starts at 3 s, with the label.
    let ops = vec![
        Op::MoveItem {
            id: "title-0".into(),
            track_id: None,
            start_us: S,
        },
        speed("clip-0", 1.0),
        speed("clip-2", 4.0),
        speed("clip-3", 2.0),
    ];
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        passes() - before,
        4,
        "after the first op, for clip-0 where the title meets the label, then one per op"
    );
    assert_eq!(
        LANES.with(Cell::get)[..2],
        [1, 1],
        "played on lanes, and stopped"
    );
    assert_eq!(batch, one_by_one(&p, &ops));
    assert!(
        batch
            .timeline
            .tracks
            .iter()
            .any(|t| t.items.iter().any(|i| i.id == "title-1"))
            && !batch.timeline.tracks[0]
                .items
                .iter()
                .any(|i| i.id == "title-1"),
        "the title was bumped off the label's track"
    );
}

/// The ordering case the fuzz found (case 141): a title that started *before* a free label comes
/// to start at the label's very moment (its clip slowed to 0.5×). The whole pass keeps the two in the order they were
/// in, so the label is bumped, not the title. A stopped run writes its tracks by the starts before the op
/// (`clip_back`); by the new ones, the label would come first and the title would be bumped.
#[test]
fn a_tie_after_a_stop_keeps_the_order_from_before_the_op() {
    LANES.with(|c| c.set([0; 6]));
    let mut p = titled();
    // title-1 shows 3 s of footage in, at 4 s (0.75×); at 0.5× it starts at 6 s, with the label.
    let mut label = title("label", 6 * S);
    label.attach = None;
    label.duration_us = S / 2;
    p.timeline.tracks[0].items.push(label);
    p.timeline.tracks[0].items.sort_by_key(|i| i.start_us);
    let p = apply_ops(&p, &[]).unwrap();
    let ops = vec![
        Op::MoveItem {
            id: "title-0".into(),
            track_id: None,
            start_us: S,
        },
        speed("clip-0", 0.5),
        speed("clip-3", 2.0),
    ];
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        LANES.with(Cell::get)[..2],
        [1, 1],
        "played on lanes, and stopped"
    );
    assert_eq!(
        passes() - before,
        3,
        "after the first op, for clip-0 where the title meets the label, then for clip-3"
    );
    assert_eq!(batch, one_by_one(&p, &ops));
    let on_first = |id: &str| batch.timeline.tracks[0].items.iter().any(|i| i.id == id);
    assert!(
        on_first("title-1") && !on_first("label"),
        "the label was bumped"
    );
}

/// Random runs of speed changes, laid out once, played on lanes or one by one, match applying each op
/// as its own batch: two title tracks with titles close enough to bump at some speeds, sometimes a track of free
/// layers or a locked one above them, sometimes separated cameras, and runs that revisit clips.
#[test]
fn random_speed_runs_match_one_op_at_a_time() {
    LANES.with(|c| c.set([0; 6]));
    let mut seed: u64 = 0x5eed_1234_abcd_0001;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for case in 0..120 {
        let mut p = recorded();
        let clips = p.timeline.main.len() as u64;
        let mut extra = Track::new(TrackKind::Overlay, "More");
        extra.id = "more".into();
        // Titles about 3 s apart and up to 1.5 s long: sped up 2× to 4× they collide.
        for k in 0..40 {
            let at = k * 3 * S + next(10) as Us * S / 10;
            let mut item = title(&format!("more-{k}"), at);
            item.duration_us = S / 2 + next(3) as Us * S / 2;
            item.attach = anchor_at(&p.timeline.main, at);
            extra.items.push(item);
        }
        // Sometimes free labels among them, and in the gaps between the first track's titles.
        if next(2) == 0 {
            for k in 0..(1 + next(6)) {
                let at = next(110) as Us * S + next(10) as Us * S / 10;
                let mut label = title(&format!("label-{k}"), at);
                label.duration_us = S / 2 + next(3) as Us * S / 2;
                label.attach = None;
                extra.items.push(label);
            }
            for k in 0..next(4) {
                let mut label = title(&format!("gap-{k}"), (4 * next(30) as Us + 2) * S);
                label.duration_us = S / 2 + next(3) as Us * S / 4;
                label.attach = None;
                p.timeline.tracks[0].items.push(label);
            }
            p.timeline.tracks[0].items.sort_by_key(|i| i.start_us);
            p.timeline.tracks[0]
                .items
                .dedup_by(|b, a| b.start_us < a.end_us());
        }
        extra.items.sort_by_key(|i| i.start_us);
        extra.items.dedup_by(|b, a| b.start_us < a.end_us());
        p.timeline.tracks.push(extra);
        match next(3) {
            0 => {
                let mut free = Track::new(TrackKind::Overlay, "Free");
                free.id = "free".into();
                let mut f = title("free-0", 10 * S + next(100) as Us * S);
                f.attach = None;
                free.items.push(f);
                p.timeline.tracks.push(free);
            }
            1 => {
                let mut locked = Track::new(TrackKind::Overlay, "Locked");
                locked.id = "locked".into();
                locked.locked = true;
                p.timeline.tracks.push(locked);
            }
            _ => {}
        }
        let p = apply_ops(&p, &[]).unwrap();
        let mut ops = vec![Op::UpdateClip {
            id: "take-0".into(),
            patch: ClipPatch {
                volume: Some(0.5),
                ..Default::default()
            },
        }];
        if next(2) == 0 {
            ops.extend((0..6).map(|k| Op::DetachSource {
                source_id: "camera".into(),
                clip_id: format!("take-{k}"),
            }));
        }
        for _ in 0..(2 + next(10)) {
            let clip = next(clips);
            let s = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0][next(6) as usize];
            ops.push(speed(&format!("take-{clip}"), s));
        }
        let batch = apply_ops(&p, &ops);
        let reference = ops
            .iter()
            .try_fold(p.clone(), |q, op| apply_ops(&q, std::slice::from_ref(op)));
        assert_eq!(batch, reference, "case {case}: {ops:?}");
    }
    let [runs, stops, bumps, lanes, mixed, _] = LANES.with(Cell::get);
    assert!(
        runs > 0 && bumps > 0 && lanes > 0 && mixed > 0,
        "the cases play runs on lanes, bump, add lanes and meet free layers: {:?}",
        [runs, stops, bumps, lanes, mixed]
    );
}

/// "Speed every clip up" on a 2-hour project is one run, laid out without a whole pass per
/// op: at 1,500 clips, with every op bumping a title and free labels on the titles' track. Op by op each pass is
/// O(clips + layers), and the run O(clips · (clips + layers)): 554 ms in release at 1,500 clips while the lanes took
/// no free layers. On lanes it is O(clips · (log + the free labels past the op's clip)). Time and heap are measured in
/// `tests/perf_batches.rs` (`speed_runs_that_bump`: ≤ 40 ms at 1,500 clips, ≤ 64 ms at 2,400). In debug builds the run
/// is checked against the ops applied one by one (`verify-normalize`).
#[test]
fn speeding_up_1500_clips_lays_out_without_a_pass_per_op() {
    LANES.with(|c| c.set([0; 6]));
    const CLIPS: usize = 1_500;
    let p = crowded(CLIPS, 16);
    let ops = every_clip_at_4x(CLIPS, false);
    let before = passes();
    let batch = apply_ops(&p, &ops).unwrap();
    let lanes = LANES.with(Cell::get);
    assert_eq!(
        passes() - before,
        1,
        "after the first op only, not one per op: {lanes:?}"
    );
    let [runs, stops, bumps, _, mixed, _] = lanes;
    assert_eq!(
        (runs, stops),
        (1, 0),
        "the 1,500 speed changes play on lanes to the end"
    );
    // Each sped-up clip's second title is bumped, and the titles after it move 3 s closer onto the labels.
    assert!(
        bumps >= CLIPS && mixed > 0,
        "titles bump, and meet labels: {bumps}, {mixed}"
    );
    assert!(batch.timeline.tracks.len() > 1);
}

/// `clips` clips of 4 s from one file, each with two attached 1 s titles 0.1 s apart (at 4× they overlap), and a free
/// 0.6 s label 3 s into every `label_every`-th clip, all on one track: a track mixing attached and free layers.
fn crowded(clips: usize, label_every: usize) -> Project {
    const CLIP: Us = 4 * S;
    let mut p = Project::new("Crowded");
    let mut long = file("file", AssetKind::Video);
    long.duration_us = clips as Us * CLIP + S;
    p.assets = vec![long];
    p.timeline.main = (0..clips)
        .map(|k| {
            let source = ClipSource::Asset { id: "file".into() };
            let mut c = Clip::new(source, k as Us * CLIP, (k as Us + 1) * CLIP);
            c.id = format!("clip-{k}");
            c
        })
        .collect();
    let template = title("template", 0);
    let layer = |id: String, start_us: Us, duration_us: Us, attach: Option<Attach>| Item {
        id,
        start_us,
        duration_us,
        attach,
        ..template.clone()
    };
    let mut track = Track::new(TrackKind::Overlay, "Callouts");
    for k in 0..clips {
        let at = k as Us * CLIP;
        for (n, offset) in [(0, S / 2), (1, 16 * S / 10)] {
            let anchor = anchor_at(&p.timeline.main, at + offset);
            track
                .items
                .push(layer(format!("title-{k}-{n}"), at + offset, S, anchor));
        }
        if k % label_every == 0 {
            track
                .items
                .push(layer(format!("label-{k}"), at + 3 * S, 6 * S / 10, None));
        }
    }
    p.timeline.tracks = vec![track];
    apply_ops(&p, &[]).unwrap()
}

/// The speed changes of `clips` clips to 4× in clip order (or reverse), after a volume change: a batch's first op runs
/// the whole pass (`apply_ops`), so every speed change is in the run.
fn every_clip_at_4x(clips: usize, reverse: bool) -> Vec<Op> {
    let mut ops = vec![Op::UpdateClip {
        id: "clip-0".into(),
        patch: ClipPatch {
            volume: Some(0.5),
            ..Default::default()
        },
    }];
    let mut order: Vec<usize> = (0..clips).collect();
    if reverse {
        order.reverse();
    }
    ops.extend(order.into_iter().map(|k| speed(&format!("clip-{k}"), 4.0)));
    ops
}

/// An op checks only the free layers a moved layer can reach: past the first layer anchored in
/// its clip or later, found by binary search, and none on a lane where no anchored layer moved. At 1,500 clips with
/// 300 free labels the checks stay under 40 per op: 45,750 in clip order and 10,995 in reverse. Checking every free
/// layer past the op's clip's start, as before, made 439,050 in clip order. Times are `perf_batches`'s (≤ 40 ms).
#[test]
fn free_layers_are_checked_only_where_a_moved_layer_reaches() {
    for reverse in [false, true] {
        LANES.with(|c| c.set([0; 6]));
        let p = crowded(1_500, 5);
        apply_ops(&p, &every_clip_at_4x(1_500, reverse)).unwrap();
        let [runs, stops, _, _, mixed, checks] = LANES.with(Cell::get);
        assert_eq!((runs, stops), (1, 0), "on lanes to the end");
        assert!(mixed > 0, "labels were met");
        assert!(
            checks <= 40 * 1_500,
            "{checks} checks of free layers (reverse: {reverse})"
        );
    }
}
