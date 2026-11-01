//! The animation tool: how a layer comes in, goes out and moves while it shows (`animate`), with a sound from the
//! library as it comes in (or goes out). The words it takes for each (`Entrance`, `Exit`, `Emphasis`) are here;
//! `addText` takes an entrance for its words too. A layer is named by its ref from the context (`t1` words, `i1` a
//! picture, `v1` a video layer) or as `last`, the layer the call before it in the answer added; which layer that is
//! is the compiler's to judge.

use super::args::Args;
use super::error::{Problem, ToolError};
use super::schema::{Word, nullable_string, nullable_words, string};
use super::{Tool, ToolCall};

/// How a layer comes in: a built-in enter preset (`contrib::builtin::MOTION_PRESETS`), or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entrance {
    Pop,
    Fade,
    Zoom,
    Bounce,
    /// Sharpening from a blur (`blur-in`).
    Blur,
    /// Letter by letter: words only.
    Typewriter,
    /// Letters rising in one by one (the kinetic title template): `addText`'s words only.
    Kinetic,
    FromLeft,
    FromRight,
    FromTop,
    FromBottom,
    None,
}

/// How a layer goes out: a built-in exit preset, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Pop,
    Fade,
    Zoom,
    ToLeft,
    ToRight,
    ToTop,
    ToBottom,
    None,
}

/// How a layer moves while it shows: a built-in loop preset (`contrib::builtin::MOTION_LOOPS`), or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emphasis {
    Pulse,
    Shake,
    Wiggle,
    None,
}

/// The layer `animate` names: the one the call before it added, or one by its ref (`t1`, `i1`, `v1`; not yet
/// resolved).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerRef {
    Last,
    One(String),
}

impl Entrance {
    /// The tool's word for it ("fromLeft").
    pub fn key(self) -> &'static str {
        match self {
            Entrance::Pop => "pop",
            Entrance::Fade => "fade",
            Entrance::Zoom => "zoom",
            Entrance::Bounce => "bounce",
            Entrance::Blur => "blur",
            Entrance::Typewriter => "typewriter",
            Entrance::Kinetic => "kinetic",
            Entrance::FromLeft => "fromLeft",
            Entrance::FromRight => "fromRight",
            Entrance::FromTop => "fromTop",
            Entrance::FromBottom => "fromBottom",
            Entrance::None => "none",
        }
    }
}

impl Exit {
    /// The tool's word for it ("toBottom").
    pub fn key(self) -> &'static str {
        match self {
            Exit::Pop => "pop",
            Exit::Fade => "fade",
            Exit::Zoom => "zoom",
            Exit::ToLeft => "toLeft",
            Exit::ToRight => "toRight",
            Exit::ToTop => "toTop",
            Exit::ToBottom => "toBottom",
            Exit::None => "none",
        }
    }
}

impl Emphasis {
    /// The tool's word for it ("pulse").
    pub fn key(self) -> &'static str {
        match self {
            Emphasis::Pulse => "pulse",
            Emphasis::Shake => "shake",
            Emphasis::Wiggle => "wiggle",
            Emphasis::None => "none",
        }
    }
}

impl Word for Entrance {
    fn word(self) -> &'static str {
        self.key()
    }
}

impl Word for Exit {
    fn word(self) -> &'static str {
        self.key()
    }
}

impl Word for Emphasis {
    fn word(self) -> &'static str {
        self.key()
    }
}

/// The entrances, in the schema's order.
pub const ENTRANCES: [Entrance; 12] = [
    Entrance::Pop,
    Entrance::Fade,
    Entrance::Zoom,
    Entrance::Bounce,
    Entrance::Blur,
    Entrance::Typewriter,
    Entrance::Kinetic,
    Entrance::FromLeft,
    Entrance::FromRight,
    Entrance::FromTop,
    Entrance::FromBottom,
    Entrance::None,
];
/// The exits, in the schema's order.
pub const EXITS: [Exit; 8] = [
    Exit::Pop,
    Exit::Fade,
    Exit::Zoom,
    Exit::ToLeft,
    Exit::ToRight,
    Exit::ToTop,
    Exit::ToBottom,
    Exit::None,
];
/// The emphases, in the schema's order.
pub const EMPHASES: [Emphasis; 4] = [
    Emphasis::Pulse,
    Emphasis::Shake,
    Emphasis::Wiggle,
    Emphasis::None,
];

impl Args {
    /// Text, or None when it is null, left out or only spaces.
    pub(super) fn nullable_name(&mut self, key: &str) -> Result<Option<String>, ToolError> {
        match self.take_nullable(key) {
            None => Ok(None),
            Some(value) => {
                self.map.insert(key.to_owned(), value);
                let text = self.label(key)?;
                Ok((!text.is_empty()).then_some(text))
            }
        }
    }
}

pub(super) const ANIMATE: Tool = Tool {
    name: "animate",
    description: "How a layer comes in, goes out and moves while shown, with a sound. Null keeps; none removes.",
    parameters: || {
        vec![
            (
                "layer",
                string("last (added by the call before), or a ref from Layers: t1, i1, v1."),
            ),
            (
                "enter",
                nullable_words(&ENTRANCES, "typewriter: words only. kinetic: addText only."),
            ),
            ("exit", nullable_words(&EXITS, "How it goes out.")),
            ("emphasis", nullable_words(&EMPHASES, "While shown.")),
            (
                "sound",
                nullable_string("A sound from Library, as it comes in."),
            ),
        ]
    },
    read: |a| {
        let layer = a.text("layer")?;
        let layer = if layer.eq_ignore_ascii_case("last") {
            LayerRef::Last
        } else {
            LayerRef::One(layer)
        };
        let enter = a.nullable_word("enter", &ENTRANCES)?;
        let exit = a.nullable_word("exit", &EXITS)?;
        let emphasis = a.nullable_word("emphasis", &EMPHASES)?;
        let sound = a.nullable_name("sound")?;
        if enter.is_none() && exit.is_none() && emphasis.is_none() && sound.is_none() {
            return Err(ToolError {
                tool: a.tool.to_owned(),
                field: None,
                problem: Problem::NoChange {
                    fields: "enter, exit, emphasis or sound",
                },
            });
        }
        Ok(ToolCall::Animate {
            layer,
            enter,
            exit,
            emphasis,
            sound,
        })
    },
};
