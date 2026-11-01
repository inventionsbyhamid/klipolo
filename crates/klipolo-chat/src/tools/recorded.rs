//! The recording tools: how a recording's camera shows (`setCameraMode`), where its bubble sits and how it looks
//! (`adjustCamera`), the camera made a layer of its own (`detachCamera`), its recorded sounds muted, turned up or
//! down, or put on a track of their own (`muteSource`, `setSourceVolume`, `separateSound`), and zooms on its clicks
//! (`autoZoom`). Each is one entry of `TOOLS`. The words they take are the model's own (`CameraMode`, `CameraShape`,
//! `Anchor`, `FormatKind`), the bubble's sizes and steps, and the sounds by kind (`Sound`).
//!
//! Each camera and sound tool takes `clips`, the refs of the clips it changes (`c2`, or `c3-c41` for a run as the
//! context folds it), or null for every clip that has the camera or the sound; whether a ref names a clip is the
//! compiler's to judge. `autoZoom` works on every recording on the main track.

use std::ops::RangeInclusive;

use klipolo_model::layout::Anchor;
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::{CameraMode, CameraShape};
use serde_json::{Value, json};

use super::args::{Args, kind};
use super::error::{Problem, ToolError};
use super::schema::{Word, boolean, nullable_boolean, nullable_number, nullable_words, words};
use super::{FORMATS, Tool, ToolCall};

/// The volumes a recorded sound takes, in percent of how it was recorded: the model's 0 to 200 %.
pub const SOURCE_VOLUMES: RangeInclusive<f64> = 0.0..=200.0;

/// A recorded sound, by what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sound {
    Mic,
    /// The computer's own sound (a system audio source).
    ComputerSound,
    GameSound,
    /// A picture source's own sound: the camera's, where its files carry sound.
    VideoSound,
}

impl Sound {
    /// The tool's word for it ("computerSound").
    pub fn key(self) -> &'static str {
        match self {
            Sound::Mic => "mic",
            Sound::ComputerSound => "computerSound",
            Sound::GameSound => "gameSound",
            Sound::VideoSound => "videoSound",
        }
    }

    /// How the person reads it ("computer sound").
    pub fn name(self) -> &'static str {
        match self {
            Sound::Mic => "mic",
            Sound::ComputerSound => "computer sound",
            Sound::GameSound => "game sound",
            Sound::VideoSound => "camera sound",
        }
    }
}

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

impl BubbleSize {
    /// The tool's word for it ("large").
    pub fn key(self) -> &'static str {
        match self {
            BubbleSize::Small => "small",
            BubbleSize::Medium => "medium",
            BubbleSize::Large => "large",
        }
    }
}

impl SizeStep {
    /// The tool's word for it ("bigger").
    pub fn key(self) -> &'static str {
        match self {
            SizeStep::Bigger => "bigger",
            SizeStep::Smaller => "smaller",
        }
    }
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

/// The sounds the tools offer, in the schema's order.
pub const SOUNDS: [Sound; 4] = [
    Sound::Mic,
    Sound::ComputerSound,
    Sound::GameSound,
    Sound::VideoSound,
];
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

impl Word for Sound {
    fn word(self) -> &'static str {
        self.key()
    }
}

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
        self.key()
    }
}

impl Word for SizeStep {
    fn word(self) -> &'static str {
        self.key()
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

/// The model's word for a format ("portrait").
pub fn format_word(format: FormatKind) -> &'static str {
    format.word()
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

    /// Text, or None when it is null, left out or only spaces.
    fn nullable_text(&mut self, key: &str) -> Result<Option<String>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => {
                self.map.remove(key);
                Ok(None)
            }
            Some(_) => {
                let text = self.label(key)?;
                Ok((!text.is_empty()).then_some(text))
            }
        }
    }

    /// A percent within `bounds`, as the model's gain to two decimals (150 % is 1.5).
    fn percent(&mut self, key: &str, bounds: &RangeInclusive<f64>) -> Result<f64, ToolError> {
        let value = self.take(key)?;
        let n = self.number(key, &value, "a number")?;
        if !bounds.contains(&n) {
            return Err(self.error(
                key,
                Problem::NotWithin {
                    min: *bounds.start(),
                    max: *bounds.end(),
                    got: n,
                },
            ));
        }
        Ok(n.round() / 100.0)
    }
}

