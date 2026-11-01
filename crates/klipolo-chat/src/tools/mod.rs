//! The tools the language model may call to edit, and its calls read into typed edits.
//!
//! `tools()` lists every tool as provider-neutral JSON, `[{name, description, parameters}]`, where `parameters` is a
//! strict JSON Schema: every object closed (`additionalProperties: false`) with every property required, and a value
//! the person may leave unsaid nullable (`"type": ["number", "null"]`). A provider's adapter wraps each tool in its own
//! wire shape (the Responses API's `{"type": "function", "strict": true, ...}`). The list is built the same way and in
//! the same order every time, so a provider's prompt cache can reuse it. The tools keep their order; a schema's
//! `properties` read by name (serde_json sorts an object's keys), and its `required` list keeps the order given.
//!
//! `ToolCall::parse` reads one `FunctionCall` (the tool's name and the JSON text of its arguments) into a `ToolCall`:
//! times become timeline microseconds, words become the model's own types (`FormatKind`, `Edge`,
//! `BuiltinCaptionStyle`). Only the shape is checked here (types, words, bounds, an end after its start); whether a
//! time holds a word or a scene, or a ref a clip, is the compiler's to judge against the project. A call that doesn't
//! fit is a `ToolError` naming the tool and the field, whose text is one plain sentence the planner can send back to
//! the model. Where the meaning is clear it reads leniently: no arguments at all for a tool that takes none, a nullable
//! value left out, text with stray spaces.
//!
//! Adding a tool is one `ToolCall` variant and one entry in `TOOLS`, where its name, description, parameters and reader
//! sit together; the tests check every schema is strict and every reader reads the fields its schema lists. Splitting
//! by concern: `schema` (the pieces of a schema, and the words a tool takes), `args` (a call's arguments read field by
//! field), `error` (why a call couldn't be read), `media` (the media tools and their words), `clips` (the clip and
//! transition tools and their words), `background` (the background tool and its words), `recorded` (a recording's
//! camera and recorded sounds, and their words), `animate` (a layer's animation and its words).

pub mod animate;
mod args;
pub mod background;
pub mod clips;
pub mod effects;
mod error;
pub mod layer;
pub mod media;
pub mod motion;
pub mod recorded;
mod schema;
mod sets;
pub mod style;
pub mod text_motion;

use std::ops::RangeInclusive;

use klipolo_model::captions::styles::BuiltinCaptionStyle;
use klipolo_model::ops::Edge;
use klipolo_model::project::FormatKind;
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{CameraMode, FramingMode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub use animate::{Emphasis, Entrance, Exit, LayerRef};
use args::{Args, kind};
pub use background::BackgroundChoice;
pub use clips::{ClipRef, TransitionKind};
pub use effects::{EffectChange, SettingValue};
pub use error::{Problem, ToolError};
pub use layer::{LayerChange, Order};
pub use media::{PictureSize, Position, TextPreset};
pub use motion::{KeyIn, PresetIn, Property, Slot};
pub use recorded::{BubbleChange, BubbleSize, SizeStep, Sound};
use schema::{boolean, list, nullable_number, nullable_words, object, seconds, string, words};
pub use style::{Look, OutlineStep, Paint, StyleChange};

/// One tool call as the model made it, whichever provider ran the model: its adapter maps the provider's call into
/// this.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// The tool called.
    pub name: String,
    /// The provider's id for this call, sent back with the call's result.
    pub call_id: String,
    /// The arguments, as the JSON text the model wrote.
    pub arguments: String,
}

/// A moment `findMoments` points at: where, in timeline time, and a few words on what happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moment {
    pub range: RangeUs,
    /// May be empty.
    pub label: String,
}

