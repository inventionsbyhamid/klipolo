//! Screen files that can't be analysed: marked so in the activity folder
//! (`files::failed_mark`), which the editor's store and an export's share. An export that couldn't analyse a file
//! planned without its activity, so the editor plans without it too, and so does every later run of that export: the
//! mark holds for as long as the file is the same (its stem: path, size and modification time). The editor's own pass
//! marks a file it can't analyse the same way, so an export never tries it again.

use super::{ActivityStore, lock};
use crate::media::activity::files::failed_mark;
use crate::media::key::{CacheKind, stem};

impl ActivityStore {
    /// `path` can't be analysed: marked so in the folder (once; the screen file must be there to name it), and its
    /// track (part-made) let go here, so this store's plans read none. What the file's pass made stays on disk.
    pub fn fail(&self, path: &str) {
        if let Some(stem) = stem(path, &CacheKind::Activity)
            && !self.marked(&stem)
        {
            let mark = failed_mark(&self.root, &stem);
            let made =
                std::fs::create_dir_all(&self.root).and_then(|()| std::fs::write(&mark, b""));
            if let Err(error) = made {
                tracing::warn!(%error, file = %crate::log_name(path), "screen activity: its failure couldn't be noted");
            }
        }
        self.forget(path);
    }

    /// `path` is marked as a screen file that can't be analysed (`fail`, by this process or another), for as long as it
    /// is the same file: plans go without its activity for good, so chat counts it as known, with none. A stat of the
    /// file, and one of its mark.
    pub fn failed_for_good(&self, path: &str) -> bool {
        stem(path, &CacheKind::Activity).is_some_and(|stem| self.marked(&stem))
    }

    /// The file of `stem` is marked as one that can't be analysed (by this process or another). One stat.
    pub(super) fn marked(&self, stem: &str) -> bool {
        failed_mark(&self.root, stem).exists()
    }

    /// `path`'s track goes from memory (freed off the lock), and that is said, so plans read none for it.
    pub(super) fn forget(&self, path: &str) {
        let gone = lock(&self.state).tracks.remove(path);
        if gone.is_some() {
            drop(gone);
            self.said(path);
        }
    }
}
