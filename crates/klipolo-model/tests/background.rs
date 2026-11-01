//! The project's background behind the footage.

mod common;

use common::*;
use klipolo_model::project::{Background, BackgroundKind};
use klipolo_model::{apply_ops, Op};

#[test]
fn the_blurred_video_background_needs_no_picture() {
    let p = fixture();
    let background = Background {
        kind: BackgroundKind::Blur,
        padding: 0.1,
        ..Background::default()
    };
    let next = apply_ops(
        &p,
        &[Op::SetBackground {
            background: background.clone(),
        }],
    )
    .unwrap();
    assert_eq!(next.background, background);
    let json = serde_json::to_value(&next).unwrap();
    assert_eq!(json["background"]["kind"], "blur");
}
