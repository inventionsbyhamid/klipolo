//! Generator layers' pieces keep their place in the contribution's time: a split continues the scene, trims
//! of a split piece move its exit with its end and keep its content where it was, and trims of an unsplit layer keep
//! the entrance and exit at its ends.
mod common;
use common::*;
use klipolo_model::ops::{Edge, Op};
use klipolo_model::timeline::{piece_time, seed_id, ItemContent, PieceTime};

fn scene_layer(id: &str, start_s: f64, len_s: f64) -> klipolo_model::timeline::Item {
    item(
        id,
        start_s,
        len_s,
        full(),
        ItemContent::generator(
            "lib.k3f9q2ma/scene.lower-third.v1".into(),
            Default::default(),
        ),
    )
}

#[test]
fn split_pieces_continue_the_scene() {
    let mut p = project_with_overlay(vec![scene_layer("lt", 0.0, 6.0)]);
    apply(
        &mut p,
        Op::SplitItem {
            id: "lt".into(),
            at_us: secs(2.0),
        },
    );
    let (a, b) = (&overlay_items(&p)[0], &overlay_items(&p)[1]);
    assert_eq!(
        piece_time(a),
        Some(PieceTime {
            offset_us: 0,
            span_us: secs(6.0)
        })
    );
    assert_eq!(
        piece_time(b),
        Some(PieceTime {
            offset_us: secs(2.0),
            span_us: secs(6.0)
        })
    );
    // The second piece's end trimmed by 1 s: its exit moves with its end.
    let b_id = b.id.clone();
    apply(
        &mut p,
        Op::TrimItem {
            id: b_id.clone(),
            edge: Edge::End,
            at_us: secs(5.0),
        },
    );
    assert_eq!(
        piece_time(item_by_id(&p, &b_id)),
        Some(PieceTime {
            offset_us: secs(2.0),
            span_us: secs(5.0)
        })
    );
    // Its start trimmed by 0.5 s: the content stays where it was.
    apply(
        &mut p,
        Op::TrimItem {
            id: b_id.clone(),
            edge: Edge::Start,
            at_us: secs(2.5),
        },
    );
    assert_eq!(
        piece_time(item_by_id(&p, &b_id)),
        Some(PieceTime {
            offset_us: secs(2.5),
            span_us: secs(5.0)
        })
    );
}

#[test]
fn trims_of_an_unsplit_layer_keep_the_entrance_and_exit_at_its_ends() {
    let mut p = project_with_overlay(vec![scene_layer("lt", 1.0, 4.0)]);
    apply(
        &mut p,
        Op::TrimItem {
            id: "lt".into(),
            edge: Edge::Start,
            at_us: secs(2.0),
        },
    );
    apply(
        &mut p,
        Op::TrimItem {
            id: "lt".into(),
            edge: Edge::End,
            at_us: secs(4.0),
        },
    );
    let it = item_by_id(&p, "lt");
    assert_eq!(
        piece_time(it),
        Some(PieceTime {
            offset_us: 0,
            span_us: secs(2.0)
        })
    );
    let ItemContent::Generator {
        offset_us, span_us, ..
    } = &it.content
    else {
        panic!()
    };
    assert_eq!((*offset_us, *span_us), (0, None));
}

#[test]
fn a_piece_time_outside_its_span_is_refused() {
    let mut p = project_with_overlay(vec![scene_layer("lt", 0.0, 4.0)]);
    let mut bad = item_by_id(&p, "lt").clone();
    if let ItemContent::Generator {
        offset_us, span_us, ..
    } = &mut bad.content
    {
        *offset_us = secs(3.0);
        *span_us = Some(secs(5.0)); // 3 + 4 > 5
    }
    let err = try_apply(
        &mut p,
        Op::UpdateItem {
            id: "lt".into(),
            patch: content_patch(bad.content),
        },
    )
    .unwrap_err();
    assert_eq!(err.coded().code, "model.layer.generatorTime");
}

/// With no span the layer is the whole animation, so it starts at its beginning.
#[test]
fn an_offset_without_a_span_is_refused() {
    let mut p = project_with_overlay(vec![scene_layer("lt", 0.0, 4.0)]);
    let mut bad = item_by_id(&p, "lt").clone();
    if let ItemContent::Generator { offset_us, .. } = &mut bad.content {
        *offset_us = secs(1.0);
    }
    let err = try_apply(
        &mut p,
        Op::UpdateItem {
            id: "lt".into(),
            patch: content_patch(bad.content),
        },
    )
    .unwrap_err();
    assert_eq!(err.coded().code, "model.layer.generatorTime");
}

/// A split piece made longer or shorter by its length (the inspector, chat) moves its exit with its end, as an end
/// trim does; a piece cut from the middle of the main track keeps its entrance and exit, its span shorter by what
/// went.
#[test]
fn a_new_length_and_a_ripple_cut_keep_a_piece_inside_its_span() {
    let mut p = project_with_overlay(vec![scene_layer("lt", 0.0, 6.0)]);
    apply(
        &mut p,
        Op::SplitItem {
            id: "lt".into(),
            at_us: secs(2.0),
        },
    );
    let b_id = overlay_items(&p)[1].id.clone();
    apply(
        &mut p,
        Op::UpdateItem {
            id: b_id.clone(),
            patch: klipolo_model::ops::ItemPatch {
                duration_us: Some(secs(5.0)),
                ..Default::default()
            },
        },
    );
    assert_eq!(
        piece_time(item_by_id(&p, &b_id)),
        Some(PieceTime {
            offset_us: secs(2.0),
            span_us: secs(7.0)
        })
    );
    // 1 s cut from the middle of the second piece (now 2–7 s), attached so it ripples with the main track.
    apply(
        &mut p,
        Op::SetAttach {
            id: b_id.clone(),
            attached: true,
        },
    );
    apply(
        &mut p,
        Op::DeleteRanges {
            ranges: vec![klipolo_model::time::RangeUs {
                start: secs(4.0),
                end: secs(5.0),
            }],
        },
    );
    let b = item_by_id(&p, &b_id);
    assert_eq!(b.duration_us, secs(4.0));
    assert_eq!(
        piece_time(b),
        Some(PieceTime {
            offset_us: secs(2.0),
            span_us: secs(6.0)
        })
    );
}

/// A split's pieces keep the whole layer's id to seed their effects from (`kl.seed`), through
/// later splits too, so random patterns continue across every cut; a layer never split seeds from its own id.
#[test]
fn split_pieces_keep_the_whole_layers_seed() {
    let mut p = project_with_overlay(vec![scene_layer("lt", 0.0, 6.0)]);
    apply(
        &mut p,
        Op::SplitItem {
            id: "lt".into(),
            at_us: secs(2.0),
        },
    );
    let second = overlay_items(&p)[1].id.clone();
    assert_ne!(second, "lt");
    apply(
        &mut p,
        Op::SplitItem {
            id: second,
            at_us: secs(4.0),
        },
    );
    let items = overlay_items(&p);
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| seed_id(i) == "lt"), "{items:?}");
    assert_eq!(seed_id(&scene_layer("other", 0.0, 1.0)), "other");
}
