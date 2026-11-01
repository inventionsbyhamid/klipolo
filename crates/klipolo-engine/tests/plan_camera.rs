//! A recording's camera in the frame plan as the clip card, the Camera card and the canvas set it: its layout in each
//! format (Bubble, Split or Hidden, each format's own), the bubble moved in one format only, its shape, border and
//! mirror, and a separated camera's mirror, which its layer's card sets.

mod support;
use klipolo_engine::plan::framing::FULL;
use klipolo_engine::plan::{VideoLayer, VideoRole, frame_plan};
use klipolo_model::Project;
use klipolo_model::layout::{BoxN, resolve_rect};
use klipolo_model::ops::{ClipPatch, ItemPatch, Op};
use klipolo_model::project::FormatKind::{self, Landscape, Portrait, Square};
use klipolo_model::timeline::{CameraLayout, CameraMode, CameraShape, ItemContent};
use support::*;

/// The default bubble's box in the main format (16:9).
const BUBBLE: BoxN = BoxN {
    x: 0.76,
    y: 0.6044,
    w: 0.2,
    h: 0.3556,
};
/// The split's camera: the bottom half.
const SPLIT: BoxN = BoxN {
    x: 0.0,
    y: 0.5,
    w: 1.0,
    h: 0.5,
};

/// `p` with clip a's camera set by `edit`, as the page sends it: one `UpdateClip` carrying the whole camera.
fn edited(p: &Project, edit: impl FnOnce(&mut CameraLayout)) -> Project {
    let mut camera = p.timeline.main[0].camera.clone().unwrap();
    edit(&mut camera);
    let op = Op::UpdateClip {
        id: "a".into(),
        patch: ClipPatch {
            camera: Some(camera),
            ..ClipPatch::default()
        },
    };
    klipolo_model::apply_ops(p, &[op]).unwrap()
}

/// What clip a draws at 2 s in `format`: its screen, and its camera if it draws one.
fn drawn(p: &Project, format: FormatKind) -> (VideoLayer, Option<VideoLayer>) {
    let plan = frame_plan(&ctx(p.clone()), secs(2.0), format);
    let v = videos(&plan);
    let screen = v
        .iter()
        .find(|v| v.role == VideoRole::Main)
        .copied()
        .cloned();
    let camera = v
        .iter()
        .find(|v| v.role == VideoRole::Camera)
        .copied()
        .cloned();
    (screen.expect("the screen is drawn"), camera)
}

fn camera(p: &Project, format: FormatKind) -> Option<VideoLayer> {
    drawn(p, format).1
}

#[test]
fn each_format_draws_the_layout_set_for_it_alone() {
    let p = project();
    // As recorded: a bubble in 16:9 and 1:1, the split in 9:16.
    close(camera(&p, Landscape).unwrap().dest, BUBBLE);
    close(camera(&p, Portrait).unwrap().dest, SPLIT);
    close(
        camera(&p, Square).unwrap().dest,
        resolve_rect(
            &p.timeline.main[0].camera.as_ref().unwrap().bubble,
            Landscape,
            Square,
        ),
    );

    // Hidden in 9:16: only the screen there, over the whole frame; 16:9 and 1:1 keep their bubble.
    let hidden = edited(&p, |c| {
        c.mode_overrides.insert(Portrait, CameraMode::Hidden);
    });
    let (screen, none) = drawn(&hidden, Portrait);
    assert!(none.is_none());
    // Fitted in the middle of the frame, not the top pane: 1080×607.5 at y = 656.25 px.
    close(screen.dest, bx(0.0, 0.341796875, 1.0, 0.31640625));
    close(camera(&hidden, Landscape).unwrap().dest, BUBBLE);
    assert!(camera(&hidden, Square).is_some());

    // A bubble in 9:16, then hidden in 16:9: each format as set, the base mode never changed.
    let bubble = edited(&hidden, |c| {
        c.mode_overrides.insert(Portrait, CameraMode::Bubble);
    });
    let mixed = edited(&bubble, |c| {
        c.mode_overrides.insert(Landscape, CameraMode::Hidden);
    });
    assert_eq!(
        mixed.timeline.main[0].camera.as_ref().unwrap().mode,
        CameraMode::Bubble
    );
    let (screen, none) = drawn(&mixed, Landscape);
    assert!(none.is_none());
    close(screen.dest, FULL);
    let portrait = camera(&mixed, Portrait).unwrap();
    assert!(
        portrait.dest.w < 1.0 && portrait.dest.y > 0.5,
        "a bubble, not the split"
    );
    assert_eq!((portrait.radius, portrait.border), (0.5, true));
    assert!(camera(&mixed, Square).is_some());
}

