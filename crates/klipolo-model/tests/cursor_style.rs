mod common;
use common::*;
use klipolo_model::cursor_style::*;
use klipolo_model::history::Document;
use klipolo_model::ops::{op_risk, CursorTarget, Op, OpRisk};
use klipolo_model::{apply_ops, Transaction};

fn tx(ops: Vec<Op>) -> Transaction {
    Transaction {
        label: "edit.cursorStyle".into(),
        params: None,
        ops,
        origin: None,
    }
}

#[test]
fn a_clip_without_its_own_style_uses_the_projects() {
    let p = fixture(); // the recording fixture tests/common already has
    let clip = &p.timeline.main[0];
    assert_eq!(p.cursor_for(clip), &p.cursor);
    assert_eq!(p.cursor.size, 1.25);
}

#[test]
fn styling_clips_then_clearing_them_restores_the_project_style_one_undo_each() {
    let mut doc = Document::new(fixture()).unwrap();
    let small = CursorStyle {
        size: 0.75,
        ..CursorStyle::default()
    };
    let ids = vec![doc.project().timeline.main[0].id.clone()];
    doc.apply(&tx(vec![Op::SetCursorStyle {
        target: CursorTarget::Clips { ids: ids.clone() },
        style: Some(small),
    }]))
    .unwrap();
    assert_eq!(doc.project().timeline.main[0].cursor.as_ref(), Some(&small));
    doc.apply(&tx(vec![Op::SetCursorStyle {
        target: CursorTarget::Clips { ids },
        style: None,
    }]))
    .unwrap();
    assert!(doc.project().timeline.main[0].cursor.is_none());
    doc.undo().unwrap();
    assert_eq!(doc.project().timeline.main[0].cursor.as_ref(), Some(&small));
}

#[test]
fn undo_restores_the_project_cursor_style_and_the_patch_carries_it() {
    let mut doc = Document::new(fixture()).unwrap();
    let big = CursorStyle {
        size: 2.0,
        ..CursorStyle::default()
    };
    let change = doc
        .apply_change(&tx(vec![Op::SetCursorStyle {
            target: CursorTarget::Project,
            style: Some(big),
        }]))
        .unwrap();
    assert_eq!(change.patch.cursor, Some(big), "the UI's copy hears of it");
    doc.undo().unwrap();
    assert_eq!(doc.project().cursor, CursorStyle::default());
}

#[test]
fn as_recorded_draws_size_one_on_the_raw_path_and_keeps_the_other_settings() {
    let s = CursorStyle {
        as_recorded: true,
        size: 3.0,
        smoothing: Smoothing::Strong,
        ..CursorStyle::default()
    };
    let e = s.effective();
    assert_eq!(
        (e.size, e.smoothing, e.click, e.motion_blur, e.look),
        (
            1.0,
            Smoothing::Off,
            ClickEffect::None,
            0.0,
            CursorLook::Recorded
        )
    );
    assert_eq!(s.size, 3.0, "kept for when it's turned off");
}

#[test]
fn out_of_range_styles_and_a_project_without_a_style_are_refused_with_codes() {
    let mut p = fixture();
    let bad = CursorStyle {
        size: 9.0,
        ..CursorStyle::default()
    };
    let e = apply_ops(
        &p,
        &[Op::SetCursorStyle {
            target: CursorTarget::Project,
            style: Some(bad),
        }],
    )
    .unwrap_err();
    assert_eq!(e.coded().code, "model.cursor.size");
    let e = apply_ops(
        &p,
        &[Op::SetCursorStyle {
            target: CursorTarget::Project,
            style: None,
        }],
    )
    .unwrap_err();
    assert_eq!(e.coded().code, "model.cursor.projectNeedsStyle");
    p.cursor.hide_when_still_us = Some(100);
    assert_eq!(
        p.cursor.check().unwrap_err().coded().code,
        "model.cursor.hide"
    );
}

#[test]
fn burned_from_says_where_the_frames_show_the_cursor() {
    let c = CursorCapture::BurnedFrom { at_us: 5_000_000 };
    assert!(c.separate_at(4_999_999) && !c.separate_at(5_000_000));
    assert!(!CursorCapture::Burned.separate_at(0) && CursorCapture::Separate.separate_at(i64::MAX));
}

