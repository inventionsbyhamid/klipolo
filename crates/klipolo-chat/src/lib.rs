//! Chat with a project. `context`: what the language model reads about the project, its speech and scenes,
//! the playhead and the selection. `prompt`: what it is told first. `tools`: the tools it may call, and its calls read
//! into typed edits.

pub mod context;
pub mod prompt;
pub mod tools;

pub use context::{
    ChatContext, MAX_CONTEXT_TOKENS, Refs, SceneEntry, SceneLog, SpeakerTurn, View, build_context,
    estimate_tokens,
};
pub use prompt::system_prompt;
pub use tools::{FunctionCall, Moment, Problem, ToolCall, ToolError, tools};
