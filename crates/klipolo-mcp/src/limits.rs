//! What apps may hold of the person's undo history.
//!
//! An app's batches commit with `Document::commit_tx_keeping` within the apps' shares of the history: an app's steps
//! hold at most a quarter of its steps and bytes, all apps' together half. Room is made only from the app's own steps,
//! so an app's step never pushes out one the person made, or their redo.

use klipolo_model::history::{HistoryLimits, Share, Whose};

/// The shares of `l` that `client`'s steps (a quarter) and all apps' steps together (half) may hold, by steps and
/// bytes.
pub fn undo_shares(client: &str, l: HistoryLimits) -> [Share<'_>; 2] {
    [
        Share {
            of: Whose::App(client),
            steps: l.steps / 4,
            bytes: l.bytes / 4,
        },
        Share {
            of: Whose::Apps,
            steps: l.steps / 2,
            bytes: l.bytes / 2,
        },
    ]
}
