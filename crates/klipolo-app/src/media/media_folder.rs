//! Where pictures Klipolo makes for a project are kept: next to the project file, in `<stem> Media/<maker>/` (for
//! `~/Movies/Klipolo/Trip.klipolo`, Muse Image's go in `~/Movies/Klipolo/Trip Media/Muse Image/`), one file each, named
//! `<short slug>-<n>.png`. A file is never overwritten and never left half-written: it is written to a hidden
//! `.partial` file, pushed to the disk, then linked into place under the first free name.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use klipolo_platform::durable::{self, Durability};
use klipolo_platform::paths::safe_file_name;

/// The most words of a prompt kept in a file's slug.
const SLUG_WORDS: usize = 4;
/// The longest slug.
const SLUG_CHARS: usize = 32;
/// The most names tried for one file (`<slug>-1.png` to `<slug>-9999.png`).
const MAX_NUMBER: u32 = 9999;

/// The folder for pictures `made_with` makes for the project saved in `project_file`.
pub fn made_folder(project_file: &Path, made_with: &str) -> PathBuf {
    let stem = project_file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let project_folder = project_file.parent().unwrap_or(Path::new("."));
    project_folder
        .join(safe_file_name(&format!("{stem} Media")))
        .join(safe_file_name(made_with))
}

/// A short file name part from `text`: its first words, lowercase ASCII letters and digits joined by hyphens, at most
/// 32 characters; `fallback` when nothing is left. It never holds a separator or a dot, so it stays in its folder.
pub fn slug(text: &str, fallback: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(SLUG_WORDS)
        .map(str::to_ascii_lowercase)
        .collect();
    let mut slug = words.join("-");
    slug.truncate(SLUG_CHARS);
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        fallback.to_owned()
    } else {
        slug.to_owned()
    }
}

/// Saves `png` in `folder` (made when missing) as `<slug>-<n>.png` with the first free `n`, and returns its absolute,
/// canonical path. The bytes reach the disk before the name appears; a failure leaves no file behind.
pub fn save_new_png(folder: &Path, slug: &str, png: &[u8]) -> io::Result<PathBuf> {
    fs::create_dir_all(folder)?;
    let folder = folder.canonicalize()?;
    let partial = folder.join(format!(".{slug}-{}.png.partial", uuid::Uuid::new_v4()));
    let placed = write_partial(&partial, png).and_then(|()| place(&partial, &folder, slug));
    // Linked into place, or failed: either way the partial goes.
    let _ = fs::remove_file(&partial);
    let file = placed?;
    // The file is whole and on the disk; only the new name's own durability is left (Linux syncs the folder).
    if let Err(error) = durable::fsync_dir(&folder) {
        tracing::warn!(%error, "a picture's folder couldn't be synced");
    }
    Ok(file)
}

fn write_partial(partial: &Path, png: &[u8]) -> io::Result<()> {
    let mut file = File::create_new(partial)?;
    file.write_all(png)?;
    durable::sync(&file, Durability::Ordered)
}

/// Gives `partial` the first free `<slug>-<n>.png` name. A hard link fails rather than replace a file that is there,
/// so a name taken a moment ago (by another save, or by the person) is skipped, never overwritten. A disk that can't
/// link gets a rename after a check instead.
fn place(partial: &Path, folder: &Path, slug: &str) -> io::Result<PathBuf> {
    for n in 1..=MAX_NUMBER {
        let target = folder.join(format!("{slug}-{n}.png"));
        match fs::hard_link(partial, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) if !target.exists() => {
                fs::rename(partial, &target)?;
                return Ok(target);
            }
            Err(_) => continue,
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "every name for this picture is taken",
    ))
}