/// What the model asked for, its arguments read and their shape checked. Times are timeline microseconds, from
/// the timeline seconds the model writes.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolCall {
    /// Remove `range`. A removal: proposed, never applied at once.
    CutRange { range: RangeUs },
    /// Keep only `ranges` (at least one, as given: they may overlap or come in any order) and remove the rest. A
    /// removal.
    KeepOnly { ranges: Vec<RangeUs> },
    /// Remove pauses in speech longer than `min_pause_us` (within `PAUSE_SECS`); None when the person gave no length.
    /// A removal.
    RemoveSilences { min_pause_us: Option<Us> },
    /// Remove filler words. A removal.
    RemoveFillers,
    /// Move `edge` of the clip that `clip` names (a ref from the context, `c2`, not yet resolved) to `to_us`. Moving
    /// it inward is a removal; outward, an addition.
    TrimClip { clip: String, edge: Edge, to_us: Us },
    /// Split the clip playing at `at_us` in two.
    SplitAt { at_us: Us },
    /// Hold the frame shown at `at_us` (None: the playhead) still for `duration_us` (None: `FREEZE_DEFAULT_US`).
    FreezeFrame {
        at_us: Option<Us>,
        duration_us: Option<Us>,
    },
    /// Set the clip `clip` names (or every clip) to play at `speed` (one of `clip_settings::SPEEDS`), at `volume`
    /// (`clip_settings::is_volume`), muted or not, and framed `framing.0` in the format `framing.1` (None: every
    /// format the video has); None leaves each as it is, and at least one is set. Speed, volume and mute are the clip
    /// card's; it is an addition, unless the new speed leaves a transition no room.
    SetClip {
        clip: ClipRef,
        speed: Option<f64>,
        volume: Option<f64>,
        muted: Option<bool>,
        framing: Option<(FramingMode, Option<FormatKind>)>,
    },
    /// Give the cut after the clip `after` names (or every cut) a transition of `kind` lasting `duration_us` (within
    /// `clips::TRANSITION_SECS`; None: the default for a new one, its own length for one already there), or take it
    /// away (`kind` None, with no length). Taking one away is a removal.
    SetTransition {
        after: ClipRef,
        kind: Option<TransitionKind>,
        duration_us: Option<Us>,
    },
    /// Set what shows behind the video where it doesn't fill the frame (`kind`, its colours), and the inset round it
    /// (`padding`, a share of the frame's shorter side); None leaves each as the project has it, and at least one is
    /// set. The background card's fields, but for a picture.
    SetBackground {
        kind: Option<BackgroundChoice>,
        color: Option<String>,
        color_end: Option<String>,
        padding: Option<f64>,
    },
    /// Make `format` the main format, keeping the others too when `keep_others`.
    SetFormat {
        format: FormatKind,
        keep_others: bool,
    },
    /// Show or hide captions; `style` None keeps the current style (or the formats' default when there is none).
    SetCaptions {
        on: bool,
        style: Option<BuiltinCaptionStyle>,
    },
    /// Make every shown word matching `find` read `replace` in captions and the transcript.
    FixCaptionWords { find: String, replace: String },
    /// Point the person at `moments` (none when nothing matched `query`); the edit doesn't change.
    FindMoments { query: String, moments: Vec<Moment> },
    /// Keep the crop on the one subject `description` names (a short noun phrase).
    FollowSubject { description: String },
    /// Blur the one kind of thing `description` names (a short noun phrase) wherever it shows.
    BlurObjects { description: String },
    /// Play the library's music `file` (a name, not yet resolved) from `at_us` (None: the playhead) to the video's
    /// end; `volume` None is the default, `duck` None is on.
    AddMusic {
        file: String,
        volume: Option<f64>,
        duck: Option<bool>,
        at_us: Option<Us>,
    },
    /// Play the library's sound `name` at each of `at_us` (at least one).
    AddSound { name: String, at_us: Vec<Us> },
    /// Show `text` in text preset `preset` from `at_us` (None: the playhead) for `duration_us` (None: the default),
    /// coming in as `animation` says (None: a fade).
    AddText {
        text: String,
        preset: TextPreset,
        at_us: Option<Us>,
        duration_us: Option<Us>,
        animation: Option<Entrance>,
        /// A font by name; None: the preset's.
        font: Option<String>,
    },
    /// Show the library's picture `file`; None for what the person left unsaid.
    AddImage {
        file: String,
        size: Option<PictureSize>,
        position: Option<Position>,
        at_us: Option<Us>,
        duration_us: Option<Us>,
    },
    /// Have a sticker of `prompt` drawn, to place once it exists.
    MakeSticker {
        prompt: String,
        position: Option<Position>,
        at_us: Option<Us>,
        duration_us: Option<Us>,
    },
    /// Show `title` in the full-frame title card template, over a background of `prompt` drawn for it.
    MakeTitleCard {
        title: String,
        prompt: String,
        at_us: Option<Us>,
        duration_us: Option<Us>,
    },
    /// Set how the layer `layer` names comes in, goes out and moves while it shows (None keeps each, a `None` word takes
    /// it away), with the library's sound `sound` (a name, not yet resolved) as it comes in, or goes out when only its
    /// exit is set.
    Animate {
        layer: LayerRef,
        enter: Option<Entrance>,
        exit: Option<Exit>,
        emphasis: Option<Emphasis>,
        sound: Option<String>,
    },
    /// Show the cameras of `clips` (refs from the context, not yet resolved; None: every clip with one) as `mode`, in
    /// `format` alone or in every format.
    SetCameraMode {
        clips: Option<Vec<String>>,
        mode: CameraMode,
        format: Option<FormatKind>,
    },
    /// Move, resize or restyle the camera bubbles of `clips` (None: every clip with one).
    AdjustCamera {
        clips: Option<Vec<String>>,
        change: BubbleChange,
    },
    /// Make the cameras of `clips` (None: every clip with one) layers of their own.
    DetachCamera { clips: Option<Vec<String>> },
    /// Mute or unmute the recorded `source` (the one `which` names when a recording has several, None: all of them) in
    /// `clips`, or everywhere when None.
    MuteSource {
        source: Sound,
        muted: bool,
        which: Option<String>,
        clips: Option<Vec<String>>,
    },
    /// Play the recorded `source` at `volume` (the model's gain: 1 is as recorded, at most 2), as `MuteSource` names it.
    SetSourceVolume {
        source: Sound,
        volume: f64,
        which: Option<String>,
        clips: Option<Vec<String>>,
    },
    /// Put the recorded `source` of `clips` (None: every clip that plays it) on a track of its own.
    SeparateSound {
        source: Sound,
        which: Option<String>,
        clips: Option<Vec<String>>,
    },
    /// Zoom in on the clicks of every recording on the main track, at most `scale` (None: the camera's own most), or
    /// take the auto zooms over them away (`on` false).
    AutoZoom { on: bool, scale: Option<f64> },
    /// Give the layer `layer` names keys (each property listed in `keys` gets exactly those keys, the others keep
    /// theirs; `Some(vec![])` takes every key away; None keeps them), and set or take away its enter, exit and loop
    /// presets.
    SetMotion {
        layer: LayerRef,
        keys: Option<Vec<KeyIn>>,
        presets: Vec<PresetIn>,
    },
    /// Bring the words `layer` names in by `motion.unit`, each from `motion.from`.
    AnimateText {
        layer: LayerRef,
        motion: klipolo_model::media::text_motion::TextMotion,
    },
    /// Place, size, turn, fade, blend, round, crop, frame or reorder the layer `layer` names, or set the fades of the
    /// clip or song it names and the song's ducking.
    SetLayer {
        layer: LayerRef,
        change: Box<LayerChange>,
    },
    /// Restyle the words `layer` names.
    StyleText {
        layer: LayerRef,
        change: Box<StyleChange>,
    },
    /// Add, change, move or take off effects on the clip or layer `target` names.
    SetEffects {
        target: LayerRef,
        effects: Vec<EffectChange>,
    },
    /// Undo the last change.
    Undo,
    /// Ask the person `question` before doing anything more.
    AskUser { question: String },
    /// Tell the person Klipolo can't do what they asked, and why.
    CannotDo { reason: String },
}

