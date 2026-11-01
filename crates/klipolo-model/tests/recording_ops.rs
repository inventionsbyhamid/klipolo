mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::ops::{op_risk, Op, OpRisk, SourcePatch};
use klipolo_model::project::*;
use klipolo_model::risk::removals;
use klipolo_model::time::Us;
use klipolo_model::timeline::{EffectInstance, ItemContent};
use klipolo_model::{apply_ops, Project, Transaction};

fn apply(p: &Project, ops: Vec<Op>) -> Project {
    apply_ops(p, &ops).unwrap()
}

#[test]
fn a_source_s_volume_label_and_sync_change_in_one_op() {
    let p = apply(
        &fixture(),
        vec![Op::UpdateRecordingSource {
            recording_id: "rec".into(),
            source_id: "mic".into(),
            patch: SourcePatch {
                label: Some("Voice".into()),
                mix: Some(SourceMix {
                    volume: 0.4,
                    ..SourceMix::default()
                }),
                nudge_us: Some(-40_000),
                ..SourcePatch::default()
            },
        }],
    );
    let mic = p.recording("rec").unwrap().source("mic").unwrap();
    assert_eq!(
        (mic.label.as_str(), mic.mix.volume, mic.nudge_us),
        ("Voice", 0.4, -40_000)
    );
    let missing = Op::UpdateRecordingSource {
        recording_id: "rec".into(),
        source_id: "nope".into(),
        patch: SourcePatch::default(),
    };
    assert!(apply_ops(&fixture(), &[missing]).is_err());
}

#[test]
fn a_clip_can_mute_a_source_only_there_and_take_it_back() {
    let muted = SourceMix {
        muted: true,
        ..SourceMix::default()
    };
    let p = apply(
        &fixture(),
        vec![Op::SetClipSourceMix {
            clip_id: "a".into(),
            source_id: "mic".into(),
            mix: Some(muted),
        }],
    );
    assert!(
        p.timeline.main[0]
            .mix_for(p.recording("rec").unwrap().source("mic").unwrap())
            .muted
    );
    let p = apply(
        &p,
        vec![Op::SetClipSourceMix {
            clip_id: "a".into(),
            source_id: "mic".into(),
            mix: None,
        }],
    );
    assert!(p.timeline.main[0].source_mix.is_empty());
    assert!(
        apply_ops(
            &p,
            &[Op::SetClipSourceMix {
                clip_id: "b".into(),
                source_id: "mic".into(),
                mix: None
            }]
        )
        .is_err(),
        "an imported clip has no sources"
    );
}

#[test]
fn separating_the_camera_makes_one_layer_per_run_inside_the_clip() {
    let mut p = fixture();
    // A camera that reconnected: 0.2–9.8 s and 12–59.8 s, the second in a file of its own.
    second_file(&mut p, "camera", "camera-2");
    let camera = &mut p.recordings[0].sources[1];
    camera.runs = vec![
        SourceRun {
            asset_id: "camera".into(),
            offset_us: 200_000,
            duration_us: secs(9.6),
            drift_ppm: 0.0,
            file_in_us: 0,
        },
        SourceRun {
            asset_id: "camera-2".into(),
            offset_us: secs(12.0),
            duration_us: secs(47.8),
            drift_ppm: 0.0,
            file_in_us: 0,
        },
    ];
    let p = apply(
        &p,
        vec![Op::DetachSource {
            clip_id: "a".into(),
            source_id: "camera".into(),
        }],
    );
    let layers: Vec<_> = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .map(|i| (i.id.as_str(), i.start_us, i.duration_us))
        .collect();
    assert_eq!(
        layers,
        [
            ("a-camera", 200_000, secs(9.6)),
            ("a-camera-2", secs(12.0), secs(8.0))
        ]
    );
    let again = apply_ops(
        &p,
        &[Op::DetachSource {
            clip_id: "a".into(),
            source_id: "camera".into(),
        }],
    );
    assert_eq!(
        again.unwrap_err(),
        refused("This clip's camera is already in its own layer.")
    );
}

