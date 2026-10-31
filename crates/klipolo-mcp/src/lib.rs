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
//! - `server`: Streamable HTTP on 127.0.0.1, with a token per run; `connection`: the file that says where it listens
//!   and with which token, and the server with its file (`McpServer`).

pub mod call;
pub mod connection;
pub mod host;
pub mod limits;
pub mod pages;
pub mod resources;
pub mod rpc;
pub mod server;
pub mod session;
pub mod tools;
pub mod words;

pub use connection::{ConnectionFile, McpServer};
pub use host::{Applied, Host, HostError, OpenProposal, ProposalOutcome, Snapshot};
pub use rpc::{Handled, Mcp, initialize_result, listed};
pub use session::{Seen, Session, UNNAMED, client_name};