impl ToolCall {
    /// Reads `call`: its tool by name, then its arguments as that tool's schema lists them.
    pub fn parse(call: &FunctionCall) -> Result<ToolCall, ToolError> {
        let Some(tool) = TOOLS.iter().find(|t| t.name == call.name) else {
            return Err(ToolError {
                tool: call.name.clone(),
                field: None,
                problem: Problem::UnknownTool,
            });
        };
        let whole = |problem| ToolError {
            tool: tool.name.to_owned(),
            field: None,
            problem,
        };
        let text = call.arguments.trim();
        let value = if text.is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str(text).map_err(|_| whole(Problem::NotJson))?
        };
        let Value::Object(map) = value else {
            return Err(whole(Problem::WrongType {
                expected: "an object",
                got: kind(&value),
            }));
        };
        let mut args = Args {
            tool: tool.name,
            path: String::new(),
            map,
        };
        let parsed = (tool.read)(&mut args)?;
        args.finish()?;
        Ok(parsed)
    }

    /// The tool's name, as the model calls it.
    pub fn name(&self) -> &'static str {
        match self {
            ToolCall::CutRange { .. } => "cutRange",
            ToolCall::KeepOnly { .. } => "keepOnly",
            ToolCall::RemoveSilences { .. } => "removeSilences",
            ToolCall::RemoveFillers => "removeFillers",
            ToolCall::TrimClip { .. } => "trimClip",
            ToolCall::SplitAt { .. } => "splitAt",
            ToolCall::FreezeFrame { .. } => "freezeFrame",
            ToolCall::SetClip { .. } => "setClip",
            ToolCall::SetTransition { .. } => "setTransition",
            ToolCall::SetBackground { .. } => "setBackground",
            ToolCall::SetFormat { .. } => "setFormat",
            ToolCall::SetCaptions { .. } => "setCaptions",
            ToolCall::FixCaptionWords { .. } => "fixCaptionWords",
            ToolCall::FindMoments { .. } => "findMoments",
            ToolCall::FollowSubject { .. } => "followSubject",
            ToolCall::BlurObjects { .. } => "blurObjects",
            ToolCall::AddMusic { .. } => "addMusic",
            ToolCall::AddSound { .. } => "addSound",
            ToolCall::AddText { .. } => "addText",
            ToolCall::AddImage { .. } => "addImage",
            ToolCall::MakeSticker { .. } => "makeSticker",
            ToolCall::MakeTitleCard { .. } => "makeTitleCard",
            ToolCall::Animate { .. } => "animate",
            ToolCall::SetCameraMode { .. } => "setCameraMode",
            ToolCall::AdjustCamera { .. } => "adjustCamera",
            ToolCall::DetachCamera { .. } => "detachCamera",
            ToolCall::MuteSource { .. } => "muteSource",
            ToolCall::SetSourceVolume { .. } => "setSourceVolume",
            ToolCall::SeparateSound { .. } => "separateSound",
            ToolCall::AutoZoom { .. } => "autoZoom",
            ToolCall::SetMotion { .. } => "setMotion",
            ToolCall::AnimateText { .. } => "animateText",
            ToolCall::SetLayer { .. } => "setLayer",
            ToolCall::StyleText { .. } => "styleText",
            ToolCall::SetEffects { .. } => "setEffects",
            ToolCall::Undo => "undo",
            ToolCall::AskUser { .. } => "askUser",
            ToolCall::CannotDo { .. } => "cannotDo",
        }
    }

    /// Whether the call names a song, a video layer, words or a picture by its ref (`m1`, `v1`, `t1`, `i1`), which holds
    /// only while the layers stay as the context named them (`context::layers_key`).
    pub fn names_layer(&self) -> bool {
        match self {
            ToolCall::SetClip {
                clip: ClipRef::One(r),
                ..
            } => r.starts_with(['m', 'v']),
            ToolCall::Animate { layer, .. }
            | ToolCall::SetMotion { layer, .. }
            | ToolCall::AnimateText { layer, .. }
            | ToolCall::StyleText { layer, .. } => matches!(layer, LayerRef::One(_)),
            ToolCall::SetLayer { layer: target, .. } | ToolCall::SetEffects { target, .. } => {
                matches!(target, LayerRef::One(r) if !r.starts_with('c'))
            }
            _ => false,
        }
    }

    /// Whether the call names a timeline time or a clip ref, which hold only while the main track's clips stay where
    /// the context put them (`context::timeline_key`): a range, a split, a trim, a moment, a clip tool's ref (not
    /// `all`), a media tool's start when it gives one (None is the playhead), or a recording tool's clips when it names
    /// them (None is every clip). Every tool is listed, so a new one must say.
    pub fn names_timeline(&self) -> bool {
        match self {
            ToolCall::CutRange { .. }
            | ToolCall::KeepOnly { .. }
            | ToolCall::TrimClip { .. }
            | ToolCall::SplitAt { .. }
            | ToolCall::FindMoments { .. }
            | ToolCall::AddSound { .. } => true,
            ToolCall::SetClip { clip: target, .. }
            | ToolCall::SetTransition { after: target, .. } => *target != ClipRef::All,
            ToolCall::AddMusic { at_us, .. }
            | ToolCall::AddText { at_us, .. }
            | ToolCall::AddImage { at_us, .. }
            | ToolCall::MakeSticker { at_us, .. }
            | ToolCall::MakeTitleCard { at_us, .. }
            | ToolCall::FreezeFrame { at_us, .. } => at_us.is_some(),
            ToolCall::SetCameraMode { clips, .. }
            | ToolCall::AdjustCamera { clips, .. }
            | ToolCall::DetachCamera { clips }
            | ToolCall::MuteSource { clips, .. }
            | ToolCall::SetSourceVolume { clips, .. }
            | ToolCall::SeparateSound { clips, .. } => clips.is_some(),
            ToolCall::SetLayer { layer, change } => {
                matches!(layer, LayerRef::One(r) if r.starts_with('c')) || change.start_us.is_some()
            }
            ToolCall::SetEffects { target, .. } => {
                matches!(target, LayerRef::One(r) if r.starts_with('c'))
            }
            ToolCall::RemoveSilences { .. }
            | ToolCall::RemoveFillers
            | ToolCall::SetFormat { .. }
            | ToolCall::SetCaptions { .. }
            | ToolCall::SetBackground { .. }
            | ToolCall::FixCaptionWords { .. }
            | ToolCall::FollowSubject { .. }
            | ToolCall::BlurObjects { .. }
            | ToolCall::Animate { .. }
            | ToolCall::AutoZoom { .. }
            | ToolCall::SetMotion { .. }
            | ToolCall::AnimateText { .. }
            | ToolCall::StyleText { .. }
            | ToolCall::Undo
            | ToolCall::AskUser { .. }
            | ToolCall::CannotDo { .. } => false,
        }
    }
}

