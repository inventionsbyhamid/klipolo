//! Chat with a project. `context`: what the language model reads about the project, its speech and scenes,
//! the media it can add (`library`), the playhead and the selection. `prompt`: what it is told first. `tools`: the
//! tools it may call, and its calls read into typed edits. `compile`: those edits checked against the project and
//! turned into ops, additions to apply at once and removals to propose, and jobs for pictures to draw, with `evidence`
//! holding the model's times to what the context showed. `sanitize`: the model's text for the person without em or en
//! dashes. `model`: the language model as an interface this crate owns. `planner`: one message sent to it, with at most
//! one refusal round, out as a reply and compiled edits.

pub mod compile;
pub mod context;
pub mod evidence;
pub mod library;
pub mod model;
pub mod planner;
pub mod prompt;
pub mod sanitize;
pub mod tools;

pub use compile::{
    ANALYZE_FIRST, AnalysisState, Audience, Compiled, Inputs, Job, MakeSticker, NO_FILLERS,
    Proposal, ROUGH_FILLERS, ROUGH_FILLERS_STAY, Refusal, STILL_LISTENING, STILL_WATCHING, compile,
    compile_for_app, sticker_ops, sticker_placement,
};
pub use context::{
    ChatContext, FullTranscript, MAX_CONTEXT_TOKENS, Refs, SceneEntry, SceneLog, SpeakerTurn,
    TranscriptLine, View, build_context, estimate_tokens, full_transcript, timeline_key,
};
pub use library::{Library, LibraryItem, MediaKind};
pub use model::{BoxFuture, Item, LanguageModel, ModelError, ModelRequest, ModelResponse};
pub use planner::{
    ChatError, ChatErrorKind, DONE_OUTPUT, KEPT_TURNS, PROPOSED_OUTPUT, PlanOut, TOLD_PERSON,
    TOO_LONG, Turn, plan, remember,
};
pub use prompt::system_prompt;
pub use tools::{FunctionCall, Moment, Problem, ToolCall, ToolError, tools};