#[test]
fn separating_the_mic_makes_attached_sound_layers_and_mutes_it_on_the_clip() {
    let p = apply(
        &fixture(),
        vec![Op::DetachSource {
            clip_id: "a".into(),
            source_id: "mic".into(),
        }],
    );
    let item = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == "a-mic")
        .unwrap();
    assert!(
        matches!(&item.content, ItemContent::Audio { asset_id, source_in_us: 0, .. } if asset_id == "mic")
    );
    assert_eq!(
        (item.start_us, item.duration_us, item.attach.is_some()),
        (0, secs(20.0), true)
    );
    assert!(
        p.timeline.main[0].source_mix["mic"].muted,
        "not heard twice"
    );
    let mut fast = fixture();
    fast.timeline.main[0].speed = 2.0;
    assert_eq!(
        apply_ops(
            &fast,
            &[Op::DetachSource {
                clip_id: "a".into(),
                source_id: "mic".into()
            }]
        )
        .unwrap_err(),
        refused("Set the clip back to normal speed before separating its sound.")
    );
    assert_eq!(
        apply_ops(
            &fixture(),
            &[Op::DetachSource {
                clip_id: "a".into(),
                source_id: "screen".into()
            }]
        )
        .unwrap_err(),
        refused("The main footage can't be separated from its clip.")
    );
}

#[test]
fn markers_stay_inside_the_recording_and_removing_one_is_a_removal() {
    let marker = Marker {
        id: "m1".into(),
        at_us: secs(12.5),
        kind: MarkerKind::Manual,
        label: Some("Boss fight".into()),
    };
    let p = apply(
        &fixture(),
        vec![Op::AddMarker {
            recording_id: "rec".into(),
            marker: marker.clone(),
        }],
    );
    assert_eq!(
        p.recording("rec").unwrap().markers,
        std::slice::from_ref(&marker)
    );
    let outside = Marker {
        at_us: secs(90.0),
        ..marker.clone()
    };
    assert!(apply_ops(
        &p,
        &[Op::AddMarker {
            recording_id: "rec".into(),
            marker: Marker {
                id: "m2".into(),
                ..outside
            }
        }]
    )
    .is_err());
    let remove = Op::RemoveMarker {
        recording_id: "rec".into(),
        id: "m1".into(),
    };
    assert_eq!(op_risk(&remove), OpRisk::Removal);
    let after = apply(&p, vec![remove]);
    assert_eq!(removals(&p, &after).markers, ["m1".to_string()]);
}

fn detach_mic() -> Op {
    Op::DetachSource {
        clip_id: "a".into(),
        source_id: "mic".into(),
    }
}

