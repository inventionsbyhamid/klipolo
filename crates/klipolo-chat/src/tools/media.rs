//! The media tools: music, sound effects, words and pictures on screen from the library (`addMusic`, `addSound`,
//! `addText`, `addImage`), and pictures Muse Image draws first (`makeSticker`, `makeTitleCard`). Each is one entry of
//! `TOOLS`; the words they take for places, sizes and title templates are here.
//!
//! A time left null is the playhead, a length left null the default (`TEXT_SECS`, `PICTURE_SECS`); whether a time
//! stands on the context is the compiler's to judge, as for every tool.

use std::ops::RangeInclusive;

use klipolo_model::contrib::ContribId;
use klipolo_model::media::music::MUSIC_VOLUME;

use super::schema::{
    Word, list, nullable_boolean, nullable_number, nullable_seconds, nullable_words, seconds,
    string, words,
};
use super::{Tool, ToolCall};

/// How long words show when the person gave no length.
pub const TEXT_SECS: f64 = 3.0;
/// How long a picture shows when the person gave no length.
pub const PICTURE_SECS: f64 = 5.0;
/// The lengths a word or picture layer takes, in seconds.
pub const OVERLAY_SECS: RangeInclusive<f64> = 0.5..=600.0;
/// The volumes music takes: 1 is the file as it is.
pub const MUSIC_VOLUMES: RangeInclusive<f64> = 0.0..=1.0;

/// A title template: one of the scene presets (`klipolo_model::presets::SCENE_PRESETS`), which fill the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlePreset {
    /// A name and a role at the bottom left.
    LowerThird,
    /// A big centred title.
    TitleCard,
}

impl TitlePreset {
    /// The tool's word for it, the scene preset's local id after `scene.` ("title-card").
    pub fn key(self) -> &'static str {
        match self {
            TitlePreset::LowerThird => "lower-third",
            TitlePreset::TitleCard => "title-card",
        }
    }

    /// The scene preset it shows.
    pub fn source(self) -> ContribId {
        ContribId::builtin(match self {
            TitlePreset::LowerThird => "scene.lower-third",
            TitlePreset::TitleCard => "scene.title-card",
        })
    }
}

/// Where a picture sits in the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

/// How big a picture is: a share of the frame's width, its shape kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureSize {
    Small,
    Medium,
    Large,
}

impl Position {
    /// The tool's word for it ("top-right").
    pub fn key(self) -> &'static str {
        match self {
            Position::TopLeft => "top-left",
            Position::TopRight => "top-right",
            Position::BottomLeft => "bottom-left",
            Position::BottomRight => "bottom-right",
            Position::Center => "center",
        }
    }
}

impl PictureSize {
    /// The tool's word for it ("medium").
    pub fn key(self) -> &'static str {
        match self {
            PictureSize::Small => "small",
            PictureSize::Medium => "medium",
            PictureSize::Large => "large",
        }
    }

    /// Its share of the frame's width.
    pub fn share(self) -> f64 {
        match self {
            PictureSize::Small => 0.2,
            PictureSize::Medium => 0.35,
            PictureSize::Large => 0.5,
        }
    }
}

/// The title templates `addText` offers, in `SCENE_PRESETS`' order.
pub const TITLE_PRESETS: [TitlePreset; 2] = [TitlePreset::LowerThird, TitlePreset::TitleCard];
/// The places a picture takes.
pub const POSITIONS: [Position; 5] = [
    Position::TopLeft,
    Position::TopRight,
    Position::BottomLeft,
    Position::BottomRight,
    Position::Center,
];
/// The sizes a picture takes.
pub const SIZES: [PictureSize; 3] = [PictureSize::Small, PictureSize::Medium, PictureSize::Large];

impl Word for TitlePreset {
    fn word(self) -> &'static str {
        self.key()
    }
}

impl Word for Position {
    fn word(self) -> &'static str {
        self.key()
    }
}

impl Word for PictureSize {
    fn word(self) -> &'static str {
        self.key()
    }
}

fn at_sec() -> (&'static str, serde_json::Value) {
    (
        "atSec",
        nullable_seconds("When it starts, timeline seconds. Null for the playhead."),
    )
}

