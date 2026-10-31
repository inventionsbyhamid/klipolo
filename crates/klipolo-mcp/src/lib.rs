//! Other apps edit the open project over MCP: any MCP client calls Klipolo's own chat tools, which go
//! through chat's compiler as chat's calls do. What adds or restyles applies at once, as one undo step the app made;
//! what removes footage waits as a proposal in the chat panel until the person clicks Apply. The crate holds the
//! protocol and reaches the app only through `host::Host`: no Tauri type, no model vendor, and it never calls a model
//! (an MCP client is its own planner).
//!
//! - `rpc`: JSON-RPC messages in, replies out (`Mcp::handle`), and what the stdio bridge answers alone (`listed`).
//! - `session`: one per `initialize`, with the app's name and what of the project it last read.
//! - `tools`: chat's edit tools minus its conversation tools, plus four reads; `resources`: the reads; `pages`: the
//!   transcript in pages.
//! - `call`: an edit tool's call; `limits`: the apps' paces and their shares of the undo history.
//! - `words`: what agents read.

pub mod call;
pub mod host;
pub mod limits;
pub mod pages;
pub mod resources;
pub mod rpc;
pub mod session;
pub mod tools;
pub mod words;

pub use host::{Applied, Host, HostError, OpenProposal, ProposalOutcome, Snapshot};
pub use rpc::{Handled, Mcp, initialize_result, listed};
pub use session::{Seen, Session, UNNAMED, client_name};
