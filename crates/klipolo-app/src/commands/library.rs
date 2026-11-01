//! The library's favorites and recently used items, for every project: thin commands over `klipolo_app::library_marks`,
//! kept in the app's config folder. One change at a time, so two quick stars both stay. Errors are one plain sentence.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_app::library_marks::{self, Marks};
use tauri::State;

/// Where the marks are kept (none when the system has no config folder for the app), changed one at a time.
pub(crate) struct LibraryMarksState {
    dir: Option<PathBuf>,
    changing: Mutex<()>,
}

impl LibraryMarksState {
    pub(crate) fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            changing: Mutex::new(()),
        }
    }

    /// `change` on the marks' folder, after any change ahead of it; its error said plainly.
    fn change(
        &self,
        change: impl FnOnce(&std::path::Path) -> std::io::Result<Marks>,
    ) -> Result<Marks, String> {
        const NOT_SAVED: &str = "Klipolo couldn't save that. Check the disk, then try again.";
        let dir = self.dir.as_deref().ok_or(NOT_SAVED)?;
        let _one = self.changing.lock().unwrap_or_else(PoisonError::into_inner);
        change(dir).map_err(|error| {
            tracing::warn!(%error, "the library's marks couldn't be saved");
            NOT_SAVED.to_owned()
        })
    }
}

/// The favorites and recently used items, as saved.
#[tauri::command(async)]
pub(crate) fn library_marks(state: State<'_, LibraryMarksState>) -> Marks {
    state
        .dir
        .as_deref()
        .map(library_marks::load)
        .unwrap_or_default()
}

/// Stars item `key` (`on`) or takes its star off; the marks as saved.
#[tauri::command(async)]
pub(crate) fn library_set_favorite(
    state: State<'_, LibraryMarksState>,
    key: String,
    on: bool,
) -> Result<Marks, String> {
    state.change(|dir| library_marks::set_favorite(dir, &key, on))
}

/// Notes that the library just added item `key` to the timeline; the marks as saved.
#[tauri::command(async)]
pub(crate) fn library_mark_used(
    state: State<'_, LibraryMarksState>,
    key: String,
) -> Result<Marks, String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    state.change(|dir| library_marks::mark_used(dir, &key, now_ms))
}
