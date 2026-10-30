//! A version folder's files as read: never through a symbolic link (a Windows reparse point, junctions included),
//! never past a file's cap, never anything but a plain file, and nothing but the names a version folder holds.

use std::collections::BTreeMap;
use std::fs::{File, Metadata, OpenOptions};
use std::io::Read;
use std::path::Path;

use super::{PackError, PackLimits, quoted};

/// Entries a version folder's listing reads at most: its five files, and the Finder's own, which are skipped.
pub(super) const MAX_LISTED: usize = 16;

/// Whether `meta` (`symlink_metadata`'s, or an open file's) is a link rather than the thing itself.
pub(super) fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// `dir` is a folder itself, not a link to one.
pub(super) fn plain_folder(dir: &Path, name: &str) -> Result<(), PackError> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| io(name, &e))?;
    if is_link(&meta) || !meta.is_dir() {
        return Err(PackError::Link(quoted(name)));
    }
    Ok(())
}

/// What the macOS Finder (`.DS_Store`) and copies to other volumes (`._<name>`) leave in any folder a person opens:
/// never read and never counted, so opening a project's folder in the Finder doesn't break its effects.
pub(super) fn os_litter(name: &str) -> bool {
    name == ".DS_Store" || name.starts_with("._")
}

/// The version folder's files by name, each read within its cap.
pub(super) fn version_files(
    dir: &Path,
    limits: &PackLimits,
) -> Result<BTreeMap<String, Vec<u8>>, PackError> {
    let mut files = BTreeMap::new();
    let listing = std::fs::read_dir(dir).map_err(|e| io("the version folder", &e))?;
    for (n, entry) in listing.enumerate() {
        if n >= MAX_LISTED {
            return Err(PackError::Extra(format!("more than {MAX_LISTED} entries")));
        }
        let entry = entry.map_err(|e| io("the version folder", &e))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(PackError::Extra(quoted(&name.to_string_lossy())));
        };
        if os_litter(name) {
            continue;
        }
        let Some(limit) = limits.of(name) else {
            return Err(PackError::Extra(quoted(name)));
        };
        files.insert(name.to_owned(), read_capped(&dir.join(name), limit)?);
    }
    Ok(files)
}

/// The plain file at `path`, at most `limit` bytes: never through a link at its last step, never a FIFO, a device or a
/// folder, and never more than `limit + 1` bytes read whatever the file grows to meanwhile.
pub fn read_capped(path: &Path, limit: u64) -> Result<Vec<u8>, PackError> {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let meta = std::fs::symlink_metadata(path).map_err(|e| io(&name, &e))?;
    if is_link(&meta) || !meta.is_file() {
        return Err(PackError::Link(quoted(&name)));
    }
    let too_large = || PackError::TooLarge {
        file: quoted(&name),
        limit,
    };
    if meta.len() > limit {
        return Err(too_large());
    }
    let file = open_no_follow(path).map_err(|e| io(&name, &e))?;
    let opened = file.metadata().map_err(|e| io(&name, &e))?;
    if is_link(&opened) || !opened.is_file() {
        return Err(PackError::Link(quoted(&name)));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io(&name, &e))?;
    if bytes.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(bytes)
}

/// `path` opened to read, refusing a link at its last step: `O_NOFOLLOW` on Unix (with `O_NONBLOCK`, so a FIFO put
/// there after the check doesn't hang the open), the reparse point itself on Windows (then refused as a link).
pub(super) fn open_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

pub(super) fn io(what: &str, e: &std::io::Error) -> PackError {
    PackError::Io(format!("{} can't be read ({})", quoted(what), e.kind()))
}
