//! Volume keys set by hand: a clip's or sound layer's level over time. They are checked as motion keys are, and every
//! edit moves and trims them with their clip or layer, keeping them on the content.
mod common;
use common::*;
use klipolo_model::motion::{Ease, Key};
use klipolo_model::ops::{ClipPatch, Edge, ItemPatch, Op};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::query::{anchor_at, find_item};
use klipolo_model::timeline::{Clip, Item, ItemContent, ReverseCopy, Track, TrackKind};
use klipolo_model::volume::{level_at, silent};
use klipolo_model::{apply_ops, Project};

fn key(t: f64, v: f64) -> Key {
    Key {
        t_us: secs(t),
        v,
        ease: None,
    }
}

/// Each key as (seconds, level), rounded to the µs and the thousandth.
fn line(keys: &[Key]) -> Vec<(f64, f64)> {
    keys.iter()
        .map(|k| (k.t_us as f64 / 1e6, (k.v * 1000.0).round() / 1000.0))
        .collect()
}

fn clip<'a>(p: &'a Project, id: &str) -> &'a Clip {
    p.timeline
        .main
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no clip {id}"))
}

/// The fixture with "b" (the b-roll at 20 s, 10 s long) going from full to silent between 2 s and 6 s into it.
fn fading() -> Project {
    let mut p = fixture();
    p.timeline.main[1].volume_keys = vec![key(2.0, 1.0), key(6.0, 0.0)];
    apply_ops(&p, &[]).unwrap()
}

fn set_keys(clip: &str, keys: Vec<Key>) -> Op {
    Op::UpdateClip {
        id: clip.into(),
        patch: ClipPatch {
            volume_keys: Some(keys),
            ..Default::default()
        },
    }
}

#[test]
fn the_level_follows_the_keys_and_rests_at_the_volume_without_them() {
    let keys = [key(2.0, 1.0), key(6.0, 0.0)];
    assert_eq!(
        level_at(&keys, secs(0.0), 0.5),
        1.0,
        "the first key's before it"
    );
    assert_eq!(level_at(&keys, secs(4.0), 0.5), 0.5);
    assert_eq!(
        level_at(&keys, secs(9.0), 0.5),
        0.0,
        "the last key's after it"
    );
    assert_eq!(level_at(&[], secs(4.0), 0.7), 0.7, "no keys: the volume");
    // An ease that overshoots stays within the volume's range.
    let springy = [
        key(0.0, 2.0),
        Key {
            ease: Some(Ease::BackIn),
            ..key(1.0, 0.0)
        },
    ];
    assert!((0..=100).all(|i| (0.0..=2.0).contains(&level_at(&springy, i * 10_000, 1.0))));
    assert!(silent(&[key(0.0, 0.0), key(1.0, 0.0)], 1.0));
    assert!(!silent(&[key(0.0, 0.0), key(1.0, 0.2)], 0.0));
    assert!(silent(&[], 0.0) && !silent(&[], 0.1));
}

#[test]
fn a_clip_s_keys_are_set_by_a_patch_and_checked_as_motion_keys_are() {
    let p = apply_ops(
        &fixture(),
        &[set_keys("b", vec![key(1.0, 0.5), key(3.0, 2.0)])],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(1.0, 0.5), (3.0, 2.0)]);
    let refuses = |keys: Vec<Key>, english: &str| {
        assert_eq!(
            apply_ops(&fixture(), &[set_keys("b", keys)]).unwrap_err(),
            refused(english)
        );
    };
    refuses(
        vec![key(1.0, 2.5)],
        "A volume key must be between 0 and 200 %.",
    );
    refuses(
        vec![key(1.0, f64::NAN)],
        "A volume key must be between 0 and 200 %.",
    );
    refuses(
        vec![key(3.0, 1.0), key(1.0, 1.0)],
        "A clip's or sound's volume keys are out of order.",
    );
    refuses(
        vec![key(10.5, 1.0)],
        "A volume key is outside its clip or sound.",
    );
    refuses(
        (0..65).map(|i| key(f64::from(i) * 0.1, 1.0)).collect(),
        "A clip or sound can have at most 64 volume keys.",
    );
    let bad = Key {
        ease: Some(Ease::Steps { n: 0 }),
        ..key(1.0, 1.0)
    };
    assert!(apply_ops(&fixture(), &[set_keys("b", vec![bad])]).is_err());
    // An empty list takes them off.
    let off = apply_ops(&p, &[set_keys("b", vec![])]).unwrap();
    assert!(clip(&off, "b").volume_keys.is_empty());
}

