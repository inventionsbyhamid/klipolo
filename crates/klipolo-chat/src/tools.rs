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
//! Adding a tool (the media tools later) is one `ToolCall` variant and one entry in `TOOLS`, where its name,
//! description, parameters and reader sit together; the tests check every schema is strict and every reader reads
//! the fields its schema lists.

use std::fmt;
use std::ops::RangeInclusive;

use klipolo_model::captions::styles::BuiltinCaptionStyle;
use klipolo_model::ops::Edge;
use klipolo_model::project::FormatKind;
use klipolo_model::time::{MAX_TIME_US, RangeUs, US_PER_SEC, Us, secs_to_us};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

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
            ToolCall::SetFormat { .. } => "setFormat",
            ToolCall::SetCaptions { .. } => "setCaptions",
            ToolCall::FixCaptionWords { .. } => "fixCaptionWords",
            ToolCall::FindMoments { .. } => "findMoments",
            ToolCall::FollowSubject { .. } => "followSubject",
            ToolCall::BlurObjects { .. } => "blurObjects",
            ToolCall::Undo => "undo",
            ToolCall::AskUser { .. } => "askUser",
            ToolCall::CannotDo { .. } => "cannotDo",
        }
    }
}

/// Why a call couldn't be read: the tool it named, the argument at fault (`ranges[1].endSec`; None for the call as a
/// whole) and the problem. Its text is one plain sentence for the model, on one line.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolError {
    pub tool: String,
    pub field: Option<String>,
    pub problem: Problem,
}

/// What was wrong with a call.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// No tool has the name called.
    UnknownTool,
    /// The arguments aren't JSON.
    NotJson,
    /// A field the tool needs isn't there.
    Missing,
    /// A field the tool doesn't take.
    Unexpected,
    /// A value of the wrong JSON type: what it must be, and what it was (`a number of seconds`, `text`).
    WrongType {
        expected: &'static str,
        got: &'static str,
    },
    /// A word that isn't one of `allowed` (or null, when `nullable`); `got` is the value given, as JSON.
    NotOneOf {
        allowed: Vec<&'static str>,
        nullable: bool,
        got: String,
    },
    /// Empty text, or a list that needs an item.
    Empty,
    /// A number of seconds outside `min..=max`.
    OutOfRange { min: f64, max: f64, got: f64 },
    /// An end at or before its start: the start's field and both values as given.
    NotAfter {
        start_field: String,
        start: f64,
        end: f64,
    },
    /// More than one short noun phrase: a list, or over `MAX_PHRASE_WORDS` words.
    NotOnePhrase,
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tool = &self.tool;
        let field = self.field.as_deref().unwrap_or("the arguments");
        match &self.problem {
            Problem::UnknownTool => write!(f, "There is no tool called {tool:?}."),
            Problem::NotJson => write!(f, "{tool}: the arguments are not valid JSON."),
            Problem::Missing => write!(f, "{tool}: {field} is missing."),
            // The model wrote this field's name: quoted, so a line break in it stays on the line.
            Problem::Unexpected => write!(f, "{tool}: {field:?} is not an argument of this tool."),
            Problem::WrongType { expected, got } => {
                write!(f, "{tool}: {field} must be {expected}, not {got}.")
            }
            Problem::NotOneOf {
                allowed,
                nullable,
                got,
            } => {
                let or_null = if *nullable { " or null" } else { "" };
                write!(
                    f,
                    "{tool}: {field} must be one of {}{or_null}, not {got}.",
                    allowed.join(", ")
                )
            }
            Problem::Empty => write!(f, "{tool}: {field} must not be empty."),
            Problem::OutOfRange { min, max, got } => write!(
                f,
                "{tool}: {field} must be between {} and {} seconds{}.",
                said_secs(*min),
                said_secs(*max),
                given(*got)
            ),
            Problem::NotAfter {
                start_field,
                start,
                end,
            } => write!(
                f,
                "{tool}: {field} ({}) must be after {start_field} ({}).",
                said_secs(*end),
                said_secs(*start)
            ),
            Problem::NotOnePhrase => write!(
                f,
                "{tool}: {field} must be one short noun phrase naming one thing, at most {MAX_PHRASE_WORDS} words; \
                 call {tool} once for each thing."
            ),
        }
    }
}

impl std::error::Error for ToolError {}

/// The longest time the model stores, in seconds: a number of seconds past it reads as how it misses (`missed_by`).
const LONGEST_SECS: f64 = (MAX_TIME_US / US_PER_SEC) as f64;

/// Seconds as an error says them: at most three decimals ("12.123", "15", "-1").
fn said_secs(secs: f64) -> String {
    // `+ 0.0` turns a rounded -0 into 0.
    format!("{}", (secs * 1000.0).round() / 1000.0 + 0.0)
}

/// What was given, after the rule it breaks: ", not 15", or past the longest time either way "; got a number past
/// 86400", so a model's `1e300` never comes back as hundreds of digits.
fn given(got: f64) -> String {
    let longest = said_secs(LONGEST_SECS);
    if got > LONGEST_SECS {
        format!("; got a number past {longest}")
    } else if got < -LONGEST_SECS {
        format!("; got a number below -{longest}")
    } else {
        format!(", not {}", said_secs(got))
    }
}

