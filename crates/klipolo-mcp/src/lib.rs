//! Other apps edit the open project over MCP: any MCP client calls Klipolo's own chat tools, which go
//! through chat's compiler as chat's calls do. What adds or restyles applies at once, as one undo step the app made;
//! what removes footage waits as a proposal in the chat panel until the person clicks Apply. The crate holds the
//! protocol and reaches the app only through `host::Host`: no Tauri type, no model vendor, and it never calls a model
//! (an MCP client is its own planner).
//!
//! - `pages`: the transcript in pages.
//! - `words`: what agents read.

pub mod host;
pub mod pages;
pub mod words;

pub use host::{Applied, Host, HostError, OpenProposal, ProposalOutcome, Snapshot};