/// Each sound layer of the mic's file: (start, length, where in the file it starts), by start.
fn mic_layers(p: &Project) -> Vec<(Us, Us, Us)> {
    let mut out: Vec<_> = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .filter_map(|i| match &i.content {
            ItemContent::Audio {
                asset_id,
                source_in_us,
                ..
            } if asset_id == "mic" => Some((i.start_us, i.duration_us, *source_in_us)),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out
}

/// A separated sound takes its clip's fades as its own (sync must hold): the clip's fade-in and fade-out where it
/// reaches the clip's edges, or, across a transition, the part of the window inside the clip.
#[test]
fn a_separated_sound_takes_its_clip_s_fades() {
    let fades = |p: &Project| {
        p.timeline
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .find_map(|i| match &i.content {
                ItemContent::Audio {
                    fade_in_us,
                    fade_out_us,
                    ..
                } => Some((*fade_in_us, *fade_out_us)),
                _ => None,
            })
            .unwrap()
    };
    let mut faded = fixture();
    (
        faded.timeline.main[0].fade_in_us,
        faded.timeline.main[0].fade_out_us,
    ) = (secs(1.0), secs(2.0));
    assert_eq!(
        fades(&apply(&faded, vec![detach_mic()])),
        (secs(1.0), secs(2.0))
    );
    // A 1 s dissolve into "b" (which has footage before its in point): the half inside "a".
    let mut cut = fixture();
    cut.timeline.main[1].source_in_us = secs(2.0);
    let dissolve = Op::SetTransition {
        clip_id: "a".into(),
        transition: Some(klipolo_model::timeline::Transition {
            effect: klipolo_model::contrib::builtin::TRANSITION_DISSOLVE.into(),
            duration_us: secs(1.0),
            params: Default::default(),
        }),
    };
    assert_eq!(
        fades(&apply(&cut, vec![dissolve, detach_mic()])),
        (0, secs(0.5))
    );
}

/// A later sync change moves what a separated sound plays with its source (sync must hold). The layer keeps its place
/// and plays the file as the clip's own sound would there; where the file doesn't reach, it is cut back.
#[test]
fn a_sync_change_slips_the_separated_sound_with_its_source() {
    let separated = apply(&fixture(), vec![detach_mic()]);
    assert_eq!(mic_layers(&separated), [(0, secs(20.0), 0)]);
    let nudge = |us: Us| Op::UpdateRecordingSource {
        recording_id: "rec".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            nudge_us: Some(us),
            ..SourcePatch::default()
        },
    };
    // 0.5 s later: the file now begins 0.5 s into the layer, so the layer begins there.
    let later = apply(&separated, vec![nudge(500_000)]);
    assert_eq!(mic_layers(&later), [(secs(0.5), secs(19.5), 0)]);
    // Back again: in sync from where it now begins (it doesn't grow back).
    assert_eq!(
        mic_layers(&apply(&later, vec![nudge(0)])),
        [(secs(0.5), secs(19.5), secs(0.5))]
    );
    // 1 s earlier: the same place, a second further into the file.
    assert_eq!(
        mic_layers(&apply(&separated, vec![nudge(-1_000_000)])),
        [(0, secs(20.0), secs(1.0))]
    );
}

fn nudge_camera(us: Us) -> Op {
    Op::UpdateRecordingSource {
        recording_id: "rec".into(),
        source_id: "camera".into(),
        patch: SourcePatch {
            nudge_us: Some(us),
            ..SourcePatch::default()
        },
    }
}

fn detach_camera(clip: &str) -> Op {
    Op::DetachSource {
        clip_id: clip.into(),
        source_id: "camera".into(),
    }
}

/// Each video layer of the camera's file: (start, length, where in the file it starts), by start.
fn camera_layers(p: &Project) -> Vec<(Us, Us, Us)> {
    let mut out: Vec<_> = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .filter_map(|i| match &i.content {
            ItemContent::Video {
                asset_id,
                source_in_us,
                ..
            } if asset_id == "camera" => Some((i.start_us, i.duration_us, *source_in_us)),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out
}

/// A later sync change moves a separated camera with its source, as separated sound slips.
/// The layer keeps its place and shows what the clip's own camera would there, so the change is a proposal for chat
/// (the layer shows another stretch of its file); where the file runs out it is cut back, never past its clip; and undo
/// puts it back exactly.
#[test]
fn a_sync_change_slips_the_separated_camera_with_its_source() {
    let separated = apply(&fixture(), vec![detach_camera("a")]);
    // The camera starts 0.2 s into the take.
    assert_eq!(camera_layers(&separated), [(secs(0.2), secs(19.8), 0)]);
    // 0.2 s later: the file now begins 0.4 s into the clip, so the layer begins there.
    let later = apply(&separated, vec![nudge_camera(200_000)]);
    assert_eq!(camera_layers(&later), [(secs(0.4), secs(19.6), 0)]);
    assert_eq!(removals(&separated, &later).shortened, ["a-camera"]);
    // 1 s earlier: the same place, a second further into the file.
    assert_eq!(
        camera_layers(&apply(&separated, vec![nudge_camera(-1_000_000)])),
        [(secs(0.2), secs(19.8), secs(1.0))]
    );
    // Near the end of the file (59.8 s long, the take's last 20 s in clip "a"): a second earlier runs out of file, so
    // the layer is cut back; it never reaches past its clip into the b-roll.
    let mut end = fixture();
    end.assets[1].duration_us = secs(59.8);
    (
        end.timeline.main[0].source_in_us,
        end.timeline.main[0].source_out_us,
    ) = (secs(40.0), secs(60.0));
    let end = apply(&end, vec![detach_camera("a")]);
    assert_eq!(camera_layers(&end), [(0, secs(20.0), secs(39.8))]);
    assert_eq!(
        camera_layers(&apply(&end, vec![nudge_camera(-1_000_000)])),
        [(0, secs(19.0), secs(40.8))]
    );
    // Undo puts back exactly what was there.
    let tx = |op: Op| Transaction {
        label: "edit".into(),
        ops: vec![op],
        origin: None,
        ..Default::default()
    };
    let mut doc = Document::new(fixture()).unwrap();
    doc.apply(&tx(detach_camera("a"))).unwrap();
    let before = doc.project().clone();
    doc.apply(&tx(nudge_camera(200_000))).unwrap();
    assert_ne!(*doc.project(), before);
    doc.undo();
    assert_eq!(*doc.project(), before);
}

/// Losing a source's audio effect is a removal, whichever op replaces the mix: from its default (the
/// recording is named), or from a clip's own mix (the clip is). A new volume with the effect kept takes nothing away.
#[test]
fn losing_a_source_s_audio_effect_is_a_removal() {
    let voice = || EffectInstance {
        id: "fx".into(),
        ..EffectInstance::new(klipolo_model::contrib::builtin::audio_effect("voice"))
    };
    let mix = |volume: f64, effects: Vec<EffectInstance>| SourceMix {
        volume,
        effects,
        ..SourceMix::default()
    };
    let mut p = fixture();
    p.recordings[0].sources[2].mix = mix(1.0, vec![voice()]);
    p.timeline.main[0]
        .source_mix
        .insert("mic".into(), mix(0.5, vec![voice()]));
    let p = apply(&p, vec![]);
    let default = Op::UpdateRecordingSource {
        recording_id: "rec".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            mix: Some(mix(1.0, vec![])),
            ..SourcePatch::default()
        },
    };
    assert_eq!(op_risk(&default), OpRisk::Removal);
    assert_eq!(removals(&p, &apply(&p, vec![default])).effects, ["rec"]);
    let own = |m: SourceMix| Op::SetClipSourceMix {
        clip_id: "a".into(),
        source_id: "mic".into(),
        mix: Some(m),
    };
    assert_eq!(
        removals(&p, &apply(&p, vec![own(mix(0.5, vec![]))])).effects,
        ["a"]
    );
    assert!(removals(&p, &apply(&p, vec![own(mix(0.8, vec![voice()]))])).is_empty());
}

/// A split cuts a separated sound layer too, so each piece keeps its own voice: removing the right
/// piece takes its part of the mic with it, and moving it moves its part along.
#[test]
fn a_split_cuts_a_separated_sound_and_each_piece_keeps_its_voice() {
    let p = apply(
        &fixture(),
        vec![detach_mic(), Op::Split { at_us: secs(8.0) }],
    );
    let (left, right) = ((0, secs(8.0), 0), (secs(8.0), secs(12.0), secs(8.0)));
    assert_eq!(mic_layers(&p), [left, right]);
    let piece = p.timeline.main[1].id.clone();
    let removed = apply(&p, vec![Op::RemoveClip { id: piece.clone() }]);
    assert_eq!(mic_layers(&removed), [left], "no voice under the b-roll");
    // The b-roll (10 s) now plays at 8–18 s and the right piece after it.
    let moved = apply(
        &p,
        vec![Op::MoveClip {
            id: piece,
            index: 2,
        }],
    );
    assert_eq!(
        mic_layers(&moved),
        [left, (secs(18.0), secs(12.0), secs(8.0))]
    );
}

/// Separating a sound keeps what was heard: a mic muted in its clip stays muted as a layer, and one in
/// a clip at 50 % stays at 50 %.
#[test]
fn a_separated_sound_plays_as_its_clip_played_it() {
    let sound = |p: &Project| {
        p.timeline
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .find_map(|i| match &i.content {
                ItemContent::Audio { volume, muted, .. } => Some((*volume, *muted)),
                _ => None,
            })
            .unwrap()
    };
    let mut muted = fixture();
    muted.timeline.main[0].source_mix.insert(
        "mic".into(),
        SourceMix {
            muted: true,
            ..SourceMix::default()
        },
    );
    assert_eq!(sound(&apply(&muted, vec![detach_mic()])), (1.0, true));
    let mut half = fixture();
    half.timeline.main[0].volume = 0.5;
    assert_eq!(sound(&apply(&half, vec![detach_mic()])), (0.5, false));
    // Every one of the mix's audio effects goes with it, in order, with its settings and whether it is on: here a
    // built-in and a plugin's with a setting, switched off.
    let mut effected = fixture();
    let mut robot = EffectInstance::new("com.acme.fx/audio.robot");
    (robot.id, robot.enabled) = ("fx2".into(), false);
    robot.params.insert(
        "pitch".into(),
        klipolo_model::params::ParamValue::Number(0.3),
    );
    let mut voice = EffectInstance::new(klipolo_model::contrib::builtin::audio_effect("voice"));
    voice.id = "fx1".into();
    effected.recordings[0].sources[2].mix.effects = vec![voice, robot];
    let p = apply(&effected, vec![detach_mic()]);
    let layer = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| matches!(i.content, ItemContent::Audio { .. }))
        .unwrap();
    assert_eq!(layer.effects, effected.recordings[0].sources[2].mix.effects);
    assert!(matches!(
        layer.content,
        ItemContent::Audio { effect: None, .. }
    ));
}

/// Replacing a marker (`AddMarker` with an id already there) moves or renames it: a removal, as removing it is.
#[test]
fn replacing_a_marker_is_a_removal() {
    let marker = |at: f64| Marker {
        id: "m1".into(),
        at_us: secs(at),
        kind: MarkerKind::Hotkey,
        label: None,
    };
    let add = |at: f64| Op::AddMarker {
        recording_id: "rec".into(),
        marker: marker(at),
    };
    let p = apply(&fixture(), vec![add(5.0)]);
    assert_eq!(removals(&p, &apply(&p, vec![add(9.0)])).markers, ["m1"]);
    assert!(removals(&p, &apply(&p, vec![add(5.0)])).is_empty());
}

/// A one-sided source plays centred from one channel, by default or in one clip's own mix (as its volume), and a
/// separated layer keeps it. Only a channel a file plays is accepted, and the setting survives undo, redo, saving and
/// loading.
#[test]
fn a_one_sided_source_plays_centred_and_keeps_it() {
    let centred = |k: u8| SourceMix {
        centre_from: Some(k),
        ..SourceMix::default()
    };
    let default_mix = |mix: SourceMix| Op::UpdateRecordingSource {
        recording_id: "rec".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            mix: Some(mix),
            ..SourcePatch::default()
        },
    };
    let clip_mix = |mix: SourceMix| Op::SetClipSourceMix {
        clip_id: "a".into(),
        source_id: "mic".into(),
        mix: Some(mix),
    };
    let tx = |op: Op| Transaction {
        label: "edit".into(),
        ops: vec![op],
        origin: None,
        ..Default::default()
    };
    let mut doc = Document::new(fixture()).unwrap();
    doc.apply(&tx(default_mix(centred(0)))).unwrap();
    doc.apply(&tx(clip_mix(SourceMix {
        volume: 0.5,
        ..centred(1)
    })))
    .unwrap();
    let p = doc.project().clone();
    let centre = |p: &Project| {
        let mic = p.recording("rec").unwrap().source("mic").unwrap();
        (
            mic.mix.centre_from,
            p.timeline.main[0].mix_for(mic).centre_from,
        )
    };
    assert_eq!(centre(&p), (Some(0), Some(1)));

    let json = serde_json::to_string(&p).unwrap();
    assert!(
        json.contains(r#""centreFrom":0"#),
        "camelCase, beside the volume"
    );
    assert_eq!(klipolo_model::load_project(&json).unwrap(), p);
    doc.undo();
    doc.undo();
    assert_eq!(centre(doc.project()), (None, None));
    doc.redo();
    doc.redo();
    assert_eq!(*doc.project(), p);

    let refusal = refused("A sound can play centred from its first or second channel only.");
    assert_eq!(
        apply_ops(&p, &[default_mix(centred(2))]),
        Err(refusal.clone())
    );
    assert_eq!(apply_ops(&p, &[clip_mix(centred(2))]), Err(refusal));

    // Separated, the mic's layer plays the channel the clip played it from.
    let separated = apply(
        &p,
        vec![Op::DetachSource {
            clip_id: "a".into(),
            source_id: "mic".into(),
        }],
    );
    let layer = separated
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .next();
    assert!(matches!(
        layer.map(|i| &i.content),
        Some(ItemContent::Audio {
            centre_from: Some(1),
            ..
        })
    ));
}