/// The caption styles `setCaptions` offers, in the schema's order, each with how the model is told it looks. The
/// other built-ins (Loud, Arcade) stay out of chat.
pub const CHAT_CAPTION_STYLES: [(BuiltinCaptionStyle, &str); 5] = [
    (BuiltinCaptionStyle::Clean, "calm subtitles"),
    (
        BuiltinCaptionStyle::Bold,
        "big words that pop in as they are said",
    ),
    (
        BuiltinCaptionStyle::Karaoke,
        "a line where the spoken word lights up",
    ),
    (
        BuiltinCaptionStyle::Highlight,
        "a coloured box follows each word",
    ),
    (BuiltinCaptionStyle::Boxed, "sentences on rounded boxes"),
];

/// The formats `setFormat` offers.
const FORMATS: [FormatKind; 3] = [
    FormatKind::Landscape,
    FormatKind::Portrait,
    FormatKind::Square,
];

/// The edges `trimClip` moves.
const EDGES: [Edge; 2] = [Edge::Start, Edge::End];

/// The pause lengths `removeSilences` takes, in seconds.
pub const PAUSE_SECS: RangeInclusive<f64> = 0.3..=10.0;

/// The most words a noun phrase for `blurObjects` or `followSubject` may have.
pub const MAX_PHRASE_WORDS: usize = 8;

/// A word a tool takes for one of the model's own values: the model's name for it.
trait Word: Copy {
    fn word(self) -> &'static str;
}

impl Word for FormatKind {
    fn word(self) -> &'static str {
        match self {
            FormatKind::Landscape => "landscape",
            FormatKind::Portrait => "portrait",
            FormatKind::Square => "square",
        }
    }
}

impl Word for Edge {
    fn word(self) -> &'static str {
        match self {
            Edge::Start => "start",
            Edge::End => "end",
        }
    }
}