fn duration_sec(default_secs: f64) -> (&'static str, serde_json::Value) {
    (
        "durationSec",
        nullable_seconds(&format!("How long it shows. Null for {default_secs} s.")),
    )
}

fn position() -> (&'static str, serde_json::Value) {
    (
        "position",
        nullable_words(&POSITIONS, "Where in the frame. Null for center."),
    )
}

pub(super) const ADD_MUSIC: Tool = Tool {
    name: "addMusic",
    description: "Play music from Library to the video's end, fading in and out, quieter while someone speaks unless \
                  duck is false.",
    parameters: || {
        vec![
            ("file", string("The music's name in Library.")),
            (
                "volume",
                nullable_number(&format!("0 to 1. Null for {MUSIC_VOLUME}, under a voice.")),
            ),
            (
                "duck",
                nullable_boolean("False keeps it level while people speak."),
            ),
            (
                "atSec",
                nullable_seconds(
                    "Where it starts. Null for the playhead; 0.0 for background music.",
                ),
            ),
        ]
    },
    read: |a| {
        Ok(ToolCall::AddMusic {
            file: a.text("file")?,
            volume: a.nullable_number("volume", &MUSIC_VOLUMES)?,
            duck: a.nullable_flag("duck")?,
            at_us: a.nullable_time("atSec")?,
        })
    },
};

pub(super) const ADD_SOUND: Tool = Tool {
    name: "addSound",
    description: "Play a sound effect from Library at each time given.",
    parameters: || {
        vec![
            ("name", string("The sound's name in Library.")),
            (
                "atSec",
                list(
                    seconds("A time it plays, timeline seconds."),
                    "Every time it plays, from the context.",
                ),
            ),
        ]
    },
    read: |a| {
        Ok(ToolCall::AddSound {
            name: a.text("name")?,
            at_us: a.times("atSec")?,
        })
    },
};

pub(super) const ADD_TEXT: Tool = Tool {
    name: "addText",
    description: "Show words on screen in a title template.",
    parameters: || {
        vec![
            ("text", string("The words, as the person wrote them.")),
            (
                "preset",
                words(
                    &TITLE_PRESETS,
                    "lower-third: a name or label at the bottom left. title-card: a big centred title.",
                ),
            ),
            at_sec(),
            duration_sec(TEXT_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::AddText {
            text: a.text("text")?,
            preset: a.word("preset", &TITLE_PRESETS)?,
            at_us: a.nullable_time("atSec")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
        })
    },
};

pub(super) const ADD_IMAGE: Tool = Tool {
    name: "addImage",
    description: "Show a picture from Library over the video: the person's images or a pack sticker.",
    parameters: || {
        vec![
            ("file", string("The picture's name in Library.")),
            (
                "size",
                nullable_words(&SIZES, "A share of the frame's width. Null for medium."),
            ),
            position(),
            at_sec(),
            duration_sec(PICTURE_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::AddImage {
            file: a.text("file")?,
            size: a.nullable_word("size", &SIZES)?,
            position: a.nullable_word("position", &POSITIONS)?,
            at_us: a.nullable_time("atSec")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
        })
    },
};

pub(super) const MAKE_STICKER: Tool = Tool {
    name: "makeSticker",
    description: "Have Muse Image draw a new sticker when no picture in Library fits. It is placed once drawn.",
    parameters: || {
        vec![
            (
                "prompt",
                string("What it shows, a short noun phrase such as paper plane."),
            ),
            position(),
            at_sec(),
            duration_sec(PICTURE_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::MakeSticker {
            prompt: a.text("prompt")?,
            position: a.nullable_word("position", &POSITIONS)?,
            at_us: a.nullable_time("atSec")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
        })
    },
};

pub(super) const MAKE_TITLE_CARD: Tool = Tool {
    name: "makeTitleCard",
    description: "A title card over a background Muse Image draws, with no lettering in the picture. Only when the \
                  person asks for a picture behind their title.",
    parameters: || {
        vec![
            ("title", string("The title, the person's exact words.")),
            (
                "prompt",
                string("What the background shows, a short phrase."),
            ),
            at_sec(),
            duration_sec(TEXT_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::MakeTitleCard {
            title: a.text("title")?,
            prompt: a.text("prompt")?,
            at_us: a.nullable_time("atSec")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
        })
    },
};
