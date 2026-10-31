//! A session (`Mcp-Session-Id`): the name its app gave, and what of the project it last read.

use std::time::Instant;

/// What of the project a session last read: which project, and where its main track's clips were
/// (`klipolo_chat::timeline_key`). A call naming times or clip refs needs it to match the project as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub project_id: String,
    pub key: u64,
}

/// One app's session.
#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    /// The name its `initialize` gave (`client_name`).
    pub client: String,
    /// Set by reading the summary, the context or the transcript; None until then.
    pub seen: Option<Seen>,
    /// When it was last used.
    pub used: Instant,
}