impl Word for BuiltinCaptionStyle {
    fn word(self) -> &'static str {
        self.key()
    }
}

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
                nullable_seconds(&format!(
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
                      removals; lengthening it shows more of its file.",
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
        description: "Keep the crop on one person or thing as it moves, for portrait or square.",
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

fn chat_styles() -> [BuiltinCaptionStyle; 5] {
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

fn seconds(description: &str) -> Value {
    json!({"type": "number", "description": description})
}

fn nullable_seconds(description: &str) -> Value {
    json!({"type": ["number", "null"], "description": description})
}

fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn boolean(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

fn words<T: Word>(all: &[T], description: &str) -> Value {
    let all: Vec<&str> = all.iter().map(|w| w.word()).collect();
    json!({"type": "string", "enum": all, "description": description})
}

/// One of `all`, or null: null is listed in the enum too, or a strict schema would refuse it.
fn nullable_words<T: Word>(all: &[T], description: &str) -> Value {
    let mut listed: Vec<Value> = all.iter().map(|w| json!(w.word())).collect();
    listed.push(Value::Null);
    json!({"type": ["string", "null"], "enum": listed, "description": description})
}

fn list(items: Value, description: &str) -> Value {
    json!({"type": "array", "items": items, "description": description})
}

/// A closed object: every property required, no others.
fn object(properties: Vec<(&'static str, Value)>) -> Value {
    let required: Vec<&str> = properties.iter().map(|(name, _)| *name).collect();
    let properties: Map<String, Value> = (properties.into_iter())
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// What a value is, as an error says it was.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(true) => "true",
        Value::Bool(false) => "false",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

const SECONDS: &str = "a number of seconds";

/// One object of arguments being read: each field is taken out as it is read, so what is left is what the tool
/// doesn't take.
struct Args {
    tool: &'static str,
    /// Where these arguments sit: empty for the call's own, `ranges[1].` for an item of a list.
    path: String,
    map: Map<String, Value>,
}

impl Args {
    fn error(&self, key: &str, problem: Problem) -> ToolError {
        ToolError {
            tool: self.tool.to_owned(),
            field: Some(format!("{}{key}", self.path)),
            problem,
        }
    }

    fn take(&mut self, key: &str) -> Result<Value, ToolError> {
        self.map
            .remove(key)
            .ok_or_else(|| self.error(key, Problem::Missing))
    }

    /// A nullable field: None when it is null or left out.
    fn take_nullable(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key).filter(|v| !v.is_null())
    }

    fn number(&self, key: &str, value: &Value, expected: &'static str) -> Result<f64, ToolError> {
        value.as_f64().ok_or_else(|| {
            self.error(
                key,
                Problem::WrongType {
                    expected,
                    got: kind(value),
                },
            )
        })
    }

    /// Seconds within `bounds`, as given and in microseconds.
    fn within(&self, key: &str, secs: f64, bounds: &RangeInclusive<f64>) -> Result<Us, ToolError> {
        if bounds.contains(&secs) {
            Ok(secs_to_us(secs))
        } else {
            Err(self.error(
                key,
                Problem::OutOfRange {
                    min: *bounds.start(),
                    max: *bounds.end(),
                    got: secs,
                },
            ))
        }
    }

    /// A timeline time: seconds, from 0 to the longest time the model stores.
    fn time_secs(&mut self, key: &str) -> Result<(f64, Us), ToolError> {
        let value = self.take(key)?;
        let secs = self.number(key, &value, SECONDS)?;
        let bounds = 0.0..=LONGEST_SECS;
        Ok((secs, self.within(key, secs, &bounds)?))
    }

    fn time(&mut self, key: &str) -> Result<Us, ToolError> {
        Ok(self.time_secs(key)?.1)
    }

    /// A range from `start` to `end`, the end after the start.
    fn range(&mut self, start: &str, end: &str) -> Result<RangeUs, ToolError> {
        let (start_secs, start_us) = self.time_secs(start)?;
        let (end_secs, end_us) = self.time_secs(end)?;
        if end_us <= start_us {
            return Err(self.error(
                end,
                Problem::NotAfter {
                    start_field: format!("{}{start}", self.path),
                    start: start_secs,
                    end: end_secs,
                },
            ));
        }
        Ok(RangeUs {
            start: start_us,
            end: end_us,
        })
    }

    /// `removeSilences`' pause length: null, or seconds within `PAUSE_SECS`.
    fn nullable_pause(&mut self, key: &str) -> Result<Option<Us>, ToolError> {
        let Some(value) = self.take_nullable(key) else {
            return Ok(None);
        };
        let secs = self.number(key, &value, "a number of seconds or null")?;
        self.within(key, secs, &PAUSE_SECS).map(Some)
    }

    /// Text with its runs of white space read as one space, trimmed; possibly empty.
    fn label(&mut self, key: &str) -> Result<String, ToolError> {
        let value = self.take(key)?;
        let Value::String(text) = &value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "text",
                    got: kind(&value),
                },
            ));
        };
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    /// Text as `label` reads it, not empty.
    fn text(&mut self, key: &str) -> Result<String, ToolError> {
        let text = self.label(key)?;
        if text.is_empty() {
            return Err(self.error(key, Problem::Empty));
        }
        Ok(text)
    }

    /// One short noun phrase: no list, at most `MAX_PHRASE_WORDS` words.
    fn phrase(&mut self, key: &str) -> Result<String, ToolError> {
        let text = self.text(key)?;
        let words: Vec<&str> = text.split(' ').collect();
        let joined = |w: &&str| w.eq_ignore_ascii_case("and") || w.eq_ignore_ascii_case("or");
        if words.len() > MAX_PHRASE_WORDS
            || text.contains([',', ';', '&', '+', '/'])
            || words.iter().any(joined)
        {
            return Err(self.error(key, Problem::NotOnePhrase));
        }
        Ok(text)
    }

    fn flag(&mut self, key: &str) -> Result<bool, ToolError> {
        match self.take(key)? {
            Value::Bool(b) => Ok(b),
            other => Err(self.error(
                key,
                Problem::WrongType {
                    expected: "true or false",
                    got: kind(&other),
                },
            )),
        }
    }

    fn word_of<T: Word>(
        &self,
        key: &str,
        value: &Value,
        all: &[T],
        nullable: bool,
    ) -> Result<T, ToolError> {
        (value.as_str())
            .and_then(|s| all.iter().copied().find(|w| w.word() == s))
            .ok_or_else(|| {
                self.error(
                    key,
                    Problem::NotOneOf {
                        allowed: all.iter().map(|w| w.word()).collect(),
                        nullable,
                        got: value.to_string(),
                    },
                )
            })
    }

    fn word<T: Word>(&mut self, key: &str, all: &[T]) -> Result<T, ToolError> {
        let value = self.take(key)?;
        self.word_of(key, &value, all, false)
    }

    fn nullable_word<T: Word>(&mut self, key: &str, all: &[T]) -> Result<Option<T>, ToolError> {
        match self.take_nullable(key) {
            Some(value) => self.word_of(key, &value, all, true).map(Some),
            None => Ok(None),
        }
    }

    /// A list of objects, each read by `read` (and refused if it holds fields `read` doesn't take).
    fn each<T>(
        &mut self,
        key: &str,
        read: fn(&mut Args) -> Result<T, ToolError>,
    ) -> Result<Vec<T>, ToolError> {
        let value = self.take(key)?;
        let Value::Array(items) = value else {
            return Err(self.error(
                key,
                Problem::WrongType {
                    expected: "a list",
                    got: kind(&value),
                },
            ));
        };
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let at = format!("{key}[{i}]");
            let Value::Object(map) = item else {
                return Err(self.error(
                    &at,
                    Problem::WrongType {
                        expected: "an object",
                        got: kind(&item),
                    },
                ));
            };
            let mut args = Args {
                tool: self.tool,
                path: format!("{}{at}.", self.path),
                map,
            };
            out.push(read(&mut args)?);
            args.finish()?;
        }
        Ok(out)
    }

    /// Refuses the first field left unread.
    fn finish(self) -> Result<(), ToolError> {
        match self.map.keys().next() {
            Some(key) => Err(self.error(key, Problem::Unexpected)),
            None => Ok(()),
        }
    }
}
