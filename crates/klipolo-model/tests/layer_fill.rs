//! A video layer's picture inside its box: contained or covering it (Fill), with its edges cropped.

mod common;

use common::*;
use klipolo_model::crop::Crop;
use klipolo_model::ops::ItemPatch;
use klipolo_model::project::Project;
use klipolo_model::timeline::ItemContent;
use klipolo_model::{apply_ops, Op};

fn crop(left: f64, top: f64, right: f64, bottom: f64) -> Crop {
    Crop {
        left,
        top,
        right,
        bottom,
    }
}

/// `p` with video layer "v" over the main track.
fn with_layer(p: Project) -> Project {
    let mut p = p;
    p.timeline.tracks = vec![overlay_track("o", vec![video_item("v", 0.0, 5.0, 0.0)])];
    p
}

fn set_video(p: &Project, fill: bool, crop: Option<Crop>) -> Result<Project, klipolo_model::ModelError> {
    let mut content = p.timeline.tracks[0].items[0].content.clone();
    if let ItemContent::Video {
        fill: f, crop: c, ..
    } = &mut content
    {
        *f = fill;
        *c = crop;
    }
    apply_ops(
        p,
        &[Op::UpdateItem {
            id: "v".into(),
            patch: ItemPatch {
                content: Some(content),
                ..Default::default()
            },
        }],
    )
}

fn video_of(p: &Project) -> (bool, Option<Crop>) {
    match &p.timeline.tracks[0].items[0].content {
        ItemContent::Video { fill, crop, .. } => (*fill, *crop),
        other => panic!("not a video layer: {other:?}"),
    }
}

#[test]
fn a_video_layer_fills_its_box_or_crops_inside_it() {
    let p = with_layer(fixture());
    assert_eq!(video_of(&p), (false, None));
    let c = crop(0.2, 0.0, 0.2, 0.0);
    let next = set_video(&p, true, Some(c)).unwrap();
    assert_eq!(video_of(&next), (true, Some(c)));
    let err = set_video(&p, false, Some(crop(0.0, 0.6, 0.0, 0.35))).unwrap_err();
    assert_eq!(
        err,
        refused("A layer's crop must leave at least a tenth of its picture each way.")
    );
}

#[test]
fn a_contained_uncropped_video_layer_leaves_both_out_of_the_file() {
    let p = with_layer(fixture());
    let json = serde_json::to_value(&p).unwrap();
    let content = &json["timeline"]["tracks"][0]["items"][0]["content"];
    assert!(content.get("fill").is_none() && content.get("crop").is_none());
    let filled = set_video(&p, true, None).unwrap();
    let json = serde_json::to_value(&filled).unwrap();
    assert_eq!(json["timeline"]["tracks"][0]["items"][0]["content"]["fill"], true);
    let back: Project = serde_json::from_value(json).unwrap();
    assert_eq!(back, filled);
}
