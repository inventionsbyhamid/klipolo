//! What a quit or crash left in the store, removed at start: `.part` and temporary files, and in a folder whose
//! download didn't finish, any face that isn't recorded or no longer hashes to what was written. Finished faces stay
//! for the next download to resume from, unless Google's catalogue no longer offers that version: such a download can
//! never resume, so its folder goes whole. One folder's disk error never stops the others being tidied; the first is
//! said once all are done.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::files::{dirs, read_manifest, records, verified, write_partial};
use super::{FontStore, ManifestFile, PART, PARTIAL};

impl FontStore {
    /// `tidy_keeping` with every unfinished download still offered (tests, and a store without a catalogue).
    pub fn tidy(&self) -> io::Result<u32> {
        self.tidy_from(None)
    }

    /// Removes what a quit or crash left (each file counted), keeping the finished faces of unfinished downloads only
    /// at the `(family, version)`s Google's catalogue still `offered`. Run at start, before any download.
    pub fn tidy_keeping<'a>(
        &self,
        offered: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> io::Result<u32> {
        let folders: HashSet<PathBuf> = offered
            .into_iter()
            .map(|(family, version)| self.folder(family, version))
            .collect();
        self.tidy_from(Some(&folders))
    }

    fn tidy_from(&self, offered: Option<&HashSet<PathBuf>>) -> io::Result<u32> {
        let mut removed = 0;
        let mut first: Option<io::Error> = None;
        for family in dirs(&self.root) {
            for folder in dirs(&family) {
                let complete = read_manifest(&folder).is_some();
                let resumable = offered.is_none_or(|o| o.contains(&folder));
                let tidied = if complete || resumable {
                    tidy_folder(&folder, complete)
                } else {
                    remove_whole(&folder)
                };
                match tidied {
                    Ok(n) => removed += n,
                    Err(e) => {
                        first.get_or_insert(e);
                    }
                }
                // A folder left empty.
                let _ = fs::remove_dir(&folder);
            }
            let _ = fs::remove_dir(&family);
        }
        match first {
            Some(e) => Err(e),
            None => Ok(removed),
        }
    }
}

/// An unfinished download that can't resume: every file in its folder, counted, then the folder.
fn remove_whole(folder: &Path) -> io::Result<u32> {
    let count = fs::read_dir(folder)?.flatten().count() as u32;
    fs::remove_dir_all(folder)?;
    Ok(count)
}

/// One folder: its leftovers, and for an unfinished download any face no record proves.
fn tidy_folder(folder: &Path, complete: bool) -> io::Result<u32> {
    let mut removed = 0;
    let kept: Vec<ManifestFile> = if complete {
        Vec::new()
    } else {
        records(folder)
            .into_iter()
            .filter(|r| verified(folder, r))
            .collect()
    };
    for entry in fs::read_dir(folder)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let leftover = name.ends_with(PART) || name.ends_with(".tmp");
        let unproven = !complete && name.ends_with(".ttf") && !kept.iter().any(|r| r.file == name);
        if leftover || unproven {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    if !complete {
        if kept.is_empty() {
            let _ = fs::remove_file(folder.join(PARTIAL));
        } else {
            write_partial(folder, &kept)?;
        }
    }
    Ok(removed)
}