/// The caption styles `setCaptions` offers, in the schema's order, each with how the model is told it looks. Whether a
/// word lights up as it is said is what tells them apart when a person asks for captions "that light up" or "word by
/// word": karaoke, highlight and word pop light up each word, bold only pops words in. Word pop and bold box are the
/// shorts' styles. The other built-ins (Loud, Arcade) stay out of chat.
pub const CHAT_CAPTION_STYLES: [(BuiltinCaptionStyle, &str); 7] = [
    (
        BuiltinCaptionStyle::Clean,
        "calm subtitles, a sentence at a time",
    ),
    (
        BuiltinCaptionStyle::Bold,
        "up to three big uppercase words that pop in one by one; no word lights up",
    ),
    (
        BuiltinCaptionStyle::Karaoke,
        "a line of words where each word lights up as it is said, word by word",
    ),
    (
        BuiltinCaptionStyle::Highlight,
        "up to four big uppercase words where a coloured box lights up each word as it is said, word by word",
    ),
    (BuiltinCaptionStyle::Boxed, "sentences on rounded boxes"),
    (
        BuiltinCaptionStyle::WordPop,
        "for shorts: words pop in as said, the said one coloured",
    ),
    (
        BuiltinCaptionStyle::BoldBox,
        "for shorts: heavy words on a rounded box",
    ),
];

