//! A folder followed by what it is, not by where it was: an open project's folder renamed or moved in Finder is found
//! where it went, and one that is deleted is gone, so nothing is ever made again in its old place. A folder in the
//! Trash is gone too, not moved (`Trash`): what is saved there is lost when the Trash is emptied. Put back, it is found
//! again.
//! - macOS: a descriptor on the folder, opened with `O_EVTONLY` (it never stops its volume from being ejected), answers
//!   `F_GETPATH`; the answer counts only while the folder at that path is the held one (same device and file number),
//!   since a deleted folder may still name its last place;
//! - W: a handle opened with `FILE_FLAG_BACKUP_SEMANTICS` and every share mode (so it never blocks a rename or a
//!   delete), answering `GetFinalPathNameByHandleW`;
//! - L: an `O_PATH` descriptor, read back through `/proc/self/fd`, checked like macOS's.
//!
//! Until W and L (and wherever a folder can't be held), it is followed by its path: found where it was, else gone. A
//! folder renamed there reads as gone, which loses nothing: the caller keeps what it would have saved.

use std::path::{Component, Path, PathBuf};

/// Where deleted files wait until the Trash is emptied. A followed folder found there counts as gone.
/// - macOS: any folder named `.Trash` (the home Trash, iCloud Drive's) or `.Trashes` (other volumes'): Finder keeps
///   those names for itself;
/// - W: `$Recycle.Bin`;
/// - L: `$XDG_DATA_HOME/Trash`, and `.Trash` or `.Trash-<uid>` at a volume's top (the XDG Trash spec).
///
/// No file is read to tell: only the path's names are compared.
#[derive(Clone, Debug)]
pub struct Trash {
    /// Folders that are a Trash (`Trash::at`'s, or the OS's own).
    roots: Vec<PathBuf>,
    /// The OS's own Trash names count too (not in `Trash::at`'s).
    by_name: bool,
}

impl Trash {
    /// The OS's Trash.
    pub fn current() -> Self {
        Self {
            roots: crate::imp::trash_roots(),
            by_name: true,
        }
    }

    /// Only `root` (tests: a Trash-like folder, never the real one). Resolved like `HeldFolder::locate`'s answers
    /// (symlinks such as macOS's /var), so a folder moved into it is seen there.
    pub fn at(root: &Path) -> Self {
        Self {
            roots: vec![root.canonicalize().unwrap_or_else(|_| root.to_path_buf())],
            by_name: false,
        }
    }

    /// `path` is in this Trash. O(path components).
    pub fn holds(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
            || (self.by_name
                && path.components().any(|part| {
                    matches!(part, Component::Normal(name) if crate::imp::is_trash_name(name))
                }))
    }
}

/// Moves `path` (a file or a folder) to the OS's Trash, where the person can put it back: Delete… on an unfinished
/// recording (the UI asks first). Nothing is deleted.
/// - Linux: the freedesktop.org Trash (Trash spec 1.0): the home Trash (`$XDG_DATA_HOME/Trash`) for a path on its
///   volume, else that volume's `.Trash-<uid>`; the item goes to `files/` with its `info/<name>.trashinfo`;
/// - Windows: `IFileOperation` with `FOFX_RECYCLEONDELETE`, no UI, and a delete that wouldn't recycle (too big for the
///   Recycle Bin, a drive without one) refused: `FileTooLarge`, the item kept;
/// - macOS: `NSFileManager trashItemAtURL:`, as Finder does, so Put Back works.
pub fn move_to_trash(path: &Path) -> std::io::Result<()> {
    move_to_trash_at(path).map(|_| ())
}

/// `move_to_trash`, and where the item went in the Trash when the OS says (macOS, Linux; None on Windows, whose Recycle
/// Bin names its items itself): Start over's Undo puts it back from there (`put_back`).
pub fn move_to_trash_at(path: &Path) -> std::io::Result<Option<PathBuf>> {
    crate::imp::move_to_trash(path)
}

/// Puts an item `move_to_trash_at` moved to `trashed` back at `to`, as Put Back does: Start over's Undo.
/// Nothing is copied or overwritten: AlreadyExists when something is at `to`, NotFound when the Trash was emptied.
/// Linux also removes the item's `.trashinfo`; Windows says Unsupported until W.
pub fn put_back(trashed: &Path, to: &Path) -> std::io::Result<()> {
    crate::imp::put_back(trashed, to)
}

/// A folder, held so that [`HeldFolder::locate`] finds it after a rename or a move on its volume.
pub struct HeldFolder {
    /// Where it was when held: the fallback when it couldn't be held.
    at: PathBuf,
    held: Option<crate::imp::Held>,
    trash: Trash,
}