#[test]
fn a_bubble_moved_in_one_format_moves_there_only() {
    let p = project();
    let moved = BoxN {
        x: 0.1,
        y: 0.1,
        w: 0.3,
        h: 0.3,
    };
    // A drag in 1:1 sets that format's own box.
    let square = edited(&p, |c| {
        c.bubble.overrides.insert(Square, moved);
    });
    close(camera(&square, Square).unwrap().dest, moved);
    close(camera(&square, Landscape).unwrap().dest, BUBBLE);
    close(camera(&square, Portrait).unwrap().dest, SPLIT);

    // A drag in 16:9 (the main format) moves its box; 1:1 keeps the box it has of its own.
    let main = BoxN {
        x: 0.04,
        y: 0.6044,
        w: 0.2,
        h: 0.3556,
    };
    let both = edited(&square, |c| c.bubble.rect = main);
    close(camera(&both, Landscape).unwrap().dest, main);
    close(camera(&both, Square).unwrap().dest, moved);
}

#[test]
fn shape_and_border_draw_on_the_bubble_and_mirror_wherever_it_shows() {
    let p = project();
    for (shape, radius) in [
        (CameraShape::Circle, 0.5),
        (CameraShape::Rounded, 0.16),
        (CameraShape::Square, 0.0),
    ] {
        let shaped = edited(&p, |c| c.shape = shape);
        assert_eq!(camera(&shaped, Landscape).unwrap().radius, radius);
        // The split is square-cornered and has no border, whatever the bubble's.
        let split = camera(&shaped, Portrait).unwrap();
        assert_eq!((split.radius, split.border), (0.0, false));
    }
    let bare = edited(&p, |c| c.border = false);
    assert!(!camera(&bare, Landscape).unwrap().border);
    for on in [true, false] {
        let mirrored = edited(&p, |c| c.mirror = on);
        assert_eq!(camera(&mirrored, Landscape).unwrap().mirror, on);
        assert_eq!(camera(&mirrored, Portrait).unwrap().mirror, on);
    }
}

/// The overlay video that plays the separated camera at 2 s in `format`.
fn layer_in(p: &Project, format: FormatKind) -> VideoLayer {
    let plan = frame_plan(&ctx(p.clone()), secs(2.0), format);
    (videos(&plan).into_iter())
        .find(|v| v.role == VideoRole::Overlay && v.source.asset_id.as_ref() == "camera")
        .cloned()
        .expect("the camera's layer is drawn")
}

#[test]
fn a_separated_camera_is_drawn_by_its_layer_which_mirrors_from_its_card() {
    let separate = Op::DetachSource {
        clip_id: "a".into(),
        source_id: "camera".into(),
    };
    let p = klipolo_model::apply_ops(&project(), &[separate]).unwrap();
    // The clip draws no camera of its own in any format; its layer does, mirrored as the bubble was (by default).
    for format in [Landscape, Portrait, Square] {
        assert!(camera(&p, format).is_none());
        assert!(layer_in(&p, format).mirror);
    }
    // The layer card's Mirror: Off, sent as the layer's whole content.
    let item = (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| i.id == "a-camera")
        .unwrap();
    let mut content = item.content.clone();
    if let ItemContent::Video { mirror, .. } = &mut content {
        *mirror = false;
    }
    let off = Op::UpdateItem {
        id: "a-camera".into(),
        patch: ItemPatch {
            content: Some(content),
            ..ItemPatch::default()
        },
    };
    let p = klipolo_model::apply_ops(&p, &[off]).unwrap();
    assert!(!layer_in(&p, Landscape).mirror);
}