/// The formats `setFormat` offers, `setClip`'s framing names, and the recording tools' `format`.
const FORMATS: [FormatKind; 3] = [
    FormatKind::Landscape,
    FormatKind::Portrait,
    FormatKind::Square,
];

/// The edges `trimClip` moves.
const EDGES: [Edge; 2] = [Edge::Start, Edge::End];

/// The pause lengths `removeSilences` takes, in seconds.
pub const PAUSE_SECS: RangeInclusive<f64> = 0.3..=10.0;

/// How long `freezeFrame` may hold a frame, in seconds, and how long it holds one when the person gave no length.
pub const FREEZE_SECS: RangeInclusive<f64> = 0.1..=60.0;
pub const FREEZE_DEFAULT_US: Us = 2_000_000;

/// The most words a noun phrase for `blurObjects` or `followSubject` may have.
pub const MAX_PHRASE_WORDS: usize = 8;

/// One tool: what the model reads about it and how its arguments are read.
struct Tool {
    name: &'static str,
    description: &'static str,
    /// Its parameters: `required` lists them in this order, `properties` by name (serde_json sorts object keys).
    parameters: fn() -> Vec<(&'static str, Value)>,
    read: fn(&mut Args) -> Result<ToolCall, ToolError>,
}

/// Every tool, in the order the model is shown them.
const TOOLS: &[Tool] = &[
    Tool {
        name: "cutRange",
        description: "Remove the footage from startSec to endSec. Removals are proposed: the person approves them first.",
        parameters: || {
            vec![
                (
                    "startSec",
                    seconds("Where the cut starts, timeline seconds from the context."),
                ),
                (
                    "endSec",
                    seconds("Where the cut ends, timeline seconds from the context."),
                ),
            ]
        },
        read: |a| {
            Ok(ToolCall::CutRange {
                range: a.range("startSec", "endSec")?,
            })
        },
    },
    Tool {
        name: "keepOnly",
        description: "Keep only these parts, for a highlight or a short version, and remove everything else. Removals \
                      are proposed: the person approves them first.",
        parameters: || {
            vec![(
                "ranges",
                list(
                    object(vec![
                        (
                            "startSec",
                            seconds("Start of a part to keep, timeline seconds."),
                        ),
                        (
                            "endSec",
                            seconds("End of a part to keep, timeline seconds."),
                        ),
                    ]),
                    "The parts to keep, in time order, with times from the context.",
                ),
            )]
        },
        read: |a| {
            let ranges = a.each("ranges", |item| item.range("startSec", "endSec"))?;
            if ranges.is_empty() {
                return Err(a.error("ranges", Problem::Empty));
            }
            Ok(ToolCall::KeepOnly { ranges })
        },
    },
    Tool {
        name: "removeSilences",
        description: "Remove the pauses in speech longer than minPauseSec. Removals are proposed: the person approves \
                      them first.",
        parameters: || {
            vec![(
                "minPauseSec",
                nullable_number(&format!(
                    "Pauses longer than this many seconds go, from {} to {}. Null when the person gave no length.",
                    PAUSE_SECS.start(),
                    PAUSE_SECS.end()
                )),
            )]
        },
        read: |a| {
            Ok(ToolCall::RemoveSilences {
                min_pause_us: a.nullable_pause("minPauseSec")?,
            })
        },
    },
    Tool {
        name: "removeFillers",
        description: "Remove filler sounds such as um and uh. Removals are proposed: the person approves them first.",
        parameters: Vec::new,
        read: |_| Ok(ToolCall::RemoveFillers),
    },
    Tool {
        name: "trimClip",
        description: "Move the start or the end of one clip to a new time. Shortening a clip is proposed like other \
                      removals; lengthening it shows more of its file, at most into the clip next to it. The first \
                      clip's start can't move earlier.",
        parameters: || {
            vec![
                (
                    "clip",
                    string("The clip's ref from the context, such as c2."),
                ),
                ("edge", words(&EDGES, "Which end of the clip moves.")),
                (
                    "toSec",
                    seconds("Where that end moves to, in timeline seconds as the video plays now."),
                ),
            ]
        },
        read: |a| {
            Ok(ToolCall::TrimClip {
                clip: a.text("clip")?,
                edge: a.word("edge", &EDGES)?,
                to_us: a.time("toSec")?,
            })
        },
    },
    Tool {
        name: "splitAt",
        description: "Split the clip playing at this time into two clips.",
        parameters: || vec![("atSec", seconds("Where to split, timeline seconds."))],
        read: |a| {
            Ok(ToolCall::SplitAt {
                at_us: a.time("atSec")?,
            })
        },
    },
    Tool {
        name: "freezeFrame",
        description: "Freeze the frame at this time: it holds still, silent, then the video plays on.",
        parameters: || {
            vec![
                (
                    "atSec",
                    nullable_number("Timeline seconds. Null for the playhead."),
                ),
                (
                    "durationSec",
                    nullable_number(&format!(
                        "Seconds it holds, {} to {}. Null for 2.",
                        FREEZE_SECS.start(),
                        FREEZE_SECS.end()
                    )),
                ),
            ]
        },
        read: |a| {
            Ok(ToolCall::FreezeFrame {
                at_us: a.nullable_time("atSec")?,
                duration_us: a.nullable_length("durationSec", &FREEZE_SECS)?,
            })
        },
    },
    clips::SET_CLIP,
    clips::SET_TRANSITION,
    background::SET_BACKGROUND,
    Tool {
        name: "setFormat",
        description: "Set the video's shape. It becomes the main format.",
        parameters: || {
            vec![
                (
                    "format",
                    words(
                        &FORMATS,
                        "landscape is 16:9, portrait is 9:16 (vertical: TikTok, Reels, Shorts), square is 1:1.",
                    ),
                ),
                (
                    "keepOthers",
                    boolean(
                        "True keeps the other formats as well. Use true unless the person asks for only this format \
                         (only vertical, just square, drop the landscape version); then false.",
                    ),
                ),
            ]
        },
        read: |a| {
            Ok(ToolCall::SetFormat {
                format: a.word("format", &FORMATS)?,
                keep_others: a.flag("keepOthers")?,
            })
        },
    },
    Tool {
        name: "setCaptions",
        description: "Turn captions on or off, or change their style.",
        parameters: || {
            vec![
                ("on", boolean("True shows captions, false hides them.")),
                (
                    "style",
                    nullable_words(&chat_styles(), &style_description()),
                ),
            ]
        },
        read: |a| {
            Ok(ToolCall::SetCaptions {
                on: a.flag("on")?,
                style: a.nullable_word("style", &chat_styles())?,
            })
        },
    },
    Tool {
        name: "fixCaptionWords",
        description: "Correct how a word or name reads in the captions and the transcript, everywhere it is said. The \
                      recording does not change.",
        parameters: || {
            vec![
                (
                    "find",
                    string("The word or words as the transcript has them."),
                ),
                ("replace", string("How they should read.")),
            ]
        },
        read: |a| {
            Ok(ToolCall::FixCaptionWords {
                find: a.text("find")?,
                replace: a.text("replace")?,
            })
        },
    },
    Tool {
        name: "findMoments",
        description: "Show the person the moments that match what they are looking for, from the transcript and the \
                      scenes. Nothing in the edit changes.",
        parameters: || {
            vec![
                (
                    "query",
                    string("What the person is looking for, in a few words."),
                ),
                (
                    "ranges",
                    list(
                        object(vec![
                            (
                                "startSec",
                                seconds("Where the moment starts, timeline seconds."),
                            ),
                            (
                                "endSec",
                                seconds("Where the moment ends, timeline seconds."),
                            ),
                            ("label", string("A few words on what happens.")),
                        ]),
                        "Each moment that matches, in time order. Empty when none does.",
                    ),
                ),
            ]
        },
        read: |a| {
            Ok(ToolCall::FindMoments {
                query: a.text("query")?,
                moments: a.each("ranges", |item| {
                    Ok(Moment {
                        range: item.range("startSec", "endSec")?,
                        label: item.label("label")?,
                    })
                })?,
            })
        },
    },
    Tool {
        name: "followSubject",
        description: "Keep the vertical and square crops on one person or thing. For a short or the speaker, use person.",
        parameters: || {
            vec![(
                "description",
                string(
                    "One short noun phrase naming who or what to follow, such as woman astronaut.",
                ),
            )]
        },
        read: |a| {
            Ok(ToolCall::FollowSubject {
                description: a.phrase("description")?,
            })
        },
    },
    Tool {
        name: "blurObjects",
        description: "Blur one kind of thing wherever it shows in the video. Call it once for each kind of thing.",
        parameters: || {
            vec![(
                "description",
                string("One short noun phrase naming what to blur, such as face or license plate."),
            )]
        },
        read: |a| {
            Ok(ToolCall::BlurObjects {
                description: a.phrase("description")?,
            })
        },
    },
    media::ADD_MUSIC,
    media::ADD_SOUND,
    media::ADD_TEXT,
    media::ADD_IMAGE,
    media::MAKE_STICKER,
    media::MAKE_TITLE_CARD,
    animate::ANIMATE,
    motion::SET_MOTION,
    text_motion::ANIMATE_TEXT,
    layer::SET_LAYER,
    style::STYLE_TEXT,
    effects::SET_EFFECTS,
    recorded::SET_CAMERA_MODE,
    recorded::ADJUST_CAMERA,
    recorded::DETACH_CAMERA,
    recorded::MUTE_SOURCE,
    recorded::SET_SOURCE_VOLUME,
    recorded::SEPARATE_SOUND,
    recorded::AUTO_ZOOM,
    Tool {
        name: "undo",
        description: "Undo the last change.",
        parameters: Vec::new,
        read: |_| Ok(ToolCall::Undo),
    },
    Tool {
        name: "askUser",
        description: "Ask the person one short question when you can't tell what they want, or what a removal should \
                      remove.",
        parameters: || vec![("question", string("One short plain question."))],
        read: |a| {
            Ok(ToolCall::AskUser {
                question: a.text("question")?,
            })
        },
    },
    Tool {
        name: "cannotDo",
        description: "Tell the person Klipolo can't do this, when no tool fits the request.",
        parameters: || vec![("reason", string("One short plain sentence saying why."))],
        read: |a| {
            Ok(ToolCall::CannotDo {
                reason: a.text("reason")?,
            })
        },
    },
];

/// Every tool the model may call, in a fixed order: `[{name, description, parameters}]`.
pub fn tools() -> Value {
    Value::Array(
        (TOOLS.iter())
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": object((t.parameters)()),
                })
            })
            .collect(),
    )
}

fn chat_styles() -> [BuiltinCaptionStyle; 7] {
    CHAT_CAPTION_STYLES.map(|(s, _)| s)
}

/// `clean: calm subtitles. bold: ... Null keeps the current style.`
fn style_description() -> String {
    let mut out: String = (CHAT_CAPTION_STYLES.iter())
        .map(|(s, looks)| format!("{}: {looks}. ", s.key()))
        .collect();
    out.push_str("Null keeps the current style.");
    out
}
