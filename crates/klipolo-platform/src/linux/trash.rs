//! The freedesktop.org Trash (Trash specification 1.0), for `folder::move_to_trash`: a file manager lists what is moved
//! here and can put it back.
//! - A path on the home Trash's volume goes to `$XDG_DATA_HOME/Trash`, its `info/` entry naming it by its absolute path.
//! - A path on another volume goes to that volume's `$topdir/.Trash-<uid>` (moving it home would copy it across
//!   volumes), named relative to `$topdir`.
//!
//! The `.trashinfo` file is made first (`create_new`, which also claims the name), then the item is renamed into
//! `files/`; a rename that fails removes the claim again. Nothing is copied or deleted. `put_back` renames it out of
//! `files/` again and removes its `.trashinfo` (Start over's Undo).

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Moves `path` to its volume's Trash; where it went in `files/`.
pub(crate) fn move_to_trash(path: &Path) -> io::Result<Option<PathBuf>> {
    let path = std::path::absolute(path)?;
    let device = fs::symlink_metadata(&path)?.dev();
    let home = dirs::data_dir()
        .map(|data| data.join("Trash"))
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "no home Trash"))?;
    let home_device = home
        .ancestors()
        .find_map(|p| fs::metadata(p).ok())
        .map(|m| m.dev());
    if home_device == Some(device) {
        return move_into(&path, &home, &path, &deletion_date()).map(Some);
    }
    let top = top_dir(&path, device);
    // SAFETY: getuid never fails.
    let uid = unsafe { libc::getuid() };
    let trash = top.join(format!(".Trash-{uid}"));
    let named = path.strip_prefix(&top).unwrap_or(&path).to_path_buf();
    move_into(&path, &trash, &named, &deletion_date()).map(Some)
}

/// Puts `trashed` (an item in a Trash's `files/`) back at `to`, and removes its `.trashinfo` once it is out.
pub(crate) fn put_back(trashed: &Path, to: &Path) -> io::Result<()> {
    crate::unix::rename_new(trashed, to)?;
    if let (Some(files), Some(name)) = (trashed.parent(), trashed.file_name()) {
        let mut entry = OsString::from(name);
        entry.push(".trashinfo");
        let info = files.with_file_name("info").join(entry);
        if let Err(e) = fs::remove_file(&info)
            && e.kind() != ErrorKind::NotFound
        {
            tracing::warn!(error = %e, "a put-back item's .trashinfo stayed");
        }
    }
    Ok(())
}

/// Moves `path` into the trash folder `trash`, recorded as `named` (its `Path=`) and deleted at `deleted`
/// (`YYYY-MM-DDThh:mm:ss`, local time); where it went. A name taken already gets " 2", " 3"… after it.
fn move_into(path: &Path, trash: &Path, named: &Path, deleted: &str) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "nothing to trash"))?;
    let (files, info) = (trash.join("files"), trash.join("info"));
    for dir in [trash, &files, &info] {
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Err(e) if e.kind() != ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    for n in 1..=1_000u32 {
        let mut candidate = OsString::from(name);
        if n > 1 {
            candidate.push(format!(" {n}"));
        }
        let into = files.join(&candidate);
        if fs::symlink_metadata(&into).is_ok() {
            continue;
        }
        let mut entry = candidate.clone();
        entry.push(".trashinfo");
        let entry = info.join(entry);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&entry) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let text = format!(
            "[Trash Info]\nPath={}\nDeletionDate={deleted}\n",
            escaped(named)
        );
        let written = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all());
        let moved = written.and_then(|()| fs::rename(path, &into));
        if let Err(e) = moved {
            let _ = fs::remove_file(&entry);
            return Err(e);
        }
        return Ok(into);
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "the Trash holds too many of that name",
    ))
}

/// The top of `path`'s volume: its highest ancestor on the same device.
fn top_dir(path: &Path, device: u64) -> PathBuf {
    let mut top = path.to_path_buf();
    for p in path.ancestors().skip(1) {
        match fs::metadata(p) {
            Ok(m) if m.dev() == device => top = p.to_path_buf(),
            _ => break,
        }
    }
    top
}

/// `Path=` as the spec writes it: every byte but unreserved ones and `/` as `%XX`.
fn escaped(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Now in local time, as `DeletionDate=` takes it.
fn deletion_date() -> String {
    // SAFETY: `time` and `localtime_r` with our own out-struct.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "1970-01-01T00:00:00".into();
        }
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A take's folder goes to `files/` whole with its `.trashinfo`; a second of the same name gets its own; nothing is
    /// deleted. The Trash here is a test folder, never the person's.
    #[test]
    fn a_folder_moves_to_the_trash_with_its_info_and_names_never_clash() {
        let dir = tempfile::tempdir().unwrap();
        let trash = dir.path().join("Trash");
        let take = |name: &str| {
            let take = dir.path().join(name);
            fs::create_dir(&take).unwrap();
            fs::write(take.join("session.json"), b"{}").unwrap();
            take
        };
        let first = take("my take.klipolorec");
        move_into(&first, &trash, &first, "2026-10-16T10:00:00").unwrap();
        assert!(!first.exists());
        let kept = trash.join("files/my take.klipolorec/session.json");
        assert_eq!(fs::read(kept).unwrap(), b"{}");
        let info = fs::read_to_string(trash.join("info/my take.klipolorec.trashinfo")).unwrap();
        assert_eq!(
            info,
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-10-16T10:00:00\n",
                first.to_string_lossy().replace(' ', "%20")
            )
        );
        let second = take("my take.klipolorec");
        let went = move_into(&second, &trash, &second, "2026-10-16T10:01:00").unwrap();
        assert_eq!(went, trash.join("files/my take.klipolorec 2"));
        assert!(went.join("session.json").is_file());
        assert!(trash.join("info/my take.klipolorec 2.trashinfo").is_file());
        assert!(
            trash
                .join("files/my take.klipolorec/session.json")
                .is_file()
        );
        assert_eq!(deletion_date().len(), "2026-10-16T10:00:00".len());
        // Put back (Start over's Undo): out of `files/` with its info gone, never over something already there.
        put_back(&went, &second).unwrap();
        assert!(second.join("session.json").is_file());
        assert!(!trash.join("info/my take.klipolorec 2.trashinfo").exists());
        let kept = trash.join("files/my take.klipolorec");
        let refused = put_back(&kept, &second).unwrap_err();
        assert_eq!(refused.kind(), ErrorKind::AlreadyExists);
        assert!(kept.join("session.json").is_file(), "nothing moved");
    }
}
