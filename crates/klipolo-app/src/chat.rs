//! Chat in the app: the person's messages to the language model, and what comes of them in the open
//! project. The planner (`klipolo_chat`) reads the project and compiles the model's calls; this applies them.
//!
//! - `project`: the open project as chat reads and edits it (`Projects`), which the app's project session implements;
//!   `commit_reviewed`, the one way chat's edits enter the history; `MemoryProjects` for tools and tests.
//! - `reply`: the shapes the chat panel reads, every model-written text cleaned of dashes as they are built.

pub mod project;
mod reply;

pub use project::{
    At, CommitError, Committed, MemoryProjects, ProjectNow, Projects, commit_reviewed,
};
pub use reply::{
    ChatApplied, ChatFailure, ChatFailureKind, ChatMoment, ChatOutcome, ChatProposal, ChatReply,
    ChatRequest, ProposalTag,
};
