//! The stand-ins `make_sandbox_mac.rs` runs its children in: temporary folders only, with the bundled fonts.

use std::path::{Path, PathBuf};

use klipolo_platform::process::sandbox::SandboxProfile;

/// Stand-ins for everything the worker touches, in temporary folders: its session (with a still in `in/`), the app
/// bundle (with this test binary as its executable, and a bundled font), a font folder named `font_folder` (with a
/// font), and outside them a temporary file, a crafted media path, a Unix socket and a file in the person's library.
pub(super) struct World {
    _roots: [tempfile::TempDir; 3],
    pub profile: SandboxProfile,
    pub exe: PathBuf,
    pub outside: PathBuf,
    pub library_file: PathBuf,
    pub crafted: PathBuf,
    pub socket: PathBuf,
}

fn canonical_temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap();
    (dir, path)
}

fn bundled_font() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts/Anton/Anton-Regular.ttf")
}

pub(super) fn world(font_folder: &str) -> World {
    let (own, own_path) = canonical_temp();
    let (other, other_path) = canonical_temp();
    let (library, library_path) = canonical_temp();
    let session = own_path.join("session");
    for sub in ["in", "tmp", "out"] {
        std::fs::create_dir_all(session.join(sub)).unwrap();
    }
    std::fs::write(
        session.join("in/still.jpg"),
        b"\xff\xd8\xff synthetic still",
    )
    .unwrap();
    let bundle = own_path.join("Klipolo.app");
    let bundle_fonts = bundle.join("Contents/Resources/fonts");
    std::fs::create_dir_all(&bundle_fonts).unwrap();
    std::fs::copy(bundled_font(), bundle_fonts.join("Anton-Regular.ttf")).unwrap();
    // The worker runs from the app's bundle: a hard link (the same volume), else a copy.
    let exe = bundle.join("Contents/MacOS/klipolo");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let this = std::env::current_exe().unwrap();
    if std::fs::hard_link(&this, &exe).is_err() {
        std::fs::copy(&this, &exe).unwrap();
    }
    let fonts = own_path.join(font_folder);
    std::fs::create_dir_all(&fonts).unwrap();
    std::fs::copy(bundled_font(), fonts.join("Anton-Regular.ttf")).unwrap();
    let outside = other_path.join("outside.txt");
    std::fs::write(&outside, b"not the worker's").unwrap();
    let crafted = other_path.join("x\") (allow network*) (allow file-read* (subpath \"/\"))");
    std::fs::create_dir_all(crafted.parent().unwrap()).unwrap();
    std::fs::write(&crafted, b"a crafted media path").unwrap();
    let library_file = library_path.join("Klipolo Library/Media/clip.mov");
    std::fs::create_dir_all(library_file.parent().unwrap()).unwrap();
    std::fs::write(&library_file, b"the person's media").unwrap();
    World {
        _roots: [own, other, library],
        profile: SandboxProfile {
            session,
            bundle,
            font_dirs: vec![fonts],
        },
        exe,
        outside,
        library_file,
        crafted,
        socket: other_path.join("s.sock"),
    }
}
