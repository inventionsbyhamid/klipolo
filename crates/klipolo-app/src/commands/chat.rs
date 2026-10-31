//! Chat: thin commands over `klipolo_app::chat::ChatSession`. A message comes back as a reply (what was applied, as a
//! patch the page merges; the proposal, with its id; the moments and the revision they are on; the pictures it asked
//! for) or as why nothing happened (`ChatOutcome::Failed`). It is rejected only with `project.moved`, when its
//! selection or "Keep only these" ranges are on a revision the project left. Apply, Discard and Undo name a proposal or
//! the change a chip is under; a project that moved on is `project.moved`. A picture is drawn when the page asks (and
//! again for Try again) and placed on Use. Everything comes back from the commands: no events. None carries a key or a
//! token.

use std::sync::Arc;

use klipolo_app::analysis::AnalysisState;
use klipolo_app::chat::{
    At, ChatApplied, ChatOutcome, ChatPictureDrawn, ChatRequest, ChatSession, CommitError,
    Committed, MetaModels, ProjectNow, Projects,
};
use klipolo_model::coded::Said;
use klipolo_model::history::SnapshotPatch;
use klipolo_model::{Project, Transaction};
use tauri::{AppHandle, Manager, State};

use super::settings::CredentialsState;
use crate::project::ProjectSessionState;

/// The app's chat.
pub(crate) type ChatHandle = Arc<ChatSession>;

/// The open project, as the app's project session has it.
struct OpenProjectOf(AppHandle);

impl Projects for OpenProjectOf {
    fn now(&self) -> Option<ProjectNow> {
        self.0.state::<ProjectSessionState>().now()
    }

    fn commit(
        &self,
        at: &At,
        tx: &Transaction,
        next: Option<Project>,
    ) -> Result<Committed, CommitError> {
        self.0.state::<ProjectSessionState>().commit(at, tx, next)
    }

    fn undo(&self, at: &At) -> Result<Committed, CommitError> {
        self.0.state::<ProjectSessionState>().undo(at)
    }
}

/// The app's chat over the open project, the analysis's findings and the saved credentials.
pub(crate) fn load(
    app: &AppHandle,
    credentials: CredentialsState,
    analysis: Arc<AnalysisState>,
) -> ChatHandle {
    Arc::new(ChatSession::new(
        Arc::new(OpenProjectOf(app.clone())),
        analysis,
        Arc::new(MetaModels::new(credentials)),
    ))
}

/// One message (or "Keep only these"): the reply, or why nothing happened, the project untouched. `project.moved` when
/// its selection or moments were made on a revision the project left.
#[tauri::command]
pub(crate) async fn chat_send(
    chat: State<'_, ChatHandle>,
    request: ChatRequest,
) -> Result<ChatOutcome, Said> {
    chat.send(request).await.map_err(|e| e.said())
}

/// Applies proposal `id` as one undo step; `project.moved` when the project changed since it was made (it is dropped).
#[tauri::command(async)]
pub(crate) fn chat_apply_proposal(
    chat: State<'_, ChatHandle>,
    id: String,
) -> Result<ChatApplied, Said> {
    chat.apply(&id).map_err(|e| e.said())
}

/// Drops proposal `id`: nothing changes.
#[tauri::command(async)]
pub(crate) fn chat_discard_proposal(chat: State<'_, ChatHandle>, id: String) {
    chat.discard(&id);
}

/// The Undo chip under chat's change that left the project at `revision` (`ChatApplied.change.history.revision`): takes
/// it back while it is still the last change; else `project.moved`.
#[tauri::command(async)]
pub(crate) fn chat_undo_last(
    chat: State<'_, ChatHandle>,
    revision: u64,
) -> Result<SnapshotPatch, Said> {
    chat.undo_last(revision).map_err(|e| e.said())
}

/// New chat: the conversation kept for the model, and its proposals, are forgotten.
#[tauri::command(async)]
pub(crate) fn chat_reset(chat: State<'_, ChatHandle>) {
    chat.reset();
}

/// Draws picture `id` (again, for Try again) into the project's media folder, for its card, and lets the page show the
/// file. About 35 s; a plain sentence when it can't be drawn.
#[tauri::command]
pub(crate) async fn chat_draw_picture(
    app: AppHandle,
    chat: State<'_, ChatHandle>,
    id: String,
) -> Result<ChatPictureDrawn, Said> {
    let drawn = chat.draw_picture(&id).await.map_err(|e| e.said())?;
    if let Err(error) = app.asset_protocol_scope().allow_file(&drawn.path) {
        tracing::warn!(%error, "a picture drawn for chat can't be shown");
    }
    Ok(drawn)
}

/// Use: places picture `id` in the open project as one undo step; a plain sentence when its moment or its title is
/// gone.
#[tauri::command(async)]
pub(crate) fn chat_use_picture(
    chat: State<'_, ChatHandle>,
    id: String,
) -> Result<ChatApplied, Said> {
    chat.place_picture(&id).map_err(|e| e.said())
}
