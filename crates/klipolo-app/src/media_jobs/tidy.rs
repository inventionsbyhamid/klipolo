//! The proxies folder's budget (`PROXY_DISK_BUDGET`, least recently used first).

use super::*;

/// Tidies the proxies folder on a utility thread (`tidy_proxies`).
pub(super) fn tidy_in_background(root: PathBuf, keep: HashSet<String>) {
    let spawned = std::thread::Builder::new()
        .name("klipolo-copies-tidy".into())
        .spawn(move || {
            klipolo_platform::qos::set_current(klipolo_platform::qos::Qos::Utility);
            tidy_proxies(&root, PROXY_DISK_BUDGET, &keep);
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "editing copies: the tidy couldn't start");
    }
}

/// Keeps the proxies folder under `budget`: finished copies and unfinished jobs go least recently used first, never one
/// `keep` names (the open project's, and the running job). A job folder whose source is there but changed (its key no
/// longer matches) can never be resumed and goes at once; one whose source is missing now (an unplugged drive) stays.
/// A copy goes with its record and keyframe index beside it (`remove_copy`); a record or index left without its copy
/// goes at once. O(entries) stats, plus a job folder's files.
pub fn tidy_proxies(root: &Path, budget: u64, keep: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    // (stem, bytes, last used, path, job folder)
    let mut found: Vec<(String, u64, SystemTime, PathBuf, bool)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".mp4").filter(|_| path.is_file()) {
            let Ok(meta) = entry.metadata() else { continue };
            let used = meta.modified().unwrap_or(UNIX_EPOCH);
            // Its record and index count with it.
            let beside: u64 = [
                record_path(&path),
                klipolo_engine::media::keyframes::beside(&path),
            ]
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
            found.push((stem.to_owned(), meta.len() + beside, used, path, false));
        } else if let Some(stem) = name
            .strip_suffix(".json")
            .or_else(|| name.strip_suffix(".keys"))
            .filter(|_| path.is_file())
        {
            // A record or index whose copy is gone (a crash between the two, or an older removal).
            let copy = root.join(format!("{stem}.mp4"));
            if !copy.exists() && !keep.contains(stem) {
                let _ = std::fs::remove_file(&path);
            }
        } else if let Some(stem) = name.strip_suffix(".job").filter(|_| path.is_dir()) {
            if let Ok(job) = ProxyJob::open(&path) {
                let now = klipolo_engine::media::key::stem(
                    &job.source,
                    &CacheKind::Proxy {
                        long_side: job.long_side,
                    },
                );
                if now.is_some_and(|now| now != job.stem) && !keep.contains(stem) {
                    remove_job(&path);
                    continue;
                }
            }
            let mut bytes = 0;
            let mut used = UNIX_EPOCH;
            for file in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                if let Ok(meta) = file.metadata() {
                    bytes += meta.len();
                    used = used.max(meta.modified().unwrap_or(UNIX_EPOCH));
                }
            }
            found.push((stem.to_owned(), bytes, used, path, true));
        }
    }
    let mut total: u64 = found.iter().map(|f| f.1).sum();
    if total <= budget {
        return;
    }
    found.sort_by_key(|f| f.2);
    for (stem, bytes, _, path, is_job) in found {
        if total <= budget {
            break;
        }
        if keep.contains(&stem) {
            continue;
        }
        if is_job {
            remove_job(&path);
        } else {
            remove_copy(&path);
            if path.exists() {
                continue;
            }
        }
        total = total.saturating_sub(bytes);
    }
}