#[test]
fn a_whole_volume_change_scales_every_key() {
    // "Turn it down": half the volume halves the line, so chat's and the song-wide slider's change is heard.
    let mut p = fixture();
    p.timeline.main[1].volume_keys = vec![key(1.0, 1.0), key(3.0, 0.4)];
    let half = |p: &Project, volume: f64| {
        apply_ops(
            p,
            &[Op::UpdateClip {
                id: "b".into(),
                patch: ClipPatch {
                    volume: Some(volume),
                    ..Default::default()
                },
            }],
        )
        .unwrap()
    };
    let down = half(&p, 0.5);
    assert_eq!(clip(&down, "b").volume, 0.5);
    assert_eq!(
        line(&clip(&down, "b").volume_keys),
        [(1.0, 0.5), (3.0, 0.2)]
    );
    // Up again within 0 to 2, and from silence every key takes the new volume.
    let up = half(&down, 2.0);
    assert_eq!(line(&clip(&up, "b").volume_keys), [(1.0, 2.0), (3.0, 0.8)]);
    p.timeline.main[1].volume = 0.0;
    let from_silence = half(&p, 0.7);
    assert_eq!(
        line(&clip(&from_silence, "b").volume_keys),
        [(1.0, 0.7), (3.0, 0.7)]
    );
    // A patch that sets the keys too sets them as given.
    let given = apply_ops(
        &fading(),
        &[Op::UpdateClip {
            id: "b".into(),
            patch: ClipPatch {
                volume: Some(0.5),
                volume_keys: Some(vec![key(1.0, 1.5)]),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&given, "b").volume_keys), [(1.0, 1.5)]);
}

#[test]
fn a_sound_layer_s_whole_volume_change_scales_its_keys() {
    let p = with_sound();
    let mut content = layer(&p, "s").content.clone();
    let ItemContent::Audio { volume, .. } = &mut content else {
        unreachable!()
    };
    *volume = 0.5;
    let down = apply_ops(
        &p,
        &[Op::UpdateItem {
            id: "s".into(),
            patch: ItemPatch {
                content: Some(content),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(sound_keys(&down, "s"), [(1.0, 0.0), (5.0, 0.5)]);
}

#[test]
fn keys_are_left_out_of_the_json_while_there_are_none() {
    let json = serde_json::to_string(&fixture().timeline.main[1]).unwrap();
    assert!(!json.contains("volumeKeys"), "{json}");
    let json = serde_json::to_string(&clip(&fading(), "b")).unwrap();
    assert!(
        json.contains("\"volumeKeys\":[{\"tUs\":2000000,\"v\":1.0}"),
        "{json}"
    );
}

#[test]
fn a_split_keeps_each_side_s_keys_with_one_at_the_cut() {
    let p = apply_ops(&fading(), &[Op::Split { at_us: secs(24.0) }]).unwrap();
    let (first, second) = (&p.timeline.main[1], &p.timeline.main[2]);
    assert_eq!(line(&first.volume_keys), [(2.0, 1.0), (4.0, 0.5)]);
    assert_eq!(line(&second.volume_keys), [(0.0, 0.5), (2.0, 0.0)]);
}

#[test]
fn trims_keep_keys_on_the_content() {
    // In at the start by 3 s: what was 3 s in is the start, held at the level there.
    let p = apply_ops(
        &fading(),
        &[Op::TrimClip {
            id: "b".into(),
            edge: Edge::Start,
            source_us: secs(3.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(0.0, 0.75), (3.0, 0.0)]);
    // Out again by 2 s: the keys move later with their footage.
    let p = apply_ops(
        &p,
        &[Op::TrimClip {
            id: "b".into(),
            edge: Edge::Start,
            source_us: secs(1.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(2.0, 0.75), (5.0, 0.0)]);
    // In at the end to 5 s: keys past it go, and one holds the level there.
    let p = apply_ops(
        &fading(),
        &[Op::TrimClip {
            id: "b".into(),
            edge: Edge::End,
            source_us: secs(5.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(2.0, 1.0), (5.0, 0.25)]);
}

#[test]
fn a_ripple_delete_keeps_each_piece_s_keys_on_its_footage() {
    let mut p = fixture();
    p.timeline.main[1].volume_keys = vec![key(1.0, 1.0), key(3.0, 0.5), key(7.0, 0.0)];
    // 2 s to 5 s into "b" goes.
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![klipolo_model::RangeUs {
                start: secs(22.0),
                end: secs(25.0),
            }],
        }],
    )
    .unwrap();
    let (first, second) = (&p.timeline.main[1], &p.timeline.main[2]);
    assert_eq!(line(&first.volume_keys), [(1.0, 1.0), (2.0, 0.75)]);
    assert_eq!(line(&second.volume_keys), [(0.0, 0.25), (2.0, 0.0)]);
}

#[test]
fn a_speed_change_keeps_keys_on_the_same_footage() {
    let p = apply_ops(
        &fading(),
        &[Op::UpdateClip {
            id: "b".into(),
            patch: ClipPatch {
                speed: Some(2.0),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(1.0, 1.0), (3.0, 0.0)]);
    let p = apply_ops(
        &fading(),
        &[Op::UpdateClip {
            id: "b".into(),
            patch: ClipPatch {
                speed: Some(0.5),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(4.0, 1.0), (12.0, 0.0)]);
}

/// "a" plays the recording 2 to 20 s with keys at 10 s and 16 s, "b" the b-roll 3 to 8 s fading out over 2 to 4 s, and
/// "c" the recording 30 to 40 s rising over its first 2 s.
fn abc() -> Project {
    let mut p = fixture();
    p.timeline.main[0].source_in_us = secs(2.0);
    p.timeline.main[0].volume_keys = vec![key(10.0, 1.0), key(16.0, 0.4)];
    p.timeline.main[1].source_in_us = secs(3.0);
    p.timeline.main[1].source_out_us = secs(8.0);
    p.timeline.main[1].volume_keys = vec![key(2.0, 1.0), key(4.0, 0.0)];
    let mut c = p.timeline.main[0].clone();
    c.id = "c".into();
    c.source_in_us = secs(30.0);
    c.source_out_us = secs(40.0);
    c.volume_keys = vec![key(0.0, 0.0), key(2.0, 1.0)];
    p.timeline.main.push(c);
    apply_ops(&p, &[]).unwrap()
}

#[test]
fn a_roll_moves_the_keys_of_both_clips_with_their_footage() {
    // The cut 1 s later: "a" ends later, "b" starts 1 s into its footage later.
    let p = apply_ops(
        &abc(),
        &[Op::Roll {
            clip_id: "a".into(),
            by_us: secs(1.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "a").volume_keys), [(10.0, 1.0), (16.0, 0.4)]);
    assert_eq!(line(&clip(&p, "b").volume_keys), [(1.0, 1.0), (3.0, 0.0)]);
    // 2 s earlier: "a" loses its end, held at the level there, and "b" starts earlier in its footage.
    let p = apply_ops(
        &abc(),
        &[Op::Roll {
            clip_id: "a".into(),
            by_us: secs(-2.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "a").volume_keys), [(10.0, 1.0), (16.0, 0.4)]);
    let p = apply_ops(
        &abc(),
        &[Op::Roll {
            clip_id: "a".into(),
            by_us: secs(-3.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "a").volume_keys), [(10.0, 1.0), (15.0, 0.5)]);
    assert_eq!(line(&clip(&p, "b").volume_keys), [(5.0, 1.0), (7.0, 0.0)]);
}

#[test]
fn a_slide_moves_the_keys_of_the_clips_either_side_and_keeps_the_slid_clip_s() {
    let p = apply_ops(
        &abc(),
        &[Op::Slide {
            clip_id: "b".into(),
            by_us: secs(1.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "a").volume_keys), [(10.0, 1.0), (16.0, 0.4)]);
    assert_eq!(line(&clip(&p, "b").volume_keys), [(2.0, 1.0), (4.0, 0.0)]);
    assert_eq!(line(&clip(&p, "c").volume_keys), [(0.0, 0.5), (1.0, 1.0)]);
}

#[test]
fn a_slip_and_a_reverse_keep_the_keys_where_they_are_in_the_clip() {
    let p = apply_ops(
        &abc(),
        &[Op::Slip {
            clip_id: "b".into(),
            by_us: secs(1.0),
        }],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(2.0, 1.0), (4.0, 0.0)]);
    let mut copy: Asset = fixture().asset("broll").unwrap().clone();
    copy.id = "broll-rev".into();
    copy.kind = AssetKind::Video;
    let p = apply_ops(
        &abc(),
        &[
            Op::AddAssets { assets: vec![copy] },
            Op::ReverseClip {
                clip_id: "b".into(),
                copy: Some(ReverseCopy {
                    asset_id: "broll-rev".into(),
                    from_us: 0,
                    to_us: secs(10.0),
                }),
            },
        ],
    )
    .unwrap();
    assert_eq!(line(&clip(&p, "b").volume_keys), [(2.0, 1.0), (4.0, 0.0)]);
}

#[test]
fn a_freeze_frame_is_silent_and_holds_no_keys() {
    let p = apply_ops(
        &fading(),
        &[Op::FreezeFrame {
            at_us: secs(24.0),
            duration_us: secs(2.0),
        }],
    )
    .unwrap();
    let main = &p.timeline.main;
    assert_eq!(line(&main[1].volume_keys), [(2.0, 1.0), (4.0, 0.5)]);
    assert!(main[2].is_freeze() && main[2].volume_keys.is_empty());
    assert_eq!(line(&main[3].volume_keys), [(0.0, 0.5), (2.0, 0.0)]);
}

/// A sound layer of the mic at 4 s, 8 s long, attached to "a", going from silent to full between 1 s and 5 s into it.
fn with_sound() -> Project {
    let mut p = fixture();
    let mut track = Track::new(TrackKind::Audio, "Sounds");
    track.id = "sounds".into();
    let mut sound = Item {
        content: ItemContent::Audio {
            asset_id: "mic".into(),
            source_in_us: secs(4.0),
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
            volume_keys: vec![key(1.0, 0.0), key(5.0, 1.0)],
        },
        ..still_item("s", 4.0, 8.0)
    };
    sound.attach = anchor_at(&p.timeline.main, sound.start_us);
    track.items.push(sound);
    p.timeline.tracks.push(track);
    apply_ops(&p, &[]).unwrap()
}

fn layer<'a>(p: &'a Project, id: &str) -> &'a Item {
    let (t, i) = find_item(p, id).unwrap_or_else(|| panic!("no layer {id}"));
    &p.timeline.tracks[t].items[i]
}

fn sound_keys(p: &Project, id: &str) -> Vec<(f64, f64)> {
    line(layer(p, id).content.volume_keys())
}

#[test]
fn a_sound_layer_s_keys_are_checked() {
    let mut p = with_sound();
    let ItemContent::Audio { volume_keys, .. } = &mut p.timeline.tracks[0].items[0].content else {
        unreachable!()
    };
    *volume_keys = vec![key(9.0, 1.0)];
    assert_eq!(
        apply_ops(&p, &[]).unwrap_err(),
        refused("A volume key is outside its clip or sound.")
    );
}

#[test]
fn a_sound_layer_s_keys_follow_its_splits_and_trims() {
    let p = apply_ops(
        &with_sound(),
        &[Op::SplitItem {
            id: "s".into(),
            at_us: secs(7.0),
        }],
    )
    .unwrap();
    let second = p.timeline.tracks[0].items[1].id.clone();
    assert_eq!(sound_keys(&p, "s"), [(1.0, 0.0), (3.0, 0.5)]);
    assert_eq!(sound_keys(&p, &second), [(0.0, 0.5), (2.0, 1.0)]);
    // In at the start by 2 s, then out again by 1 s: the keys stay on the sound.
    let p = apply_ops(
        &with_sound(),
        &[Op::TrimItem {
            id: "s".into(),
            edge: Edge::Start,
            at_us: secs(6.0),
        }],
    )
    .unwrap();
    assert_eq!(sound_keys(&p, "s"), [(0.0, 0.25), (3.0, 1.0)]);
    let p = apply_ops(
        &p,
        &[Op::TrimItem {
            id: "s".into(),
            edge: Edge::Start,
            at_us: secs(5.0),
        }],
    )
    .unwrap();
    assert_eq!(sound_keys(&p, "s"), [(1.0, 0.25), (4.0, 1.0)]);
    let p = apply_ops(
        &with_sound(),
        &[Op::TrimItem {
            id: "s".into(),
            edge: Edge::End,
            at_us: secs(7.0),
        }],
    )
    .unwrap();
    assert_eq!(sound_keys(&p, "s"), [(1.0, 0.0), (3.0, 0.5)]);
    // A shorter length set by a patch fits them too.
    let p = apply_ops(
        &with_sound(),
        &[Op::UpdateItem {
            id: "s".into(),
            patch: ItemPatch {
                duration_us: Some(secs(3.0)),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(sound_keys(&p, "s"), [(1.0, 0.0), (3.0, 0.5)]);
}

#[test]
fn a_cut_under_a_sound_layer_keeps_each_piece_s_keys_on_its_sound() {
    // 6 s to 8 s goes: 2 s to 4 s into the sound.
    let p = apply_ops(
        &with_sound(),
        &[Op::DeleteRanges {
            ranges: vec![klipolo_model::RangeUs {
                start: secs(6.0),
                end: secs(8.0),
            }],
        }],
    )
    .unwrap();
    let items = &p.timeline.tracks[0].items;
    assert_eq!(items.len(), 2);
    assert_eq!(
        line(items[0].content.volume_keys()),
        [(1.0, 0.0), (2.0, 0.25)]
    );
    assert_eq!(
        line(items[1].content.volume_keys()),
        [(0.0, 0.75), (1.0, 1.0)]
    );
}

#[test]
fn a_separated_sound_takes_its_clip_s_keys() {
    let p = apply_ops(
        &fading(),
        &[Op::DetachSound {
            clip_id: "b".into(),
        }],
    )
    .unwrap();
    assert_eq!(sound_keys(&p, "b-sound"), [(2.0, 1.0), (6.0, 0.0)]);
}

#[test]
fn a_separated_recording_source_takes_its_clip_s_keys_at_its_mix_s_volume() {
    let mut p = fixture();
    p.timeline.main[0].volume_keys = vec![key(0.0, 1.0), key(10.0, 0.0)];
    p.recordings[0].sources[2].mix.volume = 0.5;
    let p = apply_ops(
        &p,
        &[Op::DetachSource {
            clip_id: "a".into(),
            source_id: "mic".into(),
        }],
    )
    .unwrap();
    assert_eq!(sound_keys(&p, "a-mic"), [(0.0, 0.5), (10.0, 0.0)]);
}

/// The page draws keys with its own evaluator (`keys.ts` in `apps/desktop/src/lib`): the canvas places a keyed layer and
/// the timeline draws a volume line where the engine plays them. `apps/desktop/src/lib/keys-parity.json`, written here
/// with `UPDATE_GOLDEN=1`, pins it to `motion::value_at` for every ease, at eleven points between two keys.
#[test]
fn keys_parity_json_is_current() {
    let eases: Vec<Ease> = [
        "linear",
        "ease",
        "easeIn",
        "easeOut",
        "easeInOut",
        "power1In",
        "power1Out",
        "power1InOut",
        "power2In",
        "power2Out",
        "power2InOut",
        "power3In",
        "power3Out",
        "power3InOut",
        "power4In",
        "power4Out",
        "power4InOut",
        "backIn",
        "backOut",
        "backInOut",
        "elasticIn",
        "elasticOut",
        "elasticInOut",
        "bounceIn",
        "bounceOut",
        "bounceInOut",
        "sineIn",
        "sineOut",
        "sineInOut",
        "expoIn",
        "expoOut",
        "expoInOut",
        "circIn",
        "circOut",
        "circInOut",
        "hold",
    ]
    .iter()
    .map(|name| serde_json::from_value(serde_json::json!(name)).unwrap())
    .chain([
        Ease::CubicBezier {
            x1: 0.3,
            y1: -0.5,
            x2: 0.6,
            y2: 1.8,
        },
        Ease::Steps { n: 5 },
        Ease::Spring {
            stiffness: 100.0,
            damping: 10.0,
            mass: 1.0,
        },
        Ease::Spring {
            stiffness: 50.0,
            damping: 40.0,
            mass: 2.0,
        },
    ])
    .collect();
    let cases: Vec<serde_json::Value> = eases
        .into_iter()
        .map(|ease| {
            let keys = vec![
                key(1.0, 0.25),
                Key {
                    ease: Some(ease),
                    ..key(3.0, 1.75)
                },
            ];
            let at: Vec<i64> = (0..=10).map(|i| secs(1.0) + i * 200_000).collect();
            let values: Vec<f64> = at
                .iter()
                .map(|&t| klipolo_model::motion::value_at(&keys, t, 0.0))
                .collect();
            serde_json::json!({ "keys": keys, "at": at, "values": values })
        })
        .collect();
    assert_golden(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/lib/keys-parity.json"),
        &sorted_keys(serde_json::json!({ "cases": cases })),
    );
}
