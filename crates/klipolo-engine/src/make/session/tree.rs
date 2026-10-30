//! The session's files as the parent sees them: a walk of what the worker wrote that fails
//! closed, and a removal that a folder the worker made unreadable can't stop. Neither follows a link.

use std::io;
use std::path::{Path, PathBuf};

use klipolo_platform::process::sandbox::unlock_for_removal;

use super::Written;

/// One entry a folder lists, as it is on disk: a link is an entry, never what it names.
pub(super) struct Entry {
    pub path: PathBuf,
    pub folder: bool,
    pub len: u64,
}

pub(super) type Listing = Box<dyn Iterator<Item = io::Result<Entry>>>;

/// `folder`'s entries (`symlink_metadata`).
pub(super) fn list(folder: &Path) -> io::Result<Listing> {
    let entries = std::fs::read_dir(folder)?;
    Ok(Box::new(entries.map(|entry| {
        let path = entry?.path();
        let meta = path.symlink_metadata()?;
        Ok(Entry {
            folder: meta.is_dir(),
            len: if meta.is_dir() { 0 } else { meta.len() },
            path,
        })
    })))
}

/// How deep the walk lets a worker nest folders below `tmp/` or `out/`: deeper counts as over the cap.
pub(super) const MAX_DEPTH: usize = 32;

/// What `roots` hold, as `list` lists them; counting stops past `max_entries`. Anything it can't read (a folder it
/// can't list, an entry it can't describe) makes the result `unreadable`, and a folder more than `MAX_DEPTH` below a root
/// makes it `too_deep`: both count as over the cap, so a worker can't hide what it wrote. What vanished meanwhile
/// (`NotFound`: the worker deleted it) is skipped.
pub(super) fn walk(
    roots: &[PathBuf],
    max_entries: usize,
    mut list: impl FnMut(&Path) -> io::Result<Listing>,
) -> Written {
    let mut written = Written::default();
    let mut folders: Vec<(PathBuf, usize)> = roots.iter().map(|root| (root.clone(), 0)).collect();
    while let Some((folder, depth)) = folders.pop() {
        let entries = match list(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                written.unreadable = true;
                return written;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => {
                    written.unreadable = true;
                    return written;
                }
            };
            written.entries += 1;
            if written.entries > max_entries {
                return written;
            }
            if !entry.folder {
                written.bytes += entry.len;
            } else if depth + 1 > MAX_DEPTH {
                written.too_deep = true;
                return written;
            } else {
                folders.push((entry.path, depth + 1));
            }
        }
    }
    written
}

/// A removal never works deeper than this below its root: a folder found there is first moved up into the root (a
/// rename inside the session, `.up-<n>`). So std's `remove_dir_all`, which keeps a descriptor open per level, holds at
/// most about `HOIST + 2` whatever the depth the worker made, and no path grows past the root plus `HOIST` names.
const HOIST: usize = 3;

/// Deletes `root` and everything under it, never following a link, within a few descriptors (`HOIST`). On any error it
/// tries once more; with `unlock` (the worker is gone) it first gives this user back every entry the worker may
/// have locked (`unlock_for_removal`: BSD flags, ACLs, folder modes). Only a root that is already gone counts as done
///: a `NotFound` from inside the tree is a failure to retry, not success.
///
/// The moves (`flatten`'s renames) work by path, so they are safe only while nothing else changes the tree. They
/// also run with `unlock` false, in a sweep whose worker has no record or whose kill wasn't confirmed, and that is safe
/// too: an unrecorded worker never got a request (the parent records it before the first), so it never ran agent
/// code, and an unconfirmed one was already sent SIGKILL (`session::stop_worker`). Every other caller runs once the
/// worker has exited.
pub(super) fn remove_tree(root: &Path, unlock: bool) -> io::Result<()> {
    let gone = |root: &Path| {
        root.symlink_metadata()
            .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
    };
    match flatten(root, false).and_then(|()| std::fs::remove_dir_all(root)) {
        Ok(()) => Ok(()),
        Err(_) if gone(root) => Ok(()),
        Err(_) => {
            if unlock {
                unlock_for_removal(root);
            }
            flatten(root, unlock)?;
            std::fs::remove_dir_all(root)
        }
    }
}

/// Moves every folder `HOIST` levels below `root` up into `root`, until none is that deep; with `unlock`, unlocks every
/// entry before it is looked into or moved. One listing open at a time, streamed: only its folders are kept, and
/// only until that listing is done. It fails closed: an entry it can't read is an error, an entry whose type it
/// can't tell is taken for a folder, and a folder that vanished below the root is an error, not skipped.
fn flatten(root: &Path, unlock: bool) -> io::Result<()> {
    let mut next = 0usize;
    let mut folders = vec![(root.to_path_buf(), 0usize)];
    while let Some((folder, depth)) = folders.pop() {
        let listing = match std::fs::read_dir(&folder) {
            Ok(listing) => listing,
            Err(e) if e.kind() == io::ErrorKind::NotFound && depth == 0 => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut inner = Vec::new();
        for entry in listing {
            let entry = entry?;
            let path = entry.path();
            if unlock {
                unlock_for_removal(&path);
            }
            if entry.file_type().map_or(true, |t| t.is_dir()) {
                inner.push(path);
            }
        }
        for path in inner {
            if depth + 1 < HOIST {
                folders.push((path, depth + 1));
                continue;
            }
            let up = loop {
                let up = root.join(format!(".up-{next}"));
                next += 1;
                if up.symlink_metadata().is_err() {
                    break up;
                }
            };
            std::fs::rename(&path, &up)?;
            folders.push((up, 1));
        }
    }
    Ok(())
}
