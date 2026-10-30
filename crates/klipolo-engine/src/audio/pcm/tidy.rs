//! The cache folder's housekeeping: the disk budget (least recently used first) and leftovers from other runs.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// An unfinished file changed this recently may still be written (a process id can be reused).
const FRESH: Duration = Duration::from_secs(10 * 60);

/// Marks a cache file as recently used (the LRU budget goes by modification time).
pub(super) fn touch(path: &Path) {
    if let Ok(file) = File::options().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// An unfinished file (`<stem>.<pid>-<n>.tmp`, `<stem>.<pid>-<n>.peaks.tmp`) that another build may still be writing:
/// the app's or an export worker's. Its process is alive, or it changed in the last ten minutes.
fn in_progress(name: &str, meta: &fs::Metadata) -> bool {
    let pid = name
        .split('.')
        .nth(1)
        .and_then(|tag| tag.split('-').next())
        .and_then(|pid| pid.parse::<u32>().ok());
    if pid.is_some_and(klipolo_platform::process::is_alive) {
        return true;
    }
    meta.modified()
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_none_or(|age| age < FRESH)
}

/// Keeps the cache folder under `budget`: the least recently used files go first, except the stems `protected`
/// returns (worked out only when something has to go: the open project's, unfinished exports'). At startup it also
/// removes unfinished files no build is writing any more, and older cache formats.
pub(super) fn tidy(
    dir: &Path,
    startup: bool,
    budget: u64,
    protected: impl FnOnce() -> HashSet<String>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut stems: HashMap<String, (u64, SystemTime)> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "pcm" | "peaks" | "energy" => {
                let stem = name.split('.').next().unwrap_or("").to_string();
                let used = meta.modified().unwrap_or(UNIX_EPOCH);
                let slot = stems.entry(stem).or_insert((0, UNIX_EPOCH));
                slot.0 += meta.len();
                slot.1 = slot.1.max(used);
            }
            "tmp" if startup && !in_progress(&name, &meta) => {
                let _ = fs::remove_file(&path);
            }
            // The per-speed stereo f32 files.
            "f32" if startup => {
                let _ = fs::remove_file(&path);
            }
            _ => {}
        }
    }
    let mut total: u64 = stems.values().map(|(bytes, _)| bytes).sum();
    if total <= budget {
        return;
    }
    let keep = protected();
    let mut oldest: Vec<_> = stems.into_iter().collect();
    oldest.sort_by_key(|(_, (_, used))| *used);
    for (stem, (bytes, _)) in oldest {
        if total <= budget {
            break;
        }
        if keep.contains(&stem) {
            continue;
        }
        for ext in ["pcm", "peaks", "energy"] {
            let _ = fs::remove_file(dir.join(format!("{stem}.{ext}")));
        }
        total -= bytes;
    }
}