/// `clips`, as every recording tool takes it.
fn clips(what: &str) -> (&'static str, Value) {
    (
        "clips",
        json!({
            "type": ["array", "null"],
            "items": {"type": "string"},
            "description": format!("Clip refs. Null for every clip {what}."),
        }),
    )
}

/// `source`, `which` and `clips`, as the sound tools take them.
fn sound(rest: Vec<(&'static str, Value)>) -> Vec<(&'static str, Value)> {
    let mut out = vec![(
        "source",
        words(
            &SOUNDS,
            "computerSound: the system sound. videoSound: the camera's own.",
        ),
    )];
    out.extend(rest);
    out.push((
        "which",
        json!({
            "type": ["string", "null"],
            "description": "2 for mic 2, when a recording has several.",
        }),
    ));
    out.push(clips("with it"));
    out
}

pub(super) const SET_CAMERA_MODE: Tool = Tool {
    name: "setCameraMode",
    description: "Show a recording's camera as a bubble, split under the screen, or hidden.",
    parameters: || {
        vec![
            clips("with a camera"),
            ("mode", words(&CAMERA_MODES, "splitBottom is for vertical.")),
            (
                "format",
                nullable_words(&FORMATS, "Only this format. Null for all."),
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
    description: "Move, resize or restyle a recording's camera bubble. Null keeps a setting.",
    parameters: || {
        vec![
            clips("with a camera"),
            ("position", nullable_words(&ANCHORS, "Where in the frame.")),
            (
                "size",
                nullable_words(&BUBBLE_SIZES, "medium is the usual."),
            ),
            (
                "sizeStep",
                nullable_words(&SIZE_STEPS, "One step from now."),
            ),
            ("shape", nullable_words(&CAMERA_SHAPES, "Its shape.")),
            ("border", nullable_boolean("A border or none.")),
            ("mirror", nullable_boolean("Flipped as in a mirror.")),
            (
                "format",
                nullable_words(&FORMATS, "Only this format. Null for all."),
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
    description: "Make a recording's camera a layer of its own, to move and cut apart.",
    parameters: || vec![clips("with a camera")],
    read: |a| {
        Ok(ToolCall::DetachCamera {
            clips: a.clip_refs("clips")?,
        })
    },
};

pub(super) const MUTE_SOURCE: Tool = Tool {
    name: "muteSource",
    description: "Mute or unmute one recorded sound, everywhere or in some clips.",
    parameters: || sound(vec![("muted", boolean("False unmutes it."))]),
    read: |a| {
        Ok(ToolCall::MuteSource {
            source: a.word("source", &SOUNDS)?,
            muted: a.flag("muted")?,
            which: a.nullable_text("which")?,
            clips: a.clip_refs("clips")?,
        })
    },
};

pub(super) const SET_SOURCE_VOLUME: Tool = Tool {
    name: "setSourceVolume",
    description: "Make one recorded sound louder or quieter, everywhere or in some clips.",
    parameters: || {
        sound(vec![(
            "volume",
            json!({
                "type": "number",
                "description": "0 to 200 percent. 100 is as recorded.",
            }),
        )])
    },
    read: |a| {
        Ok(ToolCall::SetSourceVolume {
            source: a.word("source", &SOUNDS)?,
            volume: a.percent("volume", &SOURCE_VOLUMES)?,
            which: a.nullable_text("which")?,
            clips: a.clip_refs("clips")?,
        })
    },
};

/// The scales `autoZoom` takes: 1 is no zoom.
pub const ZOOM_SCALES: RangeInclusive<f64> = 1.25..=3.0;

pub(super) const AUTO_ZOOM: Tool = Tool {
    name: "autoZoom",
    description: "Zoom in on the clicks of the screen recordings, or take those zooms away.",
    parameters: || {
        vec![
            ("on", boolean("False takes the auto zooms away.")),
            ("scale", nullable_number("How far, 1.25 to 3. Null for 2.")),
        ]
    },
    read: |a| {
        Ok(ToolCall::AutoZoom {
            on: a.flag("on")?,
            scale: a.nullable_number("scale", &ZOOM_SCALES)?,
        })
    },
};

pub(super) const SEPARATE_SOUND: Tool = Tool {
    name: "separateSound",
    description: "Put one recorded sound on a track of its own, to cut apart.",
    parameters: || sound(Vec::new()),
    read: |a| {
        Ok(ToolCall::SeparateSound {
            source: a.word("source", &SOUNDS)?,
            which: a.nullable_text("which")?,
            clips: a.clip_refs("clips")?,
        })
    },
};
