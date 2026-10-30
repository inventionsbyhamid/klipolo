//! Where the app runs from on macOS (`statfs`, `access`, the bundle's flags), and Move to Applications' steps
//! (`ditto`, then `xattr`; the Trash; `renamex_np`) under `install::moving`.

use std::cmp::Ordering;
use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::install::moving::{APP_NAME, BundleInfo, Folder, Mover, folder_for, move_into};
use crate::install::{InstallPlace, MoveError, PlaceFacts, classify};

/// Klipolo's bundle id (`tauri.conf.json`'s `identifier`): an app with it at the target is a Klipolo, replaced unless it is
/// newer.
const BUNDLE_ID: &str = "com.klipolo.app";

/// The nearest ancestor of `current_exe` named `*.app`, when it has `Contents/Info.plist`.
pub(crate) fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe
        .ancestors()
        .skip(1)
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))?;
    app.join("Contents/Info.plist")
        .is_file()
        .then(|| app.to_path_buf())
}

/// `<sys/mount.h>`: the volume is on this computer. A network share (smbfs, nfs, afpfs, webdav) isn't.
const MNT_LOCAL: u32 = 0x0000_1000;
/// `<sys/stat.h>`: the user's immutable flag (Finder's Get Info → Locked, `chflags uchg`) and the system's.
const UF_IMMUTABLE: u32 = 0x0000_0002;
const SF_IMMUTABLE: u32 = 0x0002_0000;

/// `statfs` on the bundle's volume (`MNT_RDONLY`; `f_fstypename` "apfs" swaps atomically; no `MNT_LOCAL` on a network
/// share), `access(W_OK)` on the bundle and its folder, and the bundle's immutable flags (Finder's Locked). Nothing on
/// a Mac is managed. Unknown when the volume can't be read (the bundle is gone).
pub(crate) fn install_place(bundle: &Path) -> InstallPlace {
    let Some(volume) = statfs(bundle) else {
        return InstallPlace::Unknown;
    };
    let facts = PlaceFacts {
        parent_writable: bundle.parent().is_some_and(writable),
        bundle_writable: writable(bundle),
        read_only_volume: volume.f_flags & libc::MNT_RDONLY as u32 != 0,
        managed: false,
        atomic_swap: fs_type(&volume) == b"apfs",
        locked: locked(bundle),
        network: volume.f_flags & MNT_LOCAL == 0,
    };
    classify(bundle, facts)
}

/// Locked in Finder (`UF_IMMUTABLE`), or immutable for the system (`SF_IMMUTABLE`): not even an administrator may
/// rename or replace it until it is unlocked.
fn locked(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.st_flags() & (UF_IMMUTABLE | SF_IMMUTABLE) != 0)
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput.into())
}

fn statfs(path: &Path) -> Option<libc::statfs> {
    let path = c_path(path).ok()?;
    // SAFETY: plain integers and byte arrays; all zeroes is a valid value.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path, and the struct `statfs` fills.
    (unsafe { libc::statfs(path.as_ptr(), &mut st) } == 0).then_some(st)
}

/// The volume's file system name ("apfs", "hfs", "exfat", "msdos", "smbfs"…), up to its NUL.
fn fs_type(st: &libc::statfs) -> Vec<u8> {
    st.f_fstypename
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect()
}

/// `access(W_OK)`: this user may create, rename and delete in it.
fn writable(path: &Path) -> bool {
    // SAFETY: a NUL-terminated path.
    c_path(path).is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::W_OK) } == 0)
}

/// Into /Applications when this user may write there (an administrator), else ~/Applications (made when missing)
/// unless an administrator's Klipolo is already in /Applications (`folder_for`), always as Klipolo.app
/// (`install::moving`). `than` compares a Klipolo already there with this one.
pub(crate) fn move_to_applications(
    bundle: &Path,
    than: &dyn Fn(&str) -> Ordering,
) -> Result<PathBuf, MoveError> {
    let system = Path::new("/Applications");
    let system_writable = writable(system);
    let found = if system_writable {
        None
    } else {
        bundle_info(&system.join(APP_NAME))
    };
    if folder_for(system_writable, found, BUNDLE_ID, than)? == Folder::System {
        return move_into(&Mac, bundle, system, BUNDLE_ID, than);
    }
    let home = dirs::home_dir()
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?
        .join("Applications");
    std::fs::create_dir_all(&home)?;
    move_into(&Mac, bundle, &home, BUNDLE_ID, than)
}

/// The move's macOS steps.
struct Mac;

impl Mover for Mac {
    fn info(&self, app: &Path) -> Option<BundleInfo> {
        bundle_info(app)
    }

    fn free_space(&self, dir: &Path) -> Option<u64> {
        crate::durable::free_space(dir)
    }

    fn quarantined(&self, app: &Path) -> bool {
        quarantined(app)
    }

    /// `ditto` (resource forks, extended attributes and code signatures kept), then `com.apple.quarantine` cleared
    /// on the copy, so it opens without the "downloaded from the Internet" question and is never translocated.
    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        run("/usr/bin/ditto", &[from.as_os_str(), to.as_os_str()])?;
        let args: [&OsStr; 3] = [
            "-dr".as_ref(),
            "com.apple.quarantine".as_ref(),
            to.as_os_str(),
        ];
        run("/usr/bin/xattr", &args)
    }

    fn trash(&self, path: &Path) -> io::Result<Option<PathBuf>> {
        crate::folder::move_to_trash_at(path)
    }

    /// `renamex_np(RENAME_EXCL)` too: never over a folder that appeared at `to` meanwhile, which a
    /// plain rename of a folder would replace when it is empty.
    fn put_back(&self, trashed: &Path, to: &Path) -> io::Result<()> {
        rename_new(trashed, to)
    }

    fn rename_new(&self, from: &Path, to: &Path) -> io::Result<()> {
        rename_new(from, to)
    }
}

