//! Chat with a project. `context`: what the language model reads about the project, its speech and scenes,
//! the playhead and the selection.

pub mod context;

pub use context::{
    ChatContext, MAX_CONTEXT_TOKENS, Refs, SceneEntry, SceneLog, SpeakerTurn, View, build_context,
    estimate_tokens,
};
