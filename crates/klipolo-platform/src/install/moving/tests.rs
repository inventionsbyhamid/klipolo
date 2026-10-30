//! The move's steps over plain folders (a test Trash, failures injected): the copy's name, the version check, and the
//! Trash → rename → Put Back path. The edge cases are in `edges`.

mod edges;

use std::path::{Path, PathBuf};

use super::*;

const ID: &str = "com.klipolo.app";

/// An app folder: its id and version in `Contents/Info.txt` (what `Folders` reads), and its version again in the file
/// standing in for the binary, so a copy can be told from the original.
fn app(at: &Path, name: &str, id: &str, version: &str) -> PathBuf {
    let app = at.join(name);
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::write(app.join("Contents/MacOS/klipolo"), version).unwrap();
    std::fs::write(app.join("Contents/Info.txt"), format!("{id}\n{version}")).unwrap();
    app
}

fn version_of(app: &Path) -> String {
    std::fs::read_to_string(app.join("Contents/MacOS/klipolo")).unwrap()
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let at = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &at)?;
        } else {
            std::fs::copy(entry.path(), at)?;
        }
    }
    Ok(())
}

/// Plain folders, a test Trash, and the failures a test asks for.
#[derive(Default)]
struct Folders {
    trash: PathBuf,
    /// What the volume says is free (None: it can't say), less what a killed attempt left in it.
    free: Option<u64>,
    /// The Trash refuses (an ACL, App Management).
    fail_trash: bool,
    /// During the copy, someone puts a Klipolo of this version at the target.
    swapped_in: Option<&'static str>,
    /// The copy stops halfway with a full disk.
    fail_copy: bool,
    /// The rename fails with a disk error.
    fail_rename: bool,
    /// Something appears at the target just before the rename (and so the Put Back can't go there either).
    appears: bool,
}

impl Mover for Folders {
    fn info(&self, app: &Path) -> Option<BundleInfo> {
        let text = std::fs::read_to_string(app.join("Contents/Info.txt")).ok()?;
        let mut lines = text.lines();
        Some(BundleInfo {
            id: lines.next()?.to_owned(),
            version: lines.next().map(str::to_owned),
        })
    }

    fn free_space(&self, dir: &Path) -> Option<u64> {
        let left: u64 = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".Klipolo.app.moving-")
            })
            .map(|e| super::size_of(&e.path()).unwrap_or(0))
            .sum();
        self.free.map(|free| free.saturating_sub(left))
    }

    fn quarantined(&self, app: &Path) -> bool {
        app.join("Contents/quarantined").exists()
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        copy_dir(from, to)?;
        if let Some(version) = self.swapped_in {
            let target = to.parent().unwrap().join(APP_NAME);
            let _ = std::fs::remove_dir_all(&target);
            app(to.parent().unwrap(), APP_NAME, ID, version);
        }
        if self.fail_copy {
            return Err(io::Error::from_raw_os_error(28)); // ENOSPC, the copy left half made
        }
        Ok(())
    }

    fn trash(&self, path: &Path) -> io::Result<Option<PathBuf>> {
        if self.fail_trash {
            return Err(io::Error::from_raw_os_error(13)); // EACCES
        }
        let went = self.trash.join(path.file_name().unwrap());
        std::fs::rename(path, &went)?;
        Ok(Some(went))
    }

    fn put_back(&self, trashed: &Path, to: &Path) -> io::Result<()> {
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        std::fs::rename(trashed, to)
    }

    fn rename_new(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.appears {
            std::fs::create_dir(to)?;
        }
        if self.fail_rename {
            return Err(io::Error::from_raw_os_error(5)); // EIO
        }
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        std::fs::rename(from, to)
    }
}

/// A disk image's folder, an Applications folder and a Trash.
struct Places {
    _dir: tempfile::TempDir,
    image: PathBuf,
    apps: PathBuf,
    trash: PathBuf,
}

fn places() -> Places {
    let dir = tempfile::tempdir().unwrap();
    let [image, apps, trash] =
        ["Volumes/Klipolo", "Applications", ".Trash"].map(|p| dir.path().join(p));
    for d in [&image, &apps, &trash] {
        std::fs::create_dir_all(d).unwrap();
    }
    Places {
        _dir: dir,
        image,
        apps,
        trash,
    }
}

impl Places {
    fn mover(&self) -> Folders {
        Folders {
            trash: self.trash.clone(),
            ..Folders::default()
        }
    }
}

