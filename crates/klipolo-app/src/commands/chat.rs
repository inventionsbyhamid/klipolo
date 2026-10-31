//! Chat: thin commands over `klipolo_app::chat::ChatSession`. A message comes back as a reply (what was applied, as a
//! patch the page merges; the proposal, with its id; the moments) or as why nothing happened (`ChatOutcome::Failed`),
//! never as a rejection. Apply, Discard and Undo name a proposal or chat's last change; a project that moved on is
//! `project.moved`. No command or event carries a key or a token.

use std::sync::Arc;

use klipolo_app::analysis::AnalysisState;
use klipolo_app::chat::{
    At, ChatApplied, ChatOutcome, ChatRequest, ChatSession, CommitError, Committed, MetaModels,
    ProjectNow, Projects,
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

/// Takes back chat's last change while nothing has changed since; else `project.moved`.
#[tauri::command(async)]
pub(crate) fn chat_undo_last(chat: State<'_, ChatHandle>) -> Result<SnapshotPatch, Said> {
    chat.undo_last().map_err(|e| e.said())
}

/// New chat: the conversation kept for the model, and its proposals, are forgotten.
#[tauri::command(async)]
pub(crate) fn chat_reset(chat: State<'_, ChatHandle>) {
    chat.reset();
}
