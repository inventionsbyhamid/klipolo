//! Chat in the app: the person's messages to the language model, and what comes of them in the open
//! project. The planner (`klipolo_chat`) reads the project and compiles the model's calls; this applies them.
//!
//! - `session`: one message at a time, its additions applied at once, its removals kept as a proposal for Apply or
//!   Discard, the conversation remembered, the spend counted. The proposal slot is shared with other apps (MCP): an
//!   app's proposal waits in it as chat's do (`SlotGuard`), and what the person did with it is kept for the app.
//! - `project`: the open project as chat reads and edits it (`Projects`), which the app's project session implements;
//!   `commit_reviewed`, the one way chat's edits enter the history; `MemoryProjects` for tools and tests.
//! - `sources`: the language model as the saved credentials allow it (`Models`) and what the analysis found
//!   (`Findings`).
//! - `inputs`: a message's context and compiler inputs from one snapshot. `describe`: a proposal in Klipolo's words.
//!   `jobs`: work for runners. `pictures`: pictures a message asked for, drawn on request and placed on Use. `spend`:
//!   what chat cost this session.
//! - `reply`: the shapes the chat panel reads, every model-written text cleaned of dashes as they are built; another
//!   app's change comes to the panel as an `ExternalReply`.

mod describe;
mod inputs;
mod jobs;
mod media_jobs;
pub mod pictures;
pub mod project;
mod reply;
pub mod session;
pub mod sources;
pub mod spend;

pub use inputs::inputs_for;
pub use jobs::{BLUR_COMING, FOLLOW_COMING, picture_for};
pub use pictures::{
    ChatPicture, ChatPictureDrawn, ChatPictureKind, PictureAsk, StickerPlacement, background_ops,
};
pub use project::{
    At, CommitError, Committed, MemoryProjects, ProjectNow, Projects, commit_reviewed,
};
pub use reply::{
    ChatApplied, ChatFailure, ChatFailureKind, ChatMoment, ChatOutcome, ChatProposal, ChatReply,
    ChatRequest, ExternalReply, ProposalTag,
};
pub use session::{ChatSession, Decided, Gone, SlotGuard, Waiting};
pub use sources::{Findings, MetaModels, Models, Readiness};
pub use spend::{ChatSpend, SpendCounter};
