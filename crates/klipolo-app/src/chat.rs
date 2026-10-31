//! Chat in the app: the person's messages to the language model, and what comes of them in the open
//! project. The planner (`klipolo_chat`) reads the project and compiles the model's calls; this applies them.
//!
//! - `reply`: the shapes the chat panel reads, every model-written text cleaned of dashes as they are built.

mod reply;

pub use reply::{
    ChatApplied, ChatFailure, ChatFailureKind, ChatMoment, ChatOutcome, ChatProposal, ChatReply,
    ChatRequest, ProposalTag,
};
