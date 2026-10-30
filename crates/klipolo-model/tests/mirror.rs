//! Mirrored video layers: separating a camera keeps its bubble's mirror on every run's layer, a
//! video layer's mirror is in the JSON only while on, and splitting a mirrored layer keeps it on both pieces.

mod common;
use common::*;
use klipolo_model::ops::Op;
use klipolo_model::project::SourceRun;
use klipolo_model::timeline::ItemContent;
use klipolo_model::{apply_ops, Project};
use serde_json::json;

/// Each video layer's mirror, by start.
fn mirrors(p: &Project) -> Vec<bool> {
    let mut layers: Vec<_> = (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .filter_map(|i| match i.content {
            ItemContent::Video { mirror, .. } => Some((i.start_us, mirror)),
            _ => None,
        })
        .collect();
    layers.sort_unstable_by_key(|&(start, _)| start);
    layers.into_iter().map(|(_, mirror)| mirror).collect()
}

/// The fixture's camera, which reconnected once (two runs), separated from clip `a` with its bubble mirrored or not.
fn separated(mirror: bool) -> Project {
    let mut p = fixture();
    p.timeline.main[0].camera.as_mut().unwrap().mirror = mirror;
    let run = |asset: &str, offset_us, duration_us| SourceRun {
        asset_id: asset.into(),
        offset_us,
        duration_us,
        drift_ppm: 0.0,
        file_in_us: 0,
    };
    // The reconnect started a file of its own, as the recorder writes it.
    second_file(&mut p, "camera", "camera-2");
    p.recordings[0].sources[1].runs = vec![
        run("camera", 200_000, secs(9.6)),
        run("camera-2", secs(12.0), secs(47.8)),
    ];
    let detach = Op::DetachSource {
        clip_id: "a".into(),
        source_id: "camera".into(),
    };
    apply_ops(&p, &[detach]).unwrap()
}

#[test]
fn a_separated_camera_keeps_its_bubble_s_mirror_on_every_run() {
    assert_eq!(mirrors(&separated(true)), [true, true]);
    assert_eq!(mirrors(&separated(false)), [false, false]);
}

#[test]
fn a_video_layer_s_mirror_is_in_the_json_only_while_on() {
    let plain = json!({"type": "video", "assetId": "v", "sourceInUs": 0, "speed": 1.0, "volume": 1.0, "muted": false});
    let read: ItemContent = serde_json::from_value(plain.clone()).unwrap();
    assert!(matches!(read, ItemContent::Video { mirror: false, .. }));
    assert_eq!(
        serde_json::to_value(&read).unwrap(),
        plain,
        "no key while off"
    );
    let mut mirrored = plain;
    mirrored["mirror"] = true.into();
    let read: ItemContent = serde_json::from_value(mirrored.clone()).unwrap();
    assert_eq!(serde_json::to_value(&read).unwrap(), mirrored);
}

#[test]
fn splitting_a_mirrored_layer_keeps_it_on_both_pieces() {
    let split = Op::SplitItem {
        id: "a-camera".into(),
        at_us: secs(5.0),
    };
    let p = apply_ops(&separated(true), &[split]).unwrap();
    assert_eq!(mirrors(&p), [true, true, true]);
}
