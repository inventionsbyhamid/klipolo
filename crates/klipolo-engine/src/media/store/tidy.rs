//! The thumbnails folder's housekeeping: the disk budget (least recently used files first) and, at app start, atomic
//! writes a crash or a force quit cut off.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_platform::durable::temp_owner;

use super::DISK_BUDGET;
use crate::media::key::{CacheKind, stem};

/// What tells when a file's sprites were last used (the LRU order).
pub(super) const STRIP_FILE: &str = "strip.json";

/// Marks a file as recently used (the LRU order goes by modification time).
pub(super) fn touch(path: &Path) {
    if let Ok(file) = fs::File::options().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// The thumbnails folder at app start: atomic writes a crash or force quit cut off (never a live process's), and the
/// disk budget (unfinished exports' files kept). The app runs it once, on its own thread.
pub fn tidy_at_start(root: &Path, jobs_root: Option<&Path>) {
    tidy(root, DISK_BUDGET, true, || {
        jobs_root
            .map(crate::media::export_job_sources)
            .unwrap_or_default()
            .iter()
            .filter_map(|p| stem(p, &CacheKind::Thumbs))
            .collect()
    });
}

/// Deletes whole `<stem>/` folders, least recently used first, until the folder is under `budget`; `protected` (the
/// stems to keep) is only worked out when something has to go. At `startup` it also removes leftover temporary files.
pub(crate) fn tidy(
    root: &Path,
    budget: u64,
    startup: bool,
    protected: impl FnOnce() -> HashSet<String>,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut folders: Vec<(String, u64, SystemTime)> = Vec::new();
    for entry in entries.flatten().filter(|e| e.path().is_dir()) {
        let mut bytes = 0;
        for file in fs::read_dir(entry.path()).into_iter().flatten().flatten() {
            let Ok(meta) = file.metadata() else { continue };
            let name = file.file_name().to_string_lossy().into_owned();
            if startup
                && name.ends_with(".tmp")
                && klipolo_platform::process::is_leftover(temp_owner(&name), meta.modified().ok())
            {
                let _ = fs::remove_file(file.path());
                continue;
            }
            bytes += meta.len();
        }
        let used = fs::metadata(entry.path().join(STRIP_FILE))
            .or_else(|_| entry.metadata())
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        folders.push((
            entry.file_name().to_string_lossy().into_owned(),
            bytes,
            used,
        ));
    }
    let mut total: u64 = folders.iter().map(|f| f.1).sum();
    if total <= budget {
        return;
    }
    let keep = protected();
    folders.sort_by_key(|f| f.2);
    for (name, bytes, _) in folders {
        if total <= budget {
            break;
        }
        if keep.contains(&name) {
            continue;
        }
        if fs::remove_dir_all(root.join(&name)).is_ok() {
            total -= bytes;
        }
    }
}
