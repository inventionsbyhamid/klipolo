//! Online name previews: Google's small `menu` file of a family, after the person's consent, cached
//! on disk (≤ 20 MB, the oldest used first out). One short try (10 s, no retry): a preview
//! is a nicety, never worth a thread held up on a bad network.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use klipolo_platform::durable::{self, Durability};

use super::FontError;
use super::net::{Hosts, client, fetch_small};
use crate::google::GoogleFamily;
use crate::store::plain_name;

/// How long a preview may take.
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(10);

/// The online preview of `fam`'s name, from `cache` if it's there, else fetched into it (`<slug>-<version>.ttf`) and
/// checked to be a font; the cache keeps at most `cache_max` bytes. Call it only after the person's consent, on a
/// blocking thread.
pub fn fetch_preview(
    fam: &GoogleFamily,
    cache: &Path,
    cache_max: u64,
) -> Result<Vec<u8>, FontError> {
    if !plain_name(&fam.version) {
        return Err(FontError::BadFile("address".into()));
    }
    let name = format!(
        "{}-{}.ttf",
        crate::store::folder_name(&fam.family),
        fam.version
    );
    let path = cache.join(&name);
    if let Ok(bytes) = fs::read(&path)
        && ttf_parser::Face::parse(&bytes, 0).is_ok()
    {
        // Used now: the last to go.
        if let Ok(f) = File::options().write(true).open(&path) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        return Ok(bytes);
    }
    let hosts = Hosts::of(fam);
    let u = hosts.file(&fam.menu)?;
    let bytes = fetch_small(
        &client(&hosts)?,
        &u,
        &AtomicBool::new(false),
        PREVIEW_TIMEOUT,
        0,
    )?;
    if ttf_parser::Face::parse(&bytes, 0).is_err() {
        return Err(FontError::BadFile("preview".into()));
    }
    fs::create_dir_all(cache).map_err(|e| FontError::disk(&e))?;
    durable::write_atomic(&path, &bytes, Durability::Ordered).map_err(|e| FontError::disk(&e))?;
    trim_cache(cache, cache_max);
    Ok(bytes)
}

/// The cache's files past `max` bytes go, the least recently used first.
fn trim_cache(cache: &Path, max: u64) {
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            md.is_file().then(|| {
                (
                    md.modified().unwrap_or(std::time::UNIX_EPOCH),
                    md.len(),
                    e.path(),
                )
            })
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, n, _)| n).sum();
    files.sort();
    for (_, n, path) in files {
        if total <= max {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= n;
        }
    }
}
