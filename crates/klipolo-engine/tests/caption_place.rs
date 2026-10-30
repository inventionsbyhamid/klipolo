//! Where the caption block sits, as the canvas's hit box mirrors it:
//! holds six cases of `block_centre` and `max_width_share` (Bold, Arcade at a seam, ad-safe, a placed block, landscape
//! and square), and `src/lib/captionBlock.test.ts` checks the TS `captionBlock` against them, so the block the person
//! drags sits where the engine draws it. Rewrite it with `KLIPOLO_UPDATE_PARITY=1`.

use std::fs;
use std::path::PathBuf;

use klipolo_engine::plan::captions::place::{block_centre, max_width_share};
use klipolo_model::captions::styles::CaptionStyles;
use klipolo_model::contrib::ContribId;
use klipolo_model::project::{FormatKind, Project};
use serde::Serialize;

/// One case: the style, format and settings it is placed with, and what the engine answers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: &'static str,
    style: String,
    format: FormatKind,
    place: Option<f64>,
    ad_safe: bool,
    seam: Option<f64>,
    /// The block's height as a share of the frame's: the style's size (at the output's short side ÷ 1080, as the
    /// layout scales it) × line height × most lines.
    block_h: f64,
    centre: (f64, f64),
    width: f64,
}

/// The frame's size the UI's canvas boxes are shares of (`FORMAT_SIZE`).
fn frame(f: FormatKind) -> (f64, f64) {
    match f {
        FormatKind::Landscape => (1920.0, 1080.0),
        FormatKind::Portrait => (1080.0, 1920.0),
        FormatKind::Square => (1080.0, 1080.0),
    }
}

fn case(
    name: &'static str,
    style: &str,
    format: FormatKind,
    place: Option<f64>,
    ad_safe: bool,
    seam: Option<f64>,
) -> Case {
    let mut p = Project::new("p");
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin(style));
    p.captions.ad_safe = ad_safe;
    if let Some(y) = place {
        p.captions.place.insert(format, y);
    }
    let def = CaptionStyles::builtin()
        .for_settings(&p.captions, &p.settings.formats)
        .0;
    let (w, h) = frame(format);
    let px = def.size.size(format) * w.min(h) / 1080.0;
    let block_h = px * def.line_height * f64::from(def.pages.max_lines) / h;
    Case {
        name,
        style: format!("klipolo.builtin/{style}"),
        format,
        place,
        ad_safe,
        seam,
        block_h,
        centre: block_centre(&p, &def, format, block_h, seam),
        width: max_width_share(&p, &def, format),
    }
}

fn cases() -> Vec<Case> {
    use FormatKind::{Landscape, Portrait, Square};
    vec![
        case("bold", "caption.bold", Portrait, None, false, None),
        case(
            "arcade at a seam",
            "caption.arcade",
            Portrait,
            None,
            false,
            Some(0.5),
        ),
        case("ad-safe", "caption.bold", Portrait, None, true, None),
        case("placed", "caption.bold", Portrait, Some(0.3), false, None),
        case("landscape", "caption.clean", Landscape, None, false, None),
        case("square", "caption.bold", Square, None, false, None),
    ]
}

#[test]
fn caption_place_parity_fixture_is_current() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/caption-place-parity.json");
    let text = serde_json::to_string_pretty(&cases()).unwrap() + "\n";
    if std::env::var_os("KLIPOLO_UPDATE_PARITY").is_some() {
        fs::write(&path, &text).unwrap();
    }
    let committed = fs::read_to_string(&path).expect("missing: run with KLIPOLO_UPDATE_PARITY=1");
    assert!(
        committed == text,
        "src/lib/caption-place-parity.json is stale: run with KLIPOLO_UPDATE_PARITY=1"
    );
}