impl HeldFolder {
    /// Holds the folder at `path`. One that can't be held (it isn't there, or the OS can't follow folders yet) is
    /// followed by its path.
    pub fn hold(path: &Path) -> Self {
        Self::hold_with(path, Trash::current())
    }

    /// [`HeldFolder::hold`], with `trash` as the Trash (tests).
    pub fn hold_with(path: &Path, trash: Trash) -> Self {
        let held = crate::imp::hold_folder(path)
            .map_err(|error| tracing::warn!(%error, "a folder couldn't be held; it is followed by its path"))
            .ok()
            .flatten();
        Self {
            at: path.to_path_buf(),
            held,
            trash,
        }
    }

    /// Where the folder is now; None when it is gone (deleted, in the Trash, or its volume ejected). A few µs: one
    /// `fcntl` and two `stat`s on macOS.
    pub fn locate(&self) -> Option<PathBuf> {
        let now = match &self.held {
            Some(held) => crate::imp::locate_folder(held),
            None => self.at.is_dir().then(|| self.at.clone()),
        };
        now.filter(|path| !self.trash.holds(path))
    }

    /// True when renames and moves are followed, not just the path.
    pub fn follows(&self) -> bool {
        self.held.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_folder_is_found_until_it_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("project");
        std::fs::create_dir(&folder).unwrap();
        let held = HeldFolder::hold(&folder);
        let here = held.locate().expect("found where it is");
        assert_eq!(here.canonicalize().unwrap(), folder.canonicalize().unwrap());
        std::fs::remove_dir(&folder).unwrap();
        assert_eq!(held.locate(), None);
        // A new folder in its place isn't it.
        std::fs::create_dir(&folder).unwrap();
        if held.follows() {
            assert_eq!(held.locate(), None);
        }
        // A folder that isn't there is held by its path.
        let missing = HeldFolder::hold(&dir.path().join("missing"));
        assert!(!missing.follows());
        assert_eq!(missing.locate(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_renamed_or_moved_folder_is_found_where_it_went() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("project");
        std::fs::create_dir(&folder).unwrap();
        let held = HeldFolder::hold(&folder);
        assert!(held.follows());
        let canonical = |p: &Path| p.canonicalize().unwrap();
        let renamed = dir.path().join("renamed");
        std::fs::rename(&folder, &renamed).unwrap();
        assert_eq!(
            held.locate().map(|p| canonical(&p)),
            Some(canonical(&renamed))
        );
        let elsewhere = dir.path().join("Elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let moved = elsewhere.join("project");
        std::fs::rename(&renamed, &moved).unwrap();
        assert_eq!(
            held.locate().map(|p| canonical(&p)),
            Some(canonical(&moved))
        );
        std::fs::remove_dir(&moved).unwrap();
        assert_eq!(held.locate(), None);
    }

    /// A folder moved into the Trash is gone, not followed there; put back, it is found again. The Trash here is a
    /// test folder (`Trash::at`), never the real one.
    #[test]
    fn a_folder_in_the_trash_is_gone_until_it_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("Bin");
        std::fs::create_dir(&bin).unwrap();
        let folder = dir.path().join("project");
        std::fs::create_dir(&folder).unwrap();
        let held = HeldFolder::hold_with(&folder, Trash::at(&bin));
        assert!(held.locate().is_some());
        std::fs::rename(&folder, bin.join("project")).unwrap();
        assert_eq!(held.locate(), None);
        std::fs::rename(bin.join("project"), &folder).unwrap();
        let back = held.locate().expect("found again");
        assert_eq!(back.canonicalize().unwrap(), folder.canonicalize().unwrap());
    }

    /// The OS's Trash, by name only (no file is touched).
    #[test]
    fn the_oss_trash_is_known_by_its_names() {
        let trash = Trash::current();
        let ours = Path::new("/home/me/Videos/Klipolo/Projects/p");
        assert!(!trash.holds(ours));
        #[cfg(target_os = "macos")]
        for path in [
            "/Users/me/.Trash/p",
            "/Volumes/Drive/.Trashes/501/p",
            "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/.Trash/p",
        ] {
            assert!(trash.holds(Path::new(path)), "{path}");
        }
        #[cfg(target_os = "linux")]
        for path in [
            "/media/me/Drive/.Trash-1000/files/p",
            "/media/me/Drive/.Trash/1000/files/p",
        ] {
            assert!(trash.holds(Path::new(path)), "{path}");
        }
    }
}