/// How a version found compares with the 1.5.0 being moved: 1.6.0 is newer, 1.5.0 the same, anything else older.
fn newer(v: &str) -> std::cmp::Ordering {
    match v {
        "1.6.0" => std::cmp::Ordering::Greater,
        "1.5.0" => std::cmp::Ordering::Equal,
        _ => std::cmp::Ordering::Less,
    }
}

#[test]
fn the_copy_is_named_klipolo_app_and_an_older_klipolo_goes_to_the_trash() {
    let p = places();
    let source = app(&p.image, "Klipolo 2.app", ID, "1.5.0");
    app(&p.apps, APP_NAME, ID, "1.4.0");
    // A copy an attempt left when it was killed halfway is swept.
    std::fs::create_dir(p.apps.join(".Klipolo.app.moving-999999")).unwrap();
    let m = p.mover();
    let moved = move_into(&m, &source, &p.apps, ID, &newer).unwrap();
    assert_eq!(moved, p.apps.join("Klipolo.app"));
    assert_eq!(version_of(&moved), "1.5.0");
    assert_eq!(listing(&p.apps), ["Klipolo.app"]);
    assert_eq!(
        version_of(&p.trash.join("Klipolo.app")),
        "1.4.0",
        "the older Klipolo can be put back"
    );
    assert_eq!(
        version_of(&source),
        "1.5.0",
        "the disk image's copy is only read"
    );
    // Moving the copy onto itself changes nothing.
    assert_eq!(move_into(&m, &moved, &p.apps, ID, &newer).unwrap(), moved);
    assert_eq!(listing(&p.trash), ["Klipolo.app"]);
}

#[test]
fn an_older_disk_image_never_replaces_a_newer_klipolo_nor_another_app_and_copies_nothing() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    let installed = app(&p.apps, APP_NAME, ID, "1.6.0");
    let m = p.mover();
    match move_into(&m, &source, &p.apps, ID, &newer) {
        Err(MoveError::NewerInstalled(v)) => assert_eq!(v, "1.6.0"),
        other => panic!("{other:?}"),
    }
    assert_eq!(listing(&p.apps), ["Klipolo.app"]);
    assert_eq!(version_of(&installed), "1.6.0");
    assert!(listing(&p.trash).is_empty());
    // Another app with the name stays too.
    std::fs::remove_dir_all(&installed).unwrap();
    app(&p.apps, APP_NAME, "com.example.other", "9.0");
    assert!(matches!(
        move_into(&m, &source, &p.apps, ID, &newer),
        Err(MoveError::NameTaken)
    ));
    assert_eq!(listing(&p.apps), ["Klipolo.app"]);
    assert!(listing(&p.trash).is_empty());
}

#[test]
fn a_failed_copy_or_rename_leaves_the_older_klipolo_where_it_was() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    let older = app(&p.apps, APP_NAME, ID, "1.4.0");
    let full = Folders {
        fail_copy: true,
        ..p.mover()
    };
    let e = move_into(&full, &source, &p.apps, ID, &newer).unwrap_err();
    assert!(
        matches!(e, MoveError::Io(ref e) if e.raw_os_error() == Some(28)),
        "{e:?}"
    );
    assert_eq!(
        listing(&p.apps),
        ["Klipolo.app"],
        "the half-made copy is gone"
    );
    assert_eq!(version_of(&older), "1.4.0");
    // The rename fails after the older Klipolo went to the Trash: it is put back.
    let broken = Folders {
        fail_rename: true,
        ..p.mover()
    };
    let e = move_into(&broken, &source, &p.apps, ID, &newer).unwrap_err();
    assert!(matches!(e, MoveError::Io(_)), "{e:?}");
    assert_eq!(listing(&p.apps), ["Klipolo.app"]);
    assert_eq!(version_of(&older), "1.4.0", "put back from the Trash");
    assert!(listing(&p.trash).is_empty());
}

#[test]
fn a_put_back_that_fails_leaves_the_older_klipolo_in_the_trash_and_says_so() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    app(&p.apps, APP_NAME, ID, "1.4.0");
    let raced = Folders {
        appears: true,
        ..p.mover()
    };
    let e = move_into(&raced, &source, &p.apps, ID, &newer).unwrap_err();
    assert!(matches!(e, MoveError::LeftInTrash(_)), "{e:?}");
    assert_eq!(
        version_of(&p.trash.join("Klipolo.app")),
        "1.4.0",
        "Put Back still works from the Trash"
    );
    assert_eq!(
        listing(&p.apps),
        ["Klipolo.app"],
        "what appeared stays; no copy is left behind"
    );
    assert!(
        std::fs::read_dir(p.apps.join("Klipolo.app"))
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(version_of(&source), "1.5.0");
}
