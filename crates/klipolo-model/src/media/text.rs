//! Words put over the video: a text layer from one of four presets (Title, Lower third, Plain, Caption), or a title
//! template, a motion graphic that fills the frame (`presets::SCENE_PRESETS`: the lower third and the title card).
//!
//! **Where a text preset sits in each format.** A preset is placed in every format at once: its rect in the primary,
//! and its own place in each other format as an override (as `Layout::full_frame` does). In 9:16 that place is inside
//! the area the apps' own buttons and captions leave clear ([`PORTRAIT_CLEAR`]), which a rect derived from 16:9 by
//! `resolve_rect` (margins kept, sizes following the shorter side) wouldn't be: a title 4 % from the top of a 16:9 frame
//! would sit under TikTok's tabs. Every format's box is as tall in pixels as the others (each has a 1080-pixel short
//! side), so the text draws at its size in every format (`text_scale` 1). Moving it by hand in one format moves only
//! that format's place, as for any layer.

use std::collections::BTreeMap;

use crate::contrib::builtin;
use crate::contrib::ContribId;
use crate::error::{invalid, Result};
use crate::font::FontRef;
use crate::ids::new_id;
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::code;
use crate::motion::{Motion, MotionUse};
use crate::ops::{Op, TrackTarget};
use crate::params::ParamValue;
use crate::project::{FormatKind, Project};
use crate::query::main_track_duration;
use crate::time::Us;
use crate::timeline::{Item, ItemContent, TextAlign, TextStyle};

/// How long a text layer shows when it is added.
pub const TEXT_US: Us = 3_000_000;
/// How long a title template shows when it is added: its scene's own length (4 s, `in` and `out` included).
pub const TEMPLATE_US: Us = 4_000_000;
/// The enter and exit fade of a text layer.
pub const TEXT_FADE_US: Us = 300_000;
/// What a 9:16 video keeps clear of the apps' own buttons, captions and tabs (the union of TikTok, Reels and Shorts:
/// 14 % at the top, 26 % at the bottom, 6 % at the left and 13 % at the right). The preview's safe area is the same box
/// (`apps/desktop/src/lib/safeAreas`).
pub const PORTRAIT_CLEAR: BoxN = BoxN {
    x: 0.06,
    y: 0.14,
    w: 0.81,
    h: 0.6,
};
/// Kept clear inside the area a preset sits in, in pixels at a 1080-pixel short side (4 % of it).
const MARGIN_PX: f64 = 43.2;
/// A preset's box is this many font sizes tall: one line and its box's padding.
const BOX_LINES: f64 = 1.6;

/// The four text presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPreset {
    /// Big words at the top centre, 96 px.
    Title,
    /// At the bottom left on a dark box, 56 px.
    LowerThird,
    /// In the middle, 64 px.
    Plain,
    /// At the bottom centre, where captions go, 64 px.
    Caption,
}

/// The title templates: motion graphics that fill the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleTemplate {
    LowerThird,
    TitleCard,
}

impl TitleTemplate {
    /// Its scene preset (`presets::SCENE_PRESETS`).
    pub fn source(self) -> ContribId {
        ContribId::builtin(match self {
            Self::LowerThird => "scene.lower-third",
            Self::TitleCard => "scene.title-card",
        })
    }
}

struct Spec {
    size: f64,
    align: TextAlign,
    background: Option<&'static str>,
    /// The widest the box is, in pixels at a 1080-pixel short side (narrower where the area is).
    max_w: f64,
    /// Where the box sits in its area, across and down: start, middle or end.
    at: (Side, Side),
    anchor: Anchor,
}

#[derive(Clone, Copy)]
enum Side {
    Start,
    Middle,
    End,
}

fn spec(preset: TextPreset) -> Spec {
    use Side::*;
    match preset {
        TextPreset::Title => Spec {
            size: 96.0,
            align: TextAlign::Center,
            background: None,
            max_w: 1536.0,
            at: (Middle, Start),
            anchor: Anchor::TopCenter,
        },
        TextPreset::LowerThird => Spec {
            size: 56.0,
            align: TextAlign::Left,
            // Black at 80 %.
            background: Some("#000000CC"),
            max_w: 1000.0,
            at: (Start, End),
            anchor: Anchor::BottomLeft,
        },
        TextPreset::Plain => Spec {
            size: 64.0,
            align: TextAlign::Center,
            background: None,
            max_w: 1200.0,
            at: (Middle, Middle),
            anchor: Anchor::Center,
        },
        TextPreset::Caption => Spec {
            size: 64.0,
            align: TextAlign::Center,
            background: None,
            max_w: 1400.0,
            at: (Middle, End),
            anchor: Anchor::BottomCenter,
        },
    }
}

