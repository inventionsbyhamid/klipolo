//! The recording tools: how a recording's camera shows (`setCameraMode`), where its bubble sits and how it looks
//! (`adjustCamera`), and the camera made a layer of its own (`detachCamera`). Each is one entry of `TOOLS`. The words
//! they take are the model's own (`CameraMode`, `CameraShape`, `Anchor`, `FormatKind`), and the bubble's sizes and
//! steps.
//!
//! Each takes `clips`, the refs of the clips it changes (`c2`, or `c3-c41` for a run as the context folds it), or null
//! for every clip that has a camera; whether a ref names a clip is the compiler's to judge.

use klipolo_model::layout::Anchor;
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::{CameraMode, CameraShape};
use serde_json::{Value, json};

use super::args::{Args, kind};
use super::error::{Problem, ToolError};
use super::schema::{Word, nullable_boolean, nullable_words, string, words};
use super::{FORMATS, Tool, ToolCall};

/// The bubble's sizes: a share of a 16:9 frame's width, the same bubble in every format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BubbleSize {
    Small,
    Medium,
    Large,
}

/// One step bigger or smaller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeStep {
    Bigger,
    Smaller,
}

/// What `adjustCamera` changes: None leaves it as it is. `format` gives the place and size to that format alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BubbleChange {
    pub position: Option<Anchor>,
    pub size: Option<BubbleSize>,
    pub size_step: Option<SizeStep>,
    pub shape: Option<CameraShape>,
    pub border: Option<bool>,
    pub mirror: Option<bool>,
    pub format: Option<FormatKind>,
}

impl BubbleChange {
    /// Whether it changes anything at all.
    pub fn is_empty(&self) -> bool {
        self.position.is_none()
            && self.size.is_none()
            && self.size_step.is_none()
            && self.shape.is_none()
            && self.border.is_none()
            && self.mirror.is_none()
    }
}

/// The ways a camera shows.
pub const CAMERA_MODES: [CameraMode; 3] = [
    CameraMode::Bubble,
    CameraMode::SplitBottom,
    CameraMode::Hidden,
];
/// The bubble's shapes.
pub const CAMERA_SHAPES: [CameraShape; 3] = [
    CameraShape::Circle,
    CameraShape::Rounded,
    CameraShape::Square,
];
/// Where the bubble sits: the nine places of the frame.
pub const ANCHORS: [Anchor; 9] = [
    Anchor::TopLeft,
    Anchor::TopCenter,
    Anchor::TopRight,
    Anchor::MiddleLeft,
    Anchor::Center,
    Anchor::MiddleRight,
    Anchor::BottomLeft,
    Anchor::BottomCenter,
    Anchor::BottomRight,
];
/// The bubble's sizes.
pub const BUBBLE_SIZES: [BubbleSize; 3] =
    [BubbleSize::Small, BubbleSize::Medium, BubbleSize::Large];
/// The bubble's steps.
pub const SIZE_STEPS: [SizeStep; 2] = [SizeStep::Bigger, SizeStep::Smaller];

impl Word for CameraMode {
    fn word(self) -> &'static str {
        match self {
            CameraMode::Bubble => "bubble",
            CameraMode::SplitBottom => "splitBottom",
            CameraMode::Hidden => "hidden",
        }
    }
}

impl Word for CameraShape {
    fn word(self) -> &'static str {
        match self {
            CameraShape::Circle => "circle",
            CameraShape::Rounded => "rounded",
            CameraShape::Square => "square",
        }
    }
}

impl Word for Anchor {
    fn word(self) -> &'static str {
        match self {
            Anchor::TopLeft => "topLeft",
            Anchor::TopCenter => "topCenter",
            Anchor::TopRight => "topRight",
            Anchor::MiddleLeft => "middleLeft",
            Anchor::Center => "center",
            Anchor::MiddleRight => "middleRight",
            Anchor::BottomLeft => "bottomLeft",
            Anchor::BottomCenter => "bottomCenter",
            Anchor::BottomRight => "bottomRight",
        }
    }
}

impl Word for BubbleSize {
    fn word(self) -> &'static str {
        match self {
            BubbleSize::Small => "small",
            BubbleSize::Medium => "medium",
            BubbleSize::Large => "large",
        }
    }
}