/// `renamex_np(RENAME_EXCL)`: never over something that appeared at `to` meanwhile.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (c_path(from)?, c_path(to)?);
    // SAFETY: two NUL-terminated paths.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `path` carries `com.apple.quarantine` (a download's mark, which makes an app run translocated).
fn quarantined(path: &Path) -> bool {
    let Ok(path) = c_path(path) else {
        return false;
    };
    // SAFETY: a NUL-terminated path and name; a null buffer asks only for the size.
    let size = unsafe {
        libc::getxattr(
            path.as_ptr(),
            c"com.apple.quarantine".as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    size >= 0
}

/// Runs a system tool quietly; a failure says which tool and its exit status (never a path).
fn run(tool: &str, args: &[&OsStr]) -> io::Result<()> {
    let status = Command::new(tool)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{tool} failed ({status})")))
    }
}

/// `CFBundleIdentifier` and `CFBundleShortVersionString` from `<app>/Contents/Info.plist` (XML or binary). None
/// when it has no identifier.
fn bundle_info(app: &Path) -> Option<BundleInfo> {
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSString, ns_string};
    let plist = app.join("Contents/Info.plist");
    let plist = NSString::from_str(plist.to_str()?);
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: a path; the dictionary's values are checked by type before use. (Deprecated for its error
        // reporting only: a file that can't be read is simply None.)
        #[allow(deprecated)]
        let info =
            unsafe { NSDictionary::<NSString, AnyObject>::dictionaryWithContentsOfFile(&plist) }?;
        let text = |key: &NSString| {
            let value = info.objectForKey(key)?;
            value.downcast_ref::<NSString>().map(NSString::to_string)
        };
        Some(BundleInfo {
            id: text(ns_string!("CFBundleIdentifier"))?,
            version: text(ns_string!("CFBundleShortVersionString")),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundle named `name` with an Info.plist naming `id` and `version`, and an executable.
    fn app(at: &Path, name: &str, id: &str, version: &str) -> PathBuf {
        let app = at.join(name);
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/klipolo"), b"#!/bin/sh\n").unwrap();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key>\
             <string>{id}</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>\n"
        );
        std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        app
    }

    /// The real steps (`ditto`, `xattr`, the plist), never the Trash: the Trash → rename → Put Back path is
    /// `install::moving`'s tests, over a test Trash.
    #[test]
    fn moving_copies_the_bundle_as_klipolo_app_without_quarantine_and_never_over_a_newer_one() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let source = app(from.path(), "Klipolo 2.app", BUNDLE_ID, "1.5.0");
        let exe = c_path(&source.join("Contents/MacOS/klipolo")).unwrap();
        let mark = b"0081;00000000;Klipolo;";
        // SAFETY: a NUL-terminated path and name, and the value with its length.
        let set = unsafe {
            libc::setxattr(
                exe.as_ptr(),
                c"com.apple.quarantine".as_ptr(),
                mark.as_ptr().cast(),
                mark.len(),
                0,
                0,
            )
        };
        assert_eq!(set, 0, "{}", io::Error::last_os_error());
        let newer = |v: &str| {
            if v == "1.6.0" {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        };

        // Another app with that name stays, and nothing is copied beside it.
        let other = app(to.path(), "Klipolo.app", "com.example.other", "9.0");
        let refused = move_into(&Mac, &source, to.path(), BUNDLE_ID, &newer).unwrap_err();
        assert!(matches!(refused, MoveError::NameTaken), "{refused:?}");
        assert_eq!(std::fs::read_dir(to.path()).unwrap().count(), 1);
        std::fs::remove_dir_all(&other).unwrap();

        // A newer Klipolo stays: its version is read from its Info.plist.
        let installed = app(to.path(), "Klipolo.app", BUNDLE_ID, "1.6.0");
        assert_eq!(
            bundle_info(&installed),
            Some(BundleInfo {
                id: BUNDLE_ID.into(),
                version: Some("1.6.0".into())
            })
        );
        let refused = move_into(&Mac, &source, to.path(), BUNDLE_ID, &newer).unwrap_err();
        assert!(
            matches!(refused, MoveError::NewerInstalled(ref v) if v == "1.6.0"),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_dir(to.path()).unwrap().count(), 1);
        std::fs::remove_dir_all(&installed).unwrap();

        // The bundle is copied as Klipolo.app, its quarantine cleared, the source kept.
        let moved = move_into(&Mac, &source, to.path(), BUNDLE_ID, &newer).unwrap();
        assert_eq!(moved, to.path().join("Klipolo.app"));
        assert_eq!(
            bundle_info(&moved).map(|i| i.id).as_deref(),
            Some(BUNDLE_ID)
        );
        assert!(!quarantined(&moved.join("Contents/MacOS/klipolo")));
        assert!(quarantined(&source.join("Contents/MacOS/klipolo")));
        assert_eq!(std::fs::read_dir(to.path()).unwrap().count(), 1);
        // Moving the copy onto itself changes nothing.
        assert_eq!(
            move_into(&Mac, &moved, to.path(), BUNDLE_ID, &newer).unwrap(),
            moved
        );
    }
}
