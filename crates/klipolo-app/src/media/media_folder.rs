//! Where pictures Klipolo makes for a project are kept: next to the project file, in `<stem> Media/<maker>/` (for
//! `~/Movies/Klipolo/Trip.klipolo`, Muse Image's go in `~/Movies/Klipolo/Trip Media/Muse Image/`), one file each, named
//! `<short slug>-<n>.png`. A file is never overwritten and never left half-written: it is written to a hidden
//! `.partial` file, pushed to the disk, then linked into place under the first free name. A partial a crash left
//! behind is swept away by a later save once it is an hour old.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use klipolo_platform::durable::{self, Durability};
use klipolo_platform::paths::safe_file_name;

/// The most words of a prompt kept in a file's slug.
const SLUG_WORDS: usize = 4;
/// The longest slug.
const SLUG_CHARS: usize = 32;
/// The most names tried for one file (`<slug>-1.png` to `<slug>-9999.png`).
const MAX_NUMBER: u32 = 9999;
/// A partial this old is left over from a crash: no save takes this long.
const STALE_PARTIAL: Duration = Duration::from_secs(60 * 60);
/// How a partial's name ends.
const PARTIAL_SUFFIX: &str = ".png.partial";

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
    sweep_stale_partials(&folder);
    let partial = folder.join(format!(".{slug}-{}{PARTIAL_SUFFIX}", uuid::Uuid::new_v4()));
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

/// Removes the hidden partials in `folder` older than `STALE_PARTIAL`: what a crash left between writing a picture and
/// placing it. Anything else stays, and a partial that can't be read or removed is left for next time.
fn sweep_stale_partials(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with('.') && name.ends_with(PARTIAL_SUFFIX)) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age > STALE_PARTIAL)
            });
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn write_partial(partial: &Path, png: &[u8]) -> io::Result<()> {
    let mut file = File::create_new(partial)?;
    file.write_all(png)?;
    durable::sync(&file, Durability::Ordered)
}

/// Gives `partial` the first free `<slug>-<n>.png` name. A hard link fails rather than replace a file that is there,
/// so a name taken a moment ago (by another save, or by the person) is skipped, never overwritten. A disk that can't
/// link (exFAT, FAT, some network shares) claims the name with a new empty file first, which fails just the same when
/// the name is taken, and then moves the partial over that placeholder, which is its own.
fn place(partial: &Path, folder: &Path, slug: &str) -> io::Result<PathBuf> {
    place_with(
        &|from, to| fs::hard_link(from, to),
        &|from, to| fs::rename(from, to),
        partial,
        folder,
        slug,
    )
}

