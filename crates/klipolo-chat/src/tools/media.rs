//! The media tools: music, sound effects, words and pictures on screen from the library (`addMusic`, `addSound`,
//! `addText`, `addImage`), and pictures Muse Image draws first (`makeSticker`, `makePicture`, `makeTitleCard`). Each is one entry of
//! `TOOLS`; the words they take for places, sizes and text presets are here.
//!
//! A time left null is the playhead, a length left null the default (`TEXT_SECS`, `PICTURE_SECS`, `TEMPLATE_SECS`);
//! whether a time stands on the context is the compiler's to judge, as for every tool.

use std::ops::RangeInclusive;

use klipolo_model::media::image::IMAGE_US;
use klipolo_model::media::music::MUSIC_VOLUME;
use klipolo_model::media::text::{TEMPLATE_US, TEXT_US};

use super::animate::ENTRANCES;
use super::schema::{
    Word, nullable_boolean, nullable_list, nullable_number, nullable_string, nullable_words,
    seconds, string, words,
};
use super::{Problem, Tool, ToolCall};

/// How long words show when the person gave no length: the model's text preset length (`media::text::TEXT_US`).
pub const TEXT_SECS: f64 = TEXT_US as f64 / 1e6;
/// How long a picture shows when the person gave no length (`media::image::IMAGE_US`).
pub const PICTURE_SECS: f64 = IMAGE_US as f64 / 1e6;
/// How long the title card template shows when the person gave no length: its scene's own length, its words coming
/// in and going out included (`media::text::TEMPLATE_US`).
pub const TEMPLATE_SECS: f64 = TEMPLATE_US as f64 / 1e6;
/// The lengths a word or picture layer takes, in seconds.
pub const OVERLAY_SECS: RangeInclusive<f64> = 0.5..=600.0;
/// The volumes music takes: 1 is the file as it is.
pub const MUSIC_VOLUMES: RangeInclusive<f64> = 0.0..=1.0;

/// A text preset: where words sit and how big they are (the constraints' four). The full-frame title card template is
/// `makeTitleCard`'s, not one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPreset {
    /// Big words at the top centre.
    Title,
    /// At the bottom left on a dark box.
    LowerThird,
    /// In the middle.
    Plain,
    /// At the bottom centre, where captions go.
    Caption,
}

impl TextPreset {
    /// The model's preset it is: the one the canvas's Add title and the library's text presets place.
    pub fn preset(self) -> klipolo_model::media::text::TextPreset {
        use klipolo_model::media::text::TextPreset as Model;
        match self {
            TextPreset::Title => Model::Title,
            TextPreset::LowerThird => Model::LowerThird,
            TextPreset::Plain => Model::Plain,
            TextPreset::Caption => Model::Caption,
        }
    }