fn canvas(format: FormatKind) -> (f64, f64) {
    match format {
        FormatKind::Landscape => (1920.0, 1080.0),
        FormatKind::Portrait => (1080.0, 1920.0),
        FormatKind::Square => (1080.0, 1080.0),
    }
}

/// The area a preset sits in: the whole frame, or in 9:16 what the apps keep clear.
fn area(format: FormatKind) -> BoxN {
    match format {
        FormatKind::Portrait => PORTRAIT_CLEAR,
        _ => BoxN::FULL,
    }
}

/// `preset`'s box in `format`, inside that format's area less the margin.
pub fn preset_rect(preset: TextPreset, format: FormatKind) -> BoxN {
    let s = spec(preset);
    let (cw, ch) = canvas(format);
    let a = area(format);
    let (ax, ay) = (a.x * cw + MARGIN_PX, a.y * ch + MARGIN_PX);
    let (aw, ah) = (a.w * cw - 2.0 * MARGIN_PX, a.h * ch - 2.0 * MARGIN_PX);
    let w = s.max_w.min(aw);
    let h = (s.size * BOX_LINES).min(ah);
    let place = |side: Side, from: f64, room: f64, size: f64| match side {
        Side::Start => from,
        Side::Middle => from + (room - size) / 2.0,
        Side::End => from + room - size,
    };
    BoxN {
        x: place(s.at.0, ax, aw, w) / cw,
        y: place(s.at.1, ay, ah, h) / ch,
        w: w / cw,
        h: h / ch,
    }
}

/// `preset`'s layout in `primary`: its rect there, and its own place in each other format.
pub fn preset_layout(preset: TextPreset, primary: FormatKind) -> Layout {
    let mut layout = Layout::new(spec(preset).anchor, preset_rect(preset, primary));
    for format in [
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ] {
        if format != primary {
            layout.overrides.insert(format, preset_rect(preset, format));
        }
    }
    layout
}

fn fade() -> MotionUse {
    MotionUse {
        preset: builtin::motion("fade"),
        params: BTreeMap::new(),
        duration_us: TEXT_FADE_US,
        ease: None,
    }
}

/// A text layer of `preset` reading `text`, from `start_us`: Inter 700, white, with a shadow, fading in and out.
pub fn text_item(preset: TextPreset, text: &str, primary: FormatKind, start_us: Us) -> Item {
    let s = spec(preset);
    let mut layout = preset_layout(preset, primary);
    layout.motion = Some(Motion {
        enter: Some(fade()),
        exit: Some(fade()),
        ..Motion::default()
    });
    Item {
        id: new_id(),
        start_us: start_us.max(0),
        duration_us: TEXT_US,
        attach: None,
        layout,
        content: ItemContent::Text {
            text: text.to_owned(),
            style: TextStyle {
                font: FontRef::inter(),
                size: s.size,
                weight: 700,
                italic: false,
                color: "#FFFFFF".into(),
                background: s.background.map(str::to_owned),
                outline: None,
                shadow: true,
                align: s.align,
            },
        },
        effects: Vec::new(),
    }
}

/// The ops that add a text layer of `preset` reading `text` at `at_us`: on the first overlay track with room (two at the
/// same time go on two tracks), attached to the clip under it (none past the main track's end). Refused for blank text.
pub fn add_text_ops(p: &Project, preset: TextPreset, text: &str, at_us: Us) -> Result<Vec<Op>> {
    if text.trim().is_empty() {
        return Err(invalid(code!("model.text.empty")));
    }
    let item = text_item(preset, text, p.settings.primary_format, at_us);
    Ok(vec![Op::AddItem {
        track: TrackTarget::Auto,
        attach: item.start_us < main_track_duration(&p.timeline.main),
        item,
    }])
}

/// The ops that add title template `template` at `at_us`, filling the frame in every format (`Layout::full_frame`), its
/// settings `params` (its own defaults for any left out: "Your name", the brand yellow), `TEMPLATE_US` long, on the first
/// overlay track with room, attached to the clip under it.
pub fn add_template_ops(
    p: &Project,
    template: TitleTemplate,
    params: BTreeMap<String, ParamValue>,
    at_us: Us,
) -> Vec<Op> {
    let start = at_us.max(0);
    let item = Item {
        id: new_id(),
        start_us: start,
        duration_us: TEMPLATE_US,
        attach: None,
        layout: Layout::full_frame(),
        content: ItemContent::generator(template.source(), params),
        effects: Vec::new(),
    };
    vec![Op::AddItem {
        track: TrackTarget::Auto,
        attach: start < main_track_duration(&p.timeline.main),
        item,
    }]
}
