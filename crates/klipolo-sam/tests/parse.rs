use std::collections::{BTreeMap, BTreeSet, HashMap};

use klipolo_sam::{parse_frames, BoxPx, FrameObjects};

const PERSON_TEXT: &str = include_str!("fixtures/person_concept_one_bit.txt");
const PERSON_BOXES: &str = include_str!("fixtures/person_concept_one_bit.boxes.json");
const BOX_TEXT: &str = include_str!("fixtures/box_prompt_f80_lossless.txt");
const BOX_BOXES: &str = include_str!("fixtures/box_prompt_f80_lossless.boxes.json");

/// Expected boxes from the reference parser, keyed by (frame, id).
fn expected(json: &str) -> HashMap<(u32, u32), (BoxPx, f64)> {
    let rows: Vec<(u32, u32, u32, u32, u32, u32, f64)> = serde_json::from_str(json).unwrap();
    rows.into_iter()
        .map(|(frame, id, x0, y0, x1, y1, c)| ((frame, id), (BoxPx { x0, y0, x1, y1 }, c)))
        .collect()
}

/// Checks every parsed object against the reference boxes, both ways.
fn assert_matches_reference(frames: &[FrameObjects], json: &str) {
    let want = expected(json);
    let mut seen = 0;
    for f in frames {
        for o in &f.objects {
            let (bbox, c) = want
                .get(&(f.frame, o.id))
                .unwrap_or_else(|| panic!("unexpected object {} on frame {}", o.id, f.frame));
            assert_eq!(o.bbox, *bbox, "box of object {} on frame {}", o.id, f.frame);
            let got = o.confidence.expect("confidence") as f64;
            assert!((got - c).abs() < 1e-6, "confidence {got} vs {c}");
            assert_eq!((o.frame_w, o.frame_h), (854, 480));
            seen += 1;
        }
    }
    assert_eq!(seen, want.len(), "every reference box is parsed");
}

fn line(frame: u32, records: &[&str]) -> String {
    format!("<{frame}f>{}", records.join(","))
}

fn record(id: u32, box_fields: &str, payload: &str) -> String {
    format!("{id}<|box;{box_fields}|><|mask;x=0;y=0;c=0.5;data=4,3,{payload}|>")
}

const BOX_A: &str = "x1=10;y1=20;x2=12;y2=23;w=854;h=480;c=0.5";

#[test]
fn parses_person_concept_fixture() {
    let frames = parse_frames(PERSON_TEXT).unwrap();

    assert_eq!(frames.len(), 160);
    let order: Vec<u32> = frames.iter().map(|f| f.frame).collect();
    assert_eq!(order, (0..160).collect::<Vec<_>>());
    for f in &frames {
        assert!((8..=9).contains(&f.objects.len()), "frame {} has {}", f.frame, f.objects.len());
    }

    // Ids are stable: the same nine ids across the clip, each present on
    // (almost) every frame, as the reference parser counted them.
    let mut present: BTreeMap<u32, usize> = BTreeMap::new();
    for f in &frames {
        for o in &f.objects {
            *present.entry(o.id).or_default() += 1;
        }
    }
    let want: BTreeMap<u32, usize> = [
        (0, 160),
        (1, 157),
        (2, 159),
        (3, 160),
        (4, 160),
        (5, 160),
        (6, 160),
        (7, 158),
        (8, 160),
    ]
    .into_iter()
    .collect();
    assert_eq!(present, want);

    assert_matches_reference(&frames, PERSON_BOXES);
}

#[test]
fn parses_box_prompt_fixture_with_lossless_masks() {
    // The fixture really is lossless: every mask uses the `~` marker.
    let markers: BTreeSet<char> = BOX_TEXT
        .split("data=")
        .skip(1)
        .map(|rest| {
            let after_h_w = rest.splitn(3, ',').nth(2).unwrap();
            after_h_w.chars().next().unwrap()
        })
        .collect();
    assert_eq!(markers, BTreeSet::from(['~']));
    // The model answers from the prompt frame forward, then backward.
    assert!(BOX_TEXT.starts_with("<80f>"));

    let frames = parse_frames(BOX_TEXT).unwrap();

    assert_eq!(frames.len(), 160);
    let order: Vec<u32> = frames.iter().map(|f| f.frame).collect();
    assert_eq!(order, (0..160).collect::<Vec<_>>());
    for f in &frames {
        assert_eq!(f.objects.len(), 1, "frame {}", f.frame);
        assert_eq!(f.objects[0].id, 1);
    }
    let mut cs: Vec<f32> = frames.iter().map(|f| f.objects[0].confidence.unwrap()).collect();
    cs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = cs[cs.len() / 2];
    assert!((0.96..0.98).contains(&median), "median confidence {median}");

    assert_matches_reference(&frames, BOX_BOXES);
}

