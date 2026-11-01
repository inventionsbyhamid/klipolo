//! Another app's changes to the open project (MCP), through the path every edit takes (`with_open`: the engine's feed,
//! the save): an app's batch committed as one undo step the app made, kept within the apps' shares of the history so it
//! never pushes out a step of the person's (`Document::commit_tx_keeping`), and an app's undo of its own newest step.
//! Its folded steps are labelled `mcp.fromEarlier` with the app's name ("Earlier changes from Example Client").

use klipolo_app::chat::{At, Committed};
use klipolo_mcp::limits::undo_shares;
use klipolo_model::media::music::apply_with_music_refit;
use klipolo_model::coded::ErrorParam;
use klipolo_model::history::{ByApp, LabelParams, Maker, Refused};
use klipolo_model::risk::OpRisk;

use super::apply::history_limits;
use super::chat::at_of;
use super::*;

/// The label of a step folded from one app's own steps, with the app's name as `client`.
const FROM_EARLIER: &str = "mcp.fromEarlier";

/// Why an app's change wasn't made. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppRefused {
    NoProject,
    /// The open project isn't where the app read it.
    Moved,
    /// A project from a newer Klipolo.
    ReadOnly,
    /// Checked again on the project as it is, the batch takes something away: only the person applies that.
    Removes,
    /// The redo holds a step someone else made, which any commit would drop.
    RedoWaits,
    PushesOut,
    OverShare,
    /// The newest undo step isn't the app's.
    NotYours,
    NothingToUndo,
    /// The batch doesn't apply to the project as it is, in one plain sentence.
    Invalid(String),
}

/// How a step folded from `client`'s own steps is labelled.
fn from_earlier(client: &str) -> (String, Option<LabelParams>) {
    let params = LabelParams::from([("client".to_owned(), ErrorParam::from(client))]);
    (FROM_EARLIER.to_owned(), Some(params))
}

/// Commits `tx`, app `client`'s batch, as one undo step the app made, when the open project stands where `stands` says
/// the app read it; what changed and where the project stands now. Its music is fitted to the video's new end in the
/// same step, as for every edit of the person's. Checked here, under the project's lock, in one step with the commit:
/// the project's place, that it isn't read-only, that the batch takes nothing away (judged again from its own ops on
/// the project as it is, whatever origin it carries, the music's refit aside), and that no redo step of someone else's
/// would go. The
/// commit makes room only from the app's own steps, within its share of the history and all apps' (`undo_shares`).
pub(crate) fn commit_by_app(
    state: &ProjectSessionState,
    client: &str,
    tx: &Transaction,
    stands: impl FnOnce(&At) -> bool,
) -> Result<Committed, AppRefused> {
    let done = with_open(state, |open| {
        if !stands(&at_of(state, open)) {
            return Ok(Err(AppRefused::Moved));
        }
        if open.read_only {
            return Ok(Err(AppRefused::ReadOnly));
        }
        let document = &mut open.document;
        if tx.risk_against(document.project()) == OpRisk::Removal {
            return Ok(Err(AppRefused::Removes));
        }
        let mine = Maker::App(client.to_owned());
        if document.redo_steps_where(|maker| *maker != mine) > 0 {
            return Ok(Err(AppRefused::RedoWaits));
        }
        // The music fitted to the video's new end, as for every edit of the person's (`apply_and_commit`).
        let next = match apply_with_music_refit(document.project(), tx.ops.clone()) {
            Ok(done) => {
                if let Some(why) = &done.refused {
                    tracing::warn!(%why, "the music's refit was refused");
                }
                done.project
            }
            Err(e) => return Ok(Err(AppRefused::Invalid(e.to_string()))),
        };
        let limits = history_limits();
        document.set_limits(limits);
        document.set_folded_label(from_earlier);
        let shares = undo_shares(client, limits);
        let by = ByApp {
            app: client,
            shares: &shares,
            after_point: false,
        };
        Ok(match document.commit_tx_keeping(tx, next, &by) {
            Ok(change) => Ok(Committed {
                patch: document.snapshot_patch(change),
                at: at_of(state, open),
                project: open.document.shared(),
            }),
            Err(Refused::PushesOut(_)) => Err(AppRefused::PushesOut),
            Err(Refused::OverShare(..)) => Err(AppRefused::OverShare),
        })
    });
    done.unwrap_or(Err(AppRefused::NoProject))
}

/// Undoes the newest undo step, only when app `client` made it: a step the person made (a proposal of the app's they
/// applied included) is `NotYours`.
pub(crate) fn undo_by_app(
    state: &ProjectSessionState,
    client: &str,
) -> Result<Committed, AppRefused> {
    let done = with_open(state, |open| {
        if open.read_only {
            return Ok(Err(AppRefused::ReadOnly));
        }
        let mine = Maker::App(client.to_owned());
        match open
            .document
            .undo_view()
            .last()
            .map(|step| *step.maker == mine)
        {
            None => return Ok(Err(AppRefused::NothingToUndo)),
            Some(false) => return Ok(Err(AppRefused::NotYours)),
            Some(true) => {}
        }
        open.document.set_limits(history_limits());
        let Some(change) = open.document.undo_change() else {
            return Ok(Err(AppRefused::NothingToUndo));
        };
        Ok(Ok(Committed {
            patch: open.document.snapshot_patch(change),
            at: at_of(state, open),
            project: open.document.shared(),
        }))
    });
    done.unwrap_or(Err(AppRefused::NoProject))
}
