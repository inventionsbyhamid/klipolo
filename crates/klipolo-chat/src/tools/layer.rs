//! The layer tool (`setLayer`): where a layer sits and how big it is, its rotation, opacity, blend and rounded corners,
//! a video layer's crop and Fit or Fill, its place in the stack, and words' style (`style`): the layer card's fields,
//! the text card's and the timeline's Bring forward and Send back. They come as `{key, value}` pairs (`sets`,
//! `LAYER_KEYS` and `style::STYLE_KEYS`), which the prompt lists, so the schema stays small. A box and a crop are
//! fractions of the frame and of the picture; a value outside what the model takes is refused with its range. Whether
//! the layer can take a crop, Fill or a style is the compiler's to judge.

use klipolo_model::crop::Crop;
use klipolo_model::layout::BoxN;
use klipolo_model::motion::Blend;
use klipolo_model::time::{Us, secs_to_us};

use super::error::Problem;
use super::motion::layer_field;
use super::sets::{pairs, schema};
use super::style::{STYLE_KEYS, StyleChange, read_style};
use super::{Tool, ToolCall};

/// Up or down the stack: onto the track above or below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Forward,
    Back,
}

/// The keys `setLayer` takes.
pub const LAYER_KEYS: [&str; 11] = [
    "box", "rotation", "opacity", "corners", "blend", "crop", "fill", "order", "fadeIn", "fadeOut",
    "duck",
];
/// How long a fade may be, in seconds.
pub const FADE_SECS: (f64, f64) = (0.0, 10.0);
const BLENDS: [(&str, Blend); 5] = [
    ("normal", Blend::Normal),
    ("multiply", Blend::Multiply),
    ("screen", Blend::Screen),
    ("add", Blend::Add),
    ("overlay", Blend::Overlay),
];

/// How far a box may reach past the frame, each way, as the layer card lets a layer be dragged.
pub const BOX_REACH: (f64, f64) = (-2.0, 3.0);
/// The smallest a box may be each way.
pub const MIN_BOX: f64 = 0.01;
/// Corner rounding, a share of the layer's shorter side.
pub const CORNERS: (f64, f64) = (0.0, 0.5);
/// Rotation, degrees either way.
pub const TURN: (f64, f64) = (-360.0, 360.0);

/// What `setLayer` changes; None keeps each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerChange {
    pub rect: Option<BoxN>,
    pub rotation: Option<f64>,
    pub opacity: Option<f64>,
    pub corners: Option<f64>,
    pub blend: Option<Blend>,
    /// `Crop::NONE` takes a crop off.
    pub crop: Option<Crop>,
    pub fill: Option<bool>,
    pub order: Option<Order>,
    /// A clip's or a sound layer's fades, and whether a song ducks under speech.
    pub fade_in_us: Option<Us>,
    pub fade_out_us: Option<Us>,
    pub duck: Option<bool>,
}

pub(super) const SET_LAYER: Tool = Tool {
    name: "setLayer",
    description: "Set a layer's or clip's card fields by key. Unset keys keep.",
    parameters: || vec![layer_field(), ("set", schema("Keys to set."))],
    read: |a| {
        let layer = a.layer_ref("layer")?;
        let mut change = LayerChange::default();
        let mut style = StyleChange::default();
        let keys: Vec<&'static str> = LAYER_KEYS.iter().chain(&STYLE_KEYS).copied().collect();
        let set = pairs(a, "set", &keys)?;
        if set.is_empty() {
            return Err(a.error("set", Problem::Empty));
        }
        for p in set {
            if STYLE_KEYS.contains(&p.key) {
                read_style(&p, a, &mut style)?;
                continue;
            }
            match p.key {
                "box" => {
                    let [x, y, w, h] = p.four(a, "four numbers: x y width height")?;
                    let (lo, hi) = BOX_REACH;
                    let fits = [x, y].iter().all(|v| (lo..=hi).contains(v))
                        && [w, h].iter().all(|v| (MIN_BOX..=hi).contains(v));
                    if !fits {
                        return Err(a.error(
                            &format!("{}.value", p.at),
                            Problem::Unused {
                                why: "must place x and y from -2 to 3 and size width and height from 0.01 to 3",
                            },
                        ));
                    }
                    change.rect = Some(BoxN { x, y, w, h });
                }
                "rotation" => change.rotation = Some(p.number(a, TURN)?),
                "opacity" => change.opacity = Some(p.number(a, (0.0, 1.0))?),
                "corners" => change.corners = Some(p.number(a, CORNERS)?),
                "blend" => {
                    let word = p.word(a, &BLENDS.map(|(w, _)| w))?;
                    change.blend = BLENDS.iter().find(|(w, _)| *w == word).map(|(_, b)| *b);
                }
                "crop" => {
                    let crop = if p
                        .value
                        .as_str()
                        .is_some_and(|s| s.trim().eq_ignore_ascii_case("none"))
                    {
                        Crop::NONE
                    } else {
                        let [left, top, right, bottom] =
                            p.four(a, "four numbers: left top right bottom, or none")?;
                        Crop {
                            left,
                            top,
                            right,
                            bottom,
                        }
                    };
                    if !crop.is_valid() {
                        return Err(a.error(
                            &format!("{}.value", p.at),
                            Problem::Unused {
                                why: "must be shares from 0 that leave at least a tenth of the picture each way",
                            },
                        ));
                    }
                    change.crop = Some(crop);
                }
                "fill" => change.fill = Some(p.flag(a)?),
                "fadeIn" => change.fade_in_us = Some(secs_to_us(p.number(a, FADE_SECS)?)),
                "fadeOut" => change.fade_out_us = Some(secs_to_us(p.number(a, FADE_SECS)?)),
                "duck" => change.duck = Some(p.flag(a)?),
                _ => {
                    change.order = Some(match p.word(a, &["forward", "back"])? {
                        "forward" => Order::Forward,
                        _ => Order::Back,
                    })
                }
            }
        }
        if style.outline == Some(super::style::Paint::None) && style.outline_width.is_some() {
            return Err(a.error(
                "set",
                Problem::Unused {
                    why: "sets outlineWidth with outline none: leave outlineWidth out",
                },
            ));
        }
        Ok(ToolCall::SetLayer {
            layer,
            change: Box::new(change),
            style: Box::new(style),
        })
    },
};
