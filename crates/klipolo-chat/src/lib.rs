//! Chat with a project. `context`: what the language model reads about the project, its speech and scenes,
//! the playhead and the selection. `prompt`: what it is told first. `tools`: the tools it may call, and its calls read
//! into typed edits. `compile`: those edits checked against the project and turned into ops, additions to apply at once
//! and removals to propose, with `evidence` holding the model's times to what the context showed. `sanitize`: the
//! model's text for the person without em or en dashes.

pub mod compile;
pub mod context;
pub mod evidence;
pub mod prompt;
pub mod sanitize;
pub mod tools;

pub use compile::{
    ANALYZE_FIRST, AnalysisState, Audience, Compiled, Inputs, Job, Proposal, Refusal,
    STILL_LISTENING, compile,
};
pub use context::{
    ChatContext, MAX_CONTEXT_TOKENS, Refs, SceneEntry, SceneLog, SpeakerTurn, View, build_context,
    estimate_tokens,
};
pub use prompt::system_prompt;
pub use tools::{FunctionCall, Moment, Problem, ToolCall, ToolError, tools};
