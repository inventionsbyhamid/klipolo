//! Pins TS `resolveRect`/`cameraRect`/`containRect`/`textScale` (apps/desktop/src/lib/layout.ts) to Rust: gizmos
//! hit-test with the TS functions, the engine draws with these. Also `shownFromUs`
//! (apps/desktop/src/lib/timeline.ts): where the editor puts the playhead to show a layer, past a title's intro
//! animation. Regenerate with `KLIPOLO_UPDATE_PARITY=1 cargo test -p klipolo-engine --test parity`.

use std::fs;
use std::path::PathBuf;

use klipolo_engine::plan::framing::camera_rect_for;
use klipolo_engine::plan::overlay::{contain, shown_from_us, text_scale};
use klipolo_model::contrib::builtin;
use klipolo_model::layout::{Anchor, BoxN, Layout, resolve_rect};
use klipolo_model::motion::{Motion, MotionUse};
use klipolo_model::project::FormatKind::{self, *};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::timeline::{CameraLayout, CameraMode, Item, ItemContent, TextStyle};
use serde_json::{Value, json};

const FORMATS: [FormatKind; 3] = [Landscape, Portrait, Square];

fn build() -> Value {
    use Anchor::*;
    let anchors = [
        TopLeft,
        TopCenter,
        TopRight,
        MiddleLeft,
        Center,
        MiddleRight,
        BottomLeft,
        BottomCenter,
        BottomRight,
    ];
    let mut rects = Vec::new();
    for (i, anchor) in anchors.into_iter().enumerate() {
        let k = i as f64;
        let mut layout = Layout::new(
            anchor,
            BoxN {
                x: 0.05 + 0.07 * k,
                y: 0.04 + 0.08 * k,
                w: 0.3,
                h: 0.12,
            },
        );
        if anchor == Center {
            layout.overrides.insert(
                Square,
                BoxN {
                    x: 0.1,
                    y: 0.2,
                    w: 0.5,
                    h: 0.5,
                },
            );
        }
        for primary in FORMATS {
            for target in FORMATS {
                rects.push(json!({ "layout": layout, "primary": primary, "target": target, "rect": resolve_rect(&layout, primary, target) }));
            }
        }
    }
    let mut cameras = Vec::new();
    for padding in [0.0, 0.1] {
        for mode in [
            CameraMode::Bubble,
            CameraMode::SplitBottom,
            CameraMode::Hidden,
        ] {
            let camera = CameraLayout {
                mode,
                ..CameraLayout::default()
            };
            for primary in FORMATS {
                for format in FORMATS {
                    let rect = camera_rect_for(&camera, padding, primary, format);
                    cameras.push(json!({ "camera": camera, "padding": padding, "primary": primary, "format": format, "rect": rect }));
                }
            }
        }
    }
    // Image and video layers: the picture's shape inside its box (wide, square, tall, unknown size).
    let mut contains = Vec::new();
    let boxes = [
        BoxN {
            x: 0.1,
            y: 0.2,
            w: 0.4,
            h: 0.3,
        },
        BoxN {
            x: -0.2,
            y: 0.5,
            w: 0.9,
            h: 0.6,
        },
    ];
    for rect in boxes {
        for (width, height) in [(1920, 1080), (512, 512), (1080, 1920), (0, 0)] {
            let asset = Asset {
                id: "a".into(),
                kind: AssetKind::Image,
                path: String::new(),
                name: String::new(),
                duration_us: 0,
                width,
                height,
                fps: 0.0,
                has_audio: false,
                audio_lead_us: None,
                rotation: None,
            };
            for format in FORMATS {
                contains.push(json!({ "rect": rect, "width": width, "height": height, "format": format, "box": contain(rect, &asset, format) }));
            }
        }
    }
    // Text scales with its box per format: derived boxes, one squeezed by the canvas, and own placements (one in the
    // primary format itself).
    let mut texts = Vec::new();
    let title = BoxN {
        x: 0.2,
        y: 0.08,
        w: 0.6,
        h: 0.12,
    };
    let tall = BoxN {
        x: 0.1,
        y: 0.05,
        w: 0.8,
        h: 0.9,
    };
    let mut own = Layout::new(Anchor::TopCenter, title);
    own.overrides.insert(
        Portrait,
        BoxN {
            x: 0.1,
            y: 0.3,
            w: 0.8,
            h: 0.1,
        },
    );
    own.overrides.insert(
        Landscape,
        BoxN {
            x: 0.25,
            y: 0.1,
            w: 0.5,
            h: 0.2,
        },
    );
    for layout in [
        Layout::new(Anchor::TopCenter, title),
        Layout::new(Anchor::Center, tall),
        own,
    ] {
        for primary in FORMATS {
            for format in FORMATS {
                texts.push(json!({ "layout": layout, "primary": primary, "format": format, "scale": text_scale(&layout, primary, format) }));
            }
        }
    }
    // When a layer is first drawn whole: titles with and without an enter preset, one whose enter is longer than the
    // layer (both sides cap it at the layer), and an image with and without one.
    let text = || ItemContent::Text {
        text: "Title".into(),
        style: TextStyle::default(),
    };
    let image = || ItemContent::Image {
        asset_id: "a".into(),
    };
    let preset = |word: &str, duration_us: i64| MotionUse {
        preset: builtin::motion(word),
        params: Default::default(),
        duration_us,
        ease: None,
    };
    let motion = |enter: Option<MotionUse>| Motion {
        enter,
        exit: Some(preset("fade", 300_000)),
        ..Motion::default()
    };
    let mut shown = Vec::new();
    for (start_us, duration_us, content, enter) in [
        (1_000_000, 3_000_000, text(), Some(preset("fade", 300_000))),
        (
            2_500_000,
            3_000_000,
            text(),
            Some(preset("typewriter", 300_000)),
        ),
        (1_000_000, 3_000_000, text(), None),
        (500_000, 200_000, text(), Some(preset("pop", 300_000))),
        (
            1_000_000,
            3_000_000,
            image(),
            Some(preset("slide", 500_000)),
        ),
        (1_000_000, 3_000_000, image(), None),
    ] {
        let mut layout = Layout::new(Anchor::TopCenter, title);
        layout.motion = Some(motion(enter));
        let item = Item {
            id: "i".into(),
            start_us,
            duration_us,
            attach: None,
            layout,
            content,
            effects: Vec::new(),
        };
        shown.push(json!({ "item": item, "shownUs": shown_from_us(&item) }));
    }
    sorted_keys(
        json!({ "rects": rects, "cameras": cameras, "contains": contains, "texts": texts, "shown": shown }),
    )
}

/// `v` with every object's keys sorted, at every depth: the fixture is the same whether or not a crate that turns on
/// serde_json's `preserve_order` (klipolo-ai) builds alongside, as CI's combined runs do.
fn sorted_keys(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, sorted_keys(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted_keys).collect()),
        other => other,
    }
}

#[test]
fn layout_parity_fixture_is_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/lib/layout-parity.json");
    let text = serde_json::to_string_pretty(&build()).unwrap() + "\n";
    if std::env::var_os("KLIPOLO_UPDATE_PARITY").is_some() {
        fs::write(&path, &text).unwrap();
    }
    let committed = fs::read_to_string(&path).expect("missing: run with KLIPOLO_UPDATE_PARITY=1");
    assert!(
        committed == text,
        "apps/desktop/src/lib/layout-parity.json is stale: run with KLIPOLO_UPDATE_PARITY=1"
    );
}