    /// The tool's word for it ("lower-third").
    pub fn key(self) -> &'static str {
        match self {
            TextPreset::Title => "title",
            TextPreset::LowerThird => "lower-third",
            TextPreset::Plain => "plain",
            TextPreset::Caption => "caption",
        }
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

impl Position {
    /// The model's place for it (`media::image::Spot`).
    pub fn spot(self) -> klipolo_model::media::image::Spot {
        use klipolo_model::media::image::Spot;
        match self {
            Position::TopLeft => Spot::TopLeft,
            Position::TopRight => Spot::TopRight,
            Position::BottomLeft => Spot::BottomLeft,
            Position::BottomRight => Spot::BottomRight,
            Position::Center => Spot::Center,
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

/// The text presets `addText` offers.
pub const TEXT_PRESETS: [TextPreset; 4] = [
    TextPreset::Title,
    TextPreset::LowerThird,
    TextPreset::Plain,
    TextPreset::Caption,
];
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

/// How a picture Muse Image draws to explain a moment shows (`makePicture`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureLayout {
    /// Over the whole frame for its time, as a cutaway does.
    Full,
    /// In a box in a corner, clear of the face and the captions.
    Inset,
}

impl PictureLayout {
    /// The tool's word for it ("full").
    pub fn key(self) -> &'static str {
        match self {
            PictureLayout::Full => "full",
            PictureLayout::Inset => "inset",
        }
    }
}

/// The layouts a drawn picture takes.
pub const PICTURE_LAYOUTS: [PictureLayout; 2] = [PictureLayout::Full, PictureLayout::Inset];

/// How long a drawn picture shows when no length is given: long enough to take in, short enough to keep the talk going.
pub const DRAWN_PICTURE_SECS: f64 = 3.0;

impl Word for PictureLayout {
    fn word(self) -> &'static str {
        self.key()
    }
}

impl Word for TextPreset {
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
    ("atSec", nullable_number("Start. Null for the playhead."))
}

/// What a quote of the words said at a moment is (`atWords`).
const QUOTE: &str = "1 to 4 words said there, exactly as in Transcript";

/// `atSec` beside `atWords`.
fn at_sec_or_words() -> (&'static str, serde_json::Value) {
    (
        "atSec",
        nullable_number("Start. With atWords, picks where they are said. Both null: the playhead."),
    )
}

fn at_words() -> (&'static str, serde_json::Value) {
    (
        "atWords",
        nullable_string(&format!("Or {QUOTE}; it starts on the first.")),
    )
}

fn duration_sec(default_secs: f64) -> (&'static str, serde_json::Value) {
    (
        "durationSec",
        nullable_number(&format!("Length. Null for {default_secs} s.")),
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
    description: "Play music from Library to the video's end, fading in and out.",
    parameters: || {
        vec![
            ("file", string("The music's name in Library.")),
            (
                "volume",
                nullable_number(&format!("0 to 1. Null for {MUSIC_VOLUME}.")),
            ),
            (
                "duck",
                nullable_boolean(
                    "Quieter while people speak; false keeps it level. Null for true.",
                ),
            ),
            (
                "atSec",
                nullable_number("Start. Null for 0.0, as background music."),
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
    description: "Play a sound effect from Library at each moment given.",
    parameters: || {
        vec![
            ("name", string("The sound's name in Library.")),
            (
                "atWords",
                nullable_list(
                    string(QUOTE),
                    "Each moment it plays: where the first word is said.",
                ),
            ),
            (
                "atSec",
                nullable_list(
                    seconds("A time it plays, timeline seconds."),
                    "Or times, from the context. With atWords, they pick where each is said.",
                ),
            ),
        ]
    },
    read: |a| {
        let name = a.text("name")?;
        let at_words = a.nullable_texts("atWords")?;
        let at_us = a.nullable_times("atSec")?;
        if at_words.is_empty() && at_us.is_empty() {
            return Err(a.error(
                "atWords",
                Problem::NoChange {
                    fields: "atWords or atSec",
                },
            ));
        }
        Ok(ToolCall::AddSound {
            name,
            at_us,
            at_words,
        })
    },
};

pub(super) const ADD_TEXT: Tool = Tool {
    name: "addText",
    description: "Show words over the video in a text preset, at once and free: a title, a name, a caption. For a \
                  full-screen title card, use makeTitleCard. A short's hook: at most 6 punchy words.",
    parameters: || {
        vec![
            ("text", string("The words, as the person wrote them.")),
            (
                "preset",
                words(
                    &TEXT_PRESETS,
                    "title: big, at the top. lower-third: a name or label at the bottom left. plain: in the middle. \
                     caption: at the bottom.",
                ),
            ),
            at_sec_or_words(),
            at_words(),
            duration_sec(TEXT_SECS),
            ("animation", nullable_words(&ENTRANCES, "Null: fade.")),
            (
                "font",
                nullable_string("A font by name, e.g. Poppins; null: the preset's."),
            ),
        ]
    },
    read: |a| {
        Ok(ToolCall::AddText {
            text: a.shown("text")?,
            preset: a.word("preset", &TEXT_PRESETS)?,
            at_us: a.nullable_time("atSec")?,
            at_words: a.nullable_quote("atWords")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
            animation: a.nullable_word("animation", &ENTRANCES)?,
            font: a.optional_text("font")?,
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
            at_sec_or_words(),
            at_words(),
            duration_sec(PICTURE_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::AddImage {
            file: a.text("file")?,
            size: a.nullable_word("size", &SIZES)?,
            position: a.nullable_word("position", &POSITIONS)?,
            at_us: a.nullable_time("atSec")?,
            at_words: a.nullable_quote("atWords")?,
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
            at_sec_or_words(),
            at_words(),
            duration_sec(PICTURE_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::MakeSticker {
            prompt: a.text("prompt")?,
            position: a.nullable_word("position", &POSITIONS)?,
            at_us: a.nullable_time("atSec")?,
            at_words: a.nullable_quote("atWords")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
        })
    },
};

pub(super) const MAKE_PICTURE: Tool = Tool {
    name: "makePicture",
    description: "Have Muse Image draw a picture that explains what is said at a moment (a diagram, a scene, an \
                  example), shown once drawn: full covers the frame for its time, inset sits in a corner box.",
    parameters: || {
        vec![
            (
                "prompt",
                string("What it shows, a short phrase such as a fish population over 50 years."),
            ),
            (
                "layout",
                words(
                    &PICTURE_LAYOUTS,
                    "full over the frame, or inset in a corner.",
                ),
            ),
            at_sec_or_words(),
            at_words(),
            duration_sec(DRAWN_PICTURE_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::MakePicture {
            prompt: a.text("prompt")?,
            layout: a.word("layout", &PICTURE_LAYOUTS)?,
            at_us: a.nullable_time("atSec")?,
            at_words: a.nullable_quote("atWords")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
        })
    },
};

pub(super) const MAKE_TITLE_CARD: Tool = Tool {
    name: "makeTitleCard",
    description: "A full-screen title card: the person's title over a background Muse Image draws, with no lettering \
                  in the picture. Only when the person asks for a title card or a picture behind the title; for words \
                  over the video, use addText.",
    parameters: || {
        vec![
            ("title", string("The title, the person's exact words.")),
            (
                "prompt",
                string(
                    "What the background shows, a short phrase. If the person named no background, choose one that \
                     fits the title and the video; never ask for it.",
                ),
            ),
            at_sec(),
            duration_sec(TEMPLATE_SECS),
        ]
    },
    read: |a| {
        Ok(ToolCall::MakeTitleCard {
            title: a.shown("title")?,
            prompt: a.text("prompt")?,
            at_us: a.nullable_time("atSec")?,
            duration_us: a.nullable_length("durationSec", &OVERLAY_SECS)?,
        })
    },
};
