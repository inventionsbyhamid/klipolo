//! A main clip's crop: the edges cut off its footage before it is framed.

mod common;

use common::*;
use klipolo_model::crop::Crop;
use klipolo_model::ops::ClipPatch;
use klipolo_model::project::Project;
use klipolo_model::{apply_ops, Op};

fn crop(left: f64, top: f64, right: f64, bottom: f64) -> Crop {
    Crop {
        left,
        top,
        right,
        bottom,
    }
}

fn crop_clip(id: &str, c: Crop) -> Op {
    Op::UpdateClip {
        id: id.into(),
        patch: ClipPatch {
            crop: Some(c),
            ..Default::default()
        },
    }
}

#[test]
fn a_clip_keeps_its_crop_and_a_crop_of_nothing_clears_it() {
    let p = fixture();
    let c = crop(0.1, 0.0, 0.2, 0.05);
    let next = apply_ops(&p, &[crop_clip("b", c)]).unwrap();
    assert_eq!(next.timeline.main[1].crop, Some(c));
    // A recording's clip crops its screen the same way.
    let next = apply_ops(&next, &[crop_clip("a", c)]).unwrap();
    assert_eq!(next.timeline.main[0].crop, Some(c));
    let cleared = apply_ops(&next, &[crop_clip("b", Crop::NONE)]).unwrap();
    assert_eq!(cleared.timeline.main[1].crop, None);
}

#[test]
fn a_clip_crop_leaves_a_tenth_of_the_picture_each_way() {
    let p = fixture();
    let err = apply_ops(&p, &[crop_clip("b", crop(0.5, 0.0, 0.45, 0.0))]).unwrap_err();
    assert_eq!(
        err,
        refused("A crop must leave at least a tenth of the picture each way.")
    );
    assert!(apply_ops(&p, &[crop_clip("b", crop(0.0, -0.1, 0.0, 0.0))]).is_err());
    assert!(apply_ops(&p, &[crop_clip("b", crop(0.0, 0.0, f64::NAN, 0.0))]).is_err());
}

#[test]
fn a_split_clip_keeps_its_crop_on_both_pieces() {
    let p = fixture();
    let c = crop(0.0, 0.1, 0.0, 0.1);
    let next = apply_ops(
        &p,
        &[
            crop_clip("b", c),
            Op::Split {
                at_us: secs(25.0),
            },
        ],
    )
    .unwrap();
    assert_eq!(next.timeline.main.len(), 3);
    assert!(next.timeline.main[1..].iter().all(|clip| clip.crop == Some(c)));
}

#[test]
fn a_clip_without_a_crop_leaves_it_out_of_the_file() {
    let p = fixture();
    let json = serde_json::to_value(&p).unwrap();
    assert!(json["timeline"]["main"][1].get("crop").is_none());
    let cropped = apply_ops(&p, &[crop_clip("b", crop(0.1, 0.0, 0.0, 0.0))]).unwrap();
    let json = serde_json::to_value(&cropped).unwrap();
    assert_eq!(
        json["timeline"]["main"][1]["crop"],
        serde_json::json!({"left": 0.1, "top": 0.0, "right": 0.0, "bottom": 0.0})
    );
    let back: Project = serde_json::from_value(json).unwrap();
    assert_eq!(back, cropped);
}
