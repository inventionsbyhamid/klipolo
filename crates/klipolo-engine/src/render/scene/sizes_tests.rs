//! `sizes.rs`'s tests.

use super::*;

/// A box's class is what it spans from its even top left, in whole 16s; an empty one takes the least.
#[test]
fn boxes_take_whole_classes_from_their_even_corner() {
    assert_eq!(class_of([3.0, 0.0, 20.0, 16.0]), [32, 16]);
    assert_eq!(class_of([2.0, 0.5, 18.0, 16.2]), [16, 32]);
    assert_eq!(class_of(super::super::bounds::EMPTY), [16, 16]);
}

/// The scene's own target is its box's class, placed as a node's, and keeps the largest class it needed.
#[test]
fn the_scenes_own_target_is_its_boxs_class() {
    let mut sizes = Sizes::default();
    let size = (1920, 1080);
    let lower = sizes.place_root([75.0, 668.0, 1229.0, 951.0], size, true);
    assert_eq!(lower.origin, [74, 668]);
    assert_eq!(lower.size, (1168, 288));
    let smaller = sizes.place_root([75.0, 700.0, 600.0, 760.0], size, true);
    assert_eq!(smaller.size, (1168, 288), "classes only grow");
    let edge = sizes.place_root([1500.0, 1000.0, 1920.0, 1080.0], size, true);
    assert_eq!(edge.origin, [752, 792], "moved inside the layer, not cut");
}

/// Before its survey, a scene's own target that outgrows its first class takes the layer's whole
/// target (one target more, not one each time it grows); surveyed, it grows by classes; another size starts again.
#[test]
fn before_its_survey_a_growing_scene_takes_the_whole_layer_once() {
    let mut sizes = Sizes::default();
    let size = (1920, 1080);
    let first = sizes.place_root([100.0, 100.0, 300.0, 200.0], size, false);
    assert_eq!(first.size, (208, 112));
    let inside = sizes.place_root([110.0, 110.0, 290.0, 190.0], size, false);
    assert_eq!(inside.size, (208, 112), "within its first class: kept");
    let grows = sizes.place_root([100.0, 100.0, 340.0, 200.0], size, false);
    assert_eq!(
        (grows.origin, grows.size),
        ([0, 0], size),
        "outgrown: the whole layer"
    );
    sizes.reset();
    sizes.mark_surveyed(size, 0);
    sizes.place_root([100.0, 100.0, 300.0, 200.0], size, true);
    let surveyed = sizes.place_root([100.0, 100.0, 340.0, 200.0], size, true);
    assert_eq!(surveyed.size, (240, 112), "surveyed: a class's growth");
}