/// `place` with its link and rename steps given, so the way round a disk that can't link is tested too.
fn place_with(
    link: &dyn Fn(&Path, &Path) -> io::Result<()>,
    rename: &dyn Fn(&Path, &Path) -> io::Result<()>,
    partial: &Path,
    folder: &Path,
    slug: &str,
) -> io::Result<PathBuf> {
    for n in 1..=MAX_NUMBER {
        let target = folder.join(format!("{slug}-{n}.png"));
        match link(partial, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => match File::create_new(&target) {
                Ok(placeholder) => {
                    // Closed before the move: a network share (SMB) can refuse to replace a file that is held open.
                    // (`Ok(_)` wouldn't do: the match keeps an unmoved value alive until it ends.)
                    drop(placeholder);
                    if let Err(e) = rename(partial, &target) {
                        let _ = fs::remove_file(&target);
                        return Err(e);
                    }
                    return Ok(target);
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            },
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "every name for this picture is taken",
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    /// A disk that can't hard-link (exFAT, FAT, some network shares).
    fn no_link(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::Error::new(ErrorKind::Unsupported, "no hard links here"))
    }

    fn moved(from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn partial_with(folder: &Path, bytes: &[u8]) -> PathBuf {
        let partial = folder.join(format!(".cup-{}.png.partial", uuid::Uuid::new_v4()));
        fs::write(&partial, bytes).unwrap();
        partial
    }

    fn aged(path: &Path, age: std::time::Duration) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - age)
            .unwrap();
    }

    /// A crash between writing a partial and placing it leaves the partial behind; the next save takes away any older
    /// than an hour, and nothing else.
    #[test]
    fn a_save_sweeps_partials_left_over_an_hour_ago() {
        let dir = tempfile::tempdir().unwrap();
        let old = partial_with(dir.path(), b"left by a crash");
        aged(&old, std::time::Duration::from_secs(2 * 3600));
        let fresh = partial_with(dir.path(), b"another save, still going");
        let hidden = dir.path().join(".DS_Store");
        fs::write(&hidden, b"Finder's").unwrap();
        aged(&hidden, std::time::Duration::from_secs(2 * 3600));

        let saved = save_new_png(dir.path(), "cup", b"picture").unwrap();

        assert!(!old.exists(), "the old partial is still there");
        assert!(fresh.exists());
        assert!(hidden.exists());
        assert_eq!(fs::read(saved).unwrap(), b"picture");
    }

    #[test]
    fn a_disk_that_cannot_link_still_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("cup-1.png"), b"the person's own").unwrap();
        let partial = partial_with(dir.path(), b"new picture");

        let placed = place_with(&no_link, &moved, &partial, dir.path(), "cup").unwrap();

        assert_eq!(placed, dir.path().join("cup-2.png"));
        assert_eq!(
            fs::read(dir.path().join("cup-1.png")).unwrap(),
            b"the person's own"
        );
        assert_eq!(fs::read(&placed).unwrap(), b"new picture");
        assert!(!partial.exists());
    }

    /// Whether this process holds `file` open (Linux: its open files are links in `/proc/self/fd`).
    #[cfg(target_os = "linux")]
    fn held_open(file: &Path) -> bool {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .flatten()
            .any(|fd| fs::read_link(fd.path()).is_ok_and(|target| target == file))
    }

    /// A network share (SMB) can refuse to replace a file someone holds open, so the placeholder that claims the name is
    /// closed before the picture moves over it.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_placeholder_is_closed_before_the_picture_moves_over_it() {
        let dir = tempfile::tempdir().unwrap();
        let partial = partial_with(dir.path(), b"new picture");
        let open_at_rename = std::cell::Cell::new(None);
        let rename = |from: &Path, to: &Path| {
            open_at_rename.set(Some(held_open(to)));
            fs::rename(from, to)
        };

        let placed = place_with(&no_link, &rename, &partial, dir.path(), "cup").unwrap();

        assert_eq!(open_at_rename.get(), Some(false));
        assert_eq!(fs::read(placed).unwrap(), b"new picture");
    }

    /// Two pictures saved at once with the same slug on a disk that can't link each get a name of their own: neither
    /// replaces the other.
    #[test]
    fn two_saves_at_once_without_links_keep_both_pictures() {
        for _ in 0..200 {
            let dir = tempfile::tempdir().unwrap();
            let start = Arc::new(Barrier::new(2));
            let saves: Vec<_> = [b"first".as_slice(), b"second".as_slice()]
                .into_iter()
                .map(|bytes| {
                    let folder = dir.path().to_path_buf();
                    let partial = partial_with(&folder, bytes);
                    let start = Arc::clone(&start);
                    std::thread::spawn(move || {
                        start.wait();
                        place_with(&no_link, &moved, &partial, &folder, "cup").unwrap()
                    })
                })
                .collect();
            let mut placed: Vec<PathBuf> = saves.into_iter().map(|s| s.join().unwrap()).collect();
            placed.sort();
            assert_eq!(
                placed,
                [dir.path().join("cup-1.png"), dir.path().join("cup-2.png")]
            );
            let mut kept: Vec<Vec<u8>> = placed.iter().map(|p| fs::read(p).unwrap()).collect();
            kept.sort();
            assert_eq!(kept, [b"first".to_vec(), b"second".to_vec()]);
        }
    }
}