#[test]
fn portrait_screen_takes_with_a_cursor_fill_and_follow_by_default() {
    use klipolo_model::framing::default_framing;
    use klipolo_model::project::FormatKind;
    let p = fixture_with_cursor(); // tests/common: the fixture with `cursor: Some(..)` on its recording
    let clip = &p.timeline.main[0];
    assert!(default_framing(&p, clip, FormatKind::Portrait)
        .follow
        .is_some());
    assert!(default_framing(&p, clip, FormatKind::Landscape)
        .follow
        .is_none());
    assert!(
        default_framing(
            &fixture(),
            &fixture().timeline.main[0],
            FormatKind::Portrait
        )
        .follow
        .is_none(),
        "no cursor: no follow"
    );
}

#[test]
fn an_auto_zoom_without_using_keeps_its_old_json_and_styling_is_additive() {
    let auto: klipolo_model::timeline::ZoomTarget =
        serde_json::from_str(r#"{"type":"auto"}"#).unwrap();
    assert_eq!(serde_json::to_string(&auto).unwrap(), r#"{"type":"auto"}"#);
    assert_eq!(
        op_risk(&Op::SetCursorStyle {
            target: CursorTarget::Project,
            style: Some(CursorStyle::default())
        }),
        OpRisk::Additive
    );
}

#[test]
fn a_project_saved_before_the_cursor_layer_keeps_its_cursor_file_and_saves_the_new_shape() {
    // An early version wrote the take's cursor file as `cursorPath`: it reads as a burned-in take.
    let mut earlier = serde_json::to_value(fixture()).unwrap();
    earlier["recordings"][0]["cursorPath"] = "/media/rec/cursor.jsonl".into();
    let p = klipolo_model::load_project(&earlier.to_string()).unwrap();
    assert_eq!(
        p.recordings[0].cursor,
        Some(RecordingCursor::burned_in("/media/rec/cursor.jsonl"))
    );
    let saved = serde_json::to_value(&p).unwrap();
    assert_eq!(
        saved["recordings"][0]["cursor"]["path"],
        "/media/rec/cursor.jsonl"
    );
    assert!(saved["recordings"][0].get("cursorPath").is_none());
}

#[test]
fn a_follow_closer_than_twice_is_refused() {
    use klipolo_model::project::FormatKind;
    use klipolo_model::timeline::{Framing, FramingMode};
    let mut p = fixture_with_cursor();
    let follow = |closer| Framing {
        mode: FramingMode::Fill,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: Some(Follow { closer }),
    };
    p.timeline.main[0]
        .framing
        .insert(FormatKind::Portrait, follow(2.0));
    assert!(klipolo_model::validate(&p).is_ok());
    p.timeline.main[0]
        .framing
        .insert(FormatKind::Portrait, follow(2.5));
    let e = klipolo_model::validate(&p).unwrap_err();
    assert_eq!(e.coded().code, "model.cursor.closer");
}

#[test]
fn an_unknown_clip_styles_no_clip_at_all() {
    let p = fixture();
    let op = Op::SetCursorStyle {
        target: CursorTarget::Clips {
            ids: vec!["a".into(), "gone".into()],
        },
        style: Some(CursorStyle::default()),
    };
    let e = apply_ops(&p, std::slice::from_ref(&op)).unwrap_err();
    assert_eq!(e.coded().code, "model.notFound.clip");
    // `apply_part` leaves its project part-way through on an error, so this sees any write made before the refusal.
    let mut part = p.clone();
    assert!(klipolo_model::apply_part(&mut part, &[op]).is_err());
    assert!(
        part.timeline.main[0].cursor.is_none(),
        "every clip is found before any changes"
    );
}

#[test]
fn game_takes_never_follow_by_default() {
    use klipolo_model::framing::default_framing;
    use klipolo_model::project::{FormatKind, RecordingKind, SourceRole};
    // Games keep no separate cursor; a game profile's cursor is burned in.
    let mut p = fixture_with_cursor();
    p.recordings[0].kind = RecordingKind::Gameplay;
    p.recordings[0].sources[0].role = SourceRole::Game;
    let clip = &p.timeline.main[0];
    let framing = default_framing(&p, clip, FormatKind::Portrait);
    assert!(framing.follow.is_none(), "{framing:?}");
}

#[test]
fn the_click_colour_reads_either_case_and_writes_hex_with_opacity() {
    let c: Rgba = serde_json::from_str(r##""#ff00807f""##).unwrap();
    assert_eq!(c, Rgba([0xFF, 0x00, 0x80, 0x7F]));
    assert_eq!(serde_json::to_string(&c).unwrap(), r##""#FF00807F""##);
    for bad in [
        r##""#FFF""##,
        r##""#FFFFFF""##,
        r##""#+FFFFFFF""##,
        r#""FFFFFF80""#,
        r##""#FFFFFF8G""##,
    ] {
        assert!(serde_json::from_str::<Rgba>(bad).is_err(), "{bad}");
    }
}
