//! The person's view in Klipolo as the page last told it (`editor_view`, sent once the range selection settles, after a
//! seek, or on pause): the playhead, the time range selected, the overlay selected and the format the preview shows.
//! Another app's calls read it as the person's live selection and playhead (`klipolo_chat::View`), the evidence a time
//! may stand on; an app's own words never are. Forgotten when another project opens.

use std::sync::{Arc, Mutex, PoisonError};

use klipolo_chat::View;
use klipolo_model::project::FormatKind;
use klipolo_model::time::{RangeUs, Us};
use serde::Deserialize;
use tauri::State;
use ts_rs::TS;

/// What the page sends.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditorView {
    /// The time range selected on the timeline, if any.
    pub selection: Option<RangeUs>,
    pub playhead_us: Us,
    /// The overlay selected (a title, an image), if any.
    pub item: Option<String>,
    /// The format the preview shows.
    pub format: Option<FormatKind>,
}

/// The view as last told, for the app's run.
#[derive(Default)]
pub(crate) struct ViewState(Mutex<EditorView>);

/// The view, shared with the MCP host.
pub(crate) type ViewHandle = Arc<ViewState>;

impl ViewState {
    pub(crate) fn set(&self, view: EditorView) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = view;
    }

    /// The view as last told.
    pub(crate) fn now(&self) -> EditorView {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The playhead and the selection, as chat's compiler reads them.
    pub(crate) fn chat_view(&self) -> View {
        let view = self.now();
        View {
            playhead_us: view.playhead_us,
            selection: view.selection,
        }
    }

    /// Another project opened, or it closed: nothing of the old view is the person's view of the new project.
    pub(crate) fn forget(&self) {
        self.set(EditorView::default());
    }
}

/// The person's view in Klipolo, as the page has it now.
#[tauri::command(async)]
pub(crate) fn editor_view(state: State<'_, ViewHandle>, view: EditorView) {
    state.set(view);
}