#[test]
fn boxes_are_half_open_source_pixels() {
    // First person on frame 0 of the concept fixture: inclusive x2=660, y2=471,
    // and a 381 x 125 (H x W) mask that covers the box exactly.
    let text = "<0f>0<|box;x1=536;y1=91;x2=660;y2=471;w=854;h=480;c=0.9259259104728699|>\
                <|mask;x=0;y=0;c=0.9259259104728699;data=381,125,!!!!!#`K)+wP|>,\
                7<|box;x1=0;y1=0;x2=853;y2=479;w=854;h=480|><|mask;x=0;y=0;data=480,854,!abc|>";
    let frames = parse_frames(text).unwrap();
    assert_eq!(frames.len(), 1);
    let objs = &frames[0].objects;

    assert_eq!(objs[0].bbox, BoxPx { x0: 536, y0: 91, x1: 661, y1: 472 });
    assert_eq!((objs[0].bbox.width(), objs[0].bbox.height()), (125, 381));
    assert_eq!((objs[0].frame_w, objs[0].frame_h), (854, 480));
    assert!((objs[0].confidence.unwrap() - 0.925_925_9).abs() < 1e-6);

    // A full-frame box ends exactly at the frame edge; no confidence without `c`.
    assert_eq!(objs[1].id, 7);
    assert_eq!(objs[1].bbox, BoxPx { x0: 0, y0: 0, x1: 854, y1: 480 });
    assert_eq!(objs[1].confidence, None);
}

#[test]
fn garbage_line_is_a_parse_error_with_line_number() {
    let text = format!(
        "{}\n{}\nthis is not a frame\n{}\n",
        line(0, &[&record(0, BOX_A, "!ab")]),
        line(1, &[&record(0, BOX_A, "!ab")]),
        line(2, &[&record(0, BOX_A, "!ab")]),
    );
    let err = parse_frames(&text).unwrap_err();
    assert_eq!(err.line, 3);
    assert!(!err.reason.is_empty());
    assert!(err.to_string().contains("line 3"), "{err}");
}

#[test]
fn mask_payloads_are_skipped_whatever_characters_they_hold() {
    // Base85 payloads use `=`, `~`, `!`, `{`, `}`; also survive `;` and a lone `|`.
    let payloads = ["~a=b~c!d{e}f", "!=;=;x=1;y=2", "~ab|cd", "!}~!=", "~"];
    for payload in payloads {
        let text = line(4, &[&record(2, BOX_A, payload), &record(3, BOX_A, payload)]);
        let frames = parse_frames(&text).unwrap_or_else(|e| panic!("{payload}: {e}"));
        let ids: Vec<u32> = frames[0].objects.iter().map(|o| o.id).collect();
        assert_eq!(ids, [2, 3], "{payload}");
        assert_eq!(frames[0].objects[1].bbox, BoxPx { x0: 10, y0: 20, x1: 13, y1: 24 });
    }
}

#[test]
fn frames_are_returned_in_order_and_repeats_merge() {
    let a = record(0, BOX_A, "!a");
    let b = record(1, BOX_A, "!b");
    let a_moved = record(0, "x1=0;y1=0;x2=1;y2=1;w=854;h=480", "!c");
    let text = [
        line(5, &[&a, &b]),
        line(2, &[&a]),
        line(5, &[&a_moved]),
    ]
    .join("\n");
    let frames = parse_frames(&text).unwrap();
    let order: Vec<u32> = frames.iter().map(|f| f.frame).collect();
    assert_eq!(order, [2, 5]);
    // The later line for frame 5 replaces object 0 and keeps object 1.
    let five = &frames[1].objects;
    assert_eq!(five.len(), 2);
    assert_eq!(five[0].id, 0);
    assert_eq!(five[0].bbox, BoxPx { x0: 0, y0: 0, x1: 2, y1: 2 });
    assert_eq!(five[1].id, 1);
}