impl Word for SizeStep {
    fn word(self) -> &'static str {
        match self {
            SizeStep::Bigger => "bigger",
            SizeStep::Smaller => "smaller",
        }
    }
}

/// The model's word for a camera mode, as the tools and the context write it ("splitBottom").
pub fn mode_word(mode: CameraMode) -> &'static str {
    mode.word()
}

/// The model's word for a bubble's shape ("rounded").
pub fn shape_word(shape: CameraShape) -> &'static str {
    shape.word()
}

/// The model's word for a place in the frame ("topLeft").
pub fn anchor_word(anchor: Anchor) -> &'static str {
    anchor.word()
}

impl Args {
    /// The refs of the clips a call changes: a list of refs, or None when it is null, left out or empty (every clip).
    fn clip_refs(&mut self, key: &str) -> Result<Option<Vec<String>>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(None);
        };
        let Value::Array(items) = value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "a list or null",
                    got: kind(&value),
                },
            ));
        };
        let mut refs = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let at = format!("{key}[{i}]");
            self.map.insert(at.clone(), item);
            refs.push(self.text(&at)?);
        }
        Ok((!refs.is_empty()).then_some(refs))
    }
}

/// `clips`, as every recording tool takes it.
fn clips(what: &str) -> (&'static str, Value) {
    (
        "clips",
        json!({
            "type": ["array", "null"],
            "items": string("A clip ref (c2) or a run (c3-c41)."),
            "description": format!("Null for every clip {what}."),
        }),
    )
}

pub(super) const SET_CAMERA_MODE: Tool = Tool {
    name: "setCameraMode",
    description: "Show a recording's camera as a bubble over the screen or split under the screen, or hide it.",
    parameters: || {
        vec![
            clips("with a camera"),
            (
                "mode",
                words(
                    &CAMERA_MODES,
                    "splitBottom puts it under the screen, for vertical.",
                ),
            ),
            (
                "format",
                nullable_words(
                    &FORMATS,
                    "Only this format. Null for every format, but splitBottom is for portrait.",
                ),
            ),
        ]
    },
    read: |a| {
        Ok(ToolCall::SetCameraMode {
            clips: a.clip_refs("clips")?,
            mode: a.word("mode", &CAMERA_MODES)?,
            format: a.nullable_word("format", &FORMATS)?,
        })
    },
};

pub(super) const ADJUST_CAMERA: Tool = Tool {
    name: "adjustCamera",
    description: "Move, resize or restyle a recording's camera bubble. Null leaves a setting as it is.",
    parameters: || {
        vec![
            clips("with a camera"),
            ("position", nullable_words(&ANCHORS, "Where in the frame.")),
            (
                "size",
                nullable_words(&BUBBLE_SIZES, "medium is the usual size."),
            ),
            (
                "sizeStep",
                nullable_words(&SIZE_STEPS, "One step bigger or smaller than now."),
            ),
            ("shape", nullable_words(&CAMERA_SHAPES, "Its shape.")),
            ("border", nullable_boolean("True shows a border.")),
            ("mirror", nullable_boolean("True shows it as in a mirror.")),
            (
                "format",
                nullable_words(
                    &FORMATS,
                    "Place and size it in this format only. Null for every format.",
                ),
            ),
        ]
    },
    read: |a| {
        Ok(ToolCall::AdjustCamera {
            clips: a.clip_refs("clips")?,
            change: BubbleChange {
                position: a.nullable_word("position", &ANCHORS)?,
                size: a.nullable_word("size", &BUBBLE_SIZES)?,
                size_step: a.nullable_word("sizeStep", &SIZE_STEPS)?,
                shape: a.nullable_word("shape", &CAMERA_SHAPES)?,
                border: a.nullable_flag("border")?,
                mirror: a.nullable_flag("mirror")?,
                format: a.nullable_word("format", &FORMATS)?,
            },
        })
    },
};

pub(super) const DETACH_CAMERA: Tool = Tool {
    name: "detachCamera",
    description: "Make a recording's camera a layer of its own, to move, trim and cut apart from the screen.",
    parameters: || vec![clips("with a camera")],
    read: |a| {
        Ok(ToolCall::DetachCamera {
            clips: a.clip_refs("clips")?,
        })
    },
};