#[test]
fn empty_output_and_blank_lines_are_fine() {
    assert_eq!(parse_frames("").unwrap(), vec![]);
    assert_eq!(parse_frames("\n \r\n").unwrap(), vec![]);

    let text = format!("\r\n{}\r\n\r\n{}\r\n", line(0, &[&record(0, BOX_A, "!a")]), line(1, &[&record(0, BOX_A, "!a")]));
    let frames = parse_frames(&text).unwrap();
    assert_eq!(frames.len(), 2);
}

#[test]
fn frame_line_without_objects_is_an_empty_frame() {
    let frames = parse_frames("<3f>").unwrap();
    assert_eq!(frames, vec![FrameObjects { frame: 3, objects: vec![] }]);
}

#[test]
fn unknown_tokens_and_fields_are_ignored() {
    let text = "<0f>0<|box;x1=1;y1=2;x2=3;y2=4;w=10;h=10;c=0.5;z=9|><|mask;x=0;y=0;data=3,3,!ab|><|label;text=hi|>";
    let frames = parse_frames(text).unwrap();
    assert_eq!(frames[0].objects[0].bbox, BoxPx { x0: 1, y0: 2, x1: 4, y1: 5 });
}

#[test]
fn malformed_records_are_errors_on_their_line() {
    let ok = line(0, &[&record(0, BOX_A, "!a")]);
    let cases: Vec<(&str, String)> = vec![
        ("no mask", "<1f>0<|box;x1=1;y1=1;x2=2;y2=2;w=9;h=9|>".into()),
        ("no box", "<1f>0<|mask;x=0;y=0;data=1,1,!a|>".into()),
        ("two boxes", "<1f>0<|box;x1=1;y1=1;x2=2;y2=2;w=9;h=9|><|box;x1=1;y1=1;x2=2;y2=2;w=9;h=9|><|mask;x=0;y=0;data=1,1,!a|>".into()),
        ("no id", "<1f><|box;x1=1;y1=1;x2=2;y2=2;w=9;h=9|><|mask;x=0;y=0;data=1,1,!a|>".into()),
        ("missing field", line(1, &[&record(0, "x1=1;y1=1;x2=2;w=9;h=9", "!a")])),
        ("negative", line(1, &[&record(0, "x1=-1;y1=1;x2=2;y2=2;w=9;h=9", "!a")])),
        ("not a number", line(1, &[&record(0, "x1=1;y1=1;x2=2.5;y2=2;w=9;h=9", "!a")])),
        ("past the frame", line(1, &[&record(0, "x1=1;y1=1;x2=9;y2=2;w=9;h=9", "!a")])),
        ("inverted", line(1, &[&record(0, "x1=5;y1=1;x2=4;y2=2;w=9;h=9", "!a")])),
        ("zero frame", line(1, &[&record(0, "x1=0;y1=0;x2=0;y2=0;w=0;h=9", "!a")])),
        ("bad confidence", line(1, &[&record(0, "x1=1;y1=1;x2=2;y2=2;w=9;h=9;c=high", "!a")])),
        ("confidence above 1", line(1, &[&record(0, "x1=1;y1=1;x2=2;y2=2;w=9;h=9;c=1.5", "!a")])),
        ("duplicate field", line(1, &[&record(0, "x1=1;x1=2;y1=1;x2=2;y2=2;w=9;h=9", "!a")])),
        ("bad mask marker", line(1, &[&record(0, BOX_A, "#a")])),
        ("mask without data", "<1f>0<|box;x1=1;y1=1;x2=2;y2=2;w=9;h=9|><|mask;x=0;y=0|>".into()),
        ("mask size", "<1f>0<|box;x1=1;y1=1;x2=2;y2=2;w=9;h=9|><|mask;x=0;y=0;data=0,1,!a|>".into()),
        ("unclosed token", "<1f>0<|box;x1=1;y1=1;x2=2;y2=2;w=9;h=9".into()),
        ("junk after record", format!("{}xyz", line(1, &[&record(0, BOX_A, "!a")]))),
        ("trailing comma", format!("{},", line(1, &[&record(0, BOX_A, "!a")]))),
        ("duplicate id", line(1, &[&record(0, BOX_A, "!a"), &record(0, BOX_A, "!a")])),
        ("bad header", "<1>0".into()),
        ("negative frame", "<-1f>".into()),
    ];
    for (name, bad) in cases {
        let text = format!("{ok}\n{bad}\n{ok}");
        let err = parse_frames(&text).expect_err(name);
        assert_eq!(err.line, 2, "{name}: {err}");
    }
}
