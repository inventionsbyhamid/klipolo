use std::path::Path;

use klipolo_platform::install::{InstallPlace, PlaceFacts, classify};

const WRITABLE: PlaceFacts = PlaceFacts {
    parent_writable: true,
    bundle_writable: true,
    read_only_volume: false,
    managed: false,
    atomic_swap: true,
    locked: false,
    network: false,
};

#[test]
fn a_writable_copy_on_a_volume_without_atomic_swap_installs_only_on_restart() {
    let facts = PlaceFacts {
        atomic_swap: false,
        ..WRITABLE
    };
    assert_eq!(
        classify(Path::new("/Volumes/External/Klipolo.app"), facts),
        InstallPlace::NonAtomic
    );
}

#[test]
fn a_translocated_copy_never_updates_even_when_writable() {
    let p = Path::new("/private/var/folders/x1/abc/T/AppTranslocation/6F1E2D3C/d/Klipolo.app");
    assert_eq!(classify(p, WRITABLE), InstallPlace::Translocated);
}

#[test]
fn the_mounted_disk_image_is_its_own_place() {
    let facts = PlaceFacts {
        read_only_volume: true,
        parent_writable: false,
        bundle_writable: false,
        ..WRITABLE
    };
    assert_eq!(
        classify(Path::new("/Volumes/Klipolo/Klipolo.app"), facts),
        InstallPlace::DiskImage
    );
}

#[test]
fn applications_owned_by_another_admin_needs_a_password() {
    let app = Path::new("/Applications/Klipolo.app");
    assert_eq!(
        classify(
            app,
            PlaceFacts {
                parent_writable: false,
                ..WRITABLE
            }
        ),
        InstallPlace::NeedsAdmin
    );
    assert_eq!(
        classify(
            app,
            PlaceFacts {
                bundle_writable: false,
                ..WRITABLE
            }
        ),
        InstallPlace::NeedsAdmin
    );
}

/// A bundle locked in Finder can't be replaced, not even by the password prompt's administrator, so it
/// is its own place (Settings says to unlock it), never "needs your password".
#[test]
fn a_bundle_locked_in_finder_is_its_own_place() {
    let app = Path::new("/Applications/Klipolo.app");
    let locked = PlaceFacts {
        locked: true,
        bundle_writable: false,
        ..WRITABLE
    };
    assert_eq!(classify(app, locked), InstallPlace::Locked);
    // A locked copy in a folder only an administrator may change is still locked first.
    assert_eq!(
        classify(
            app,
            PlaceFacts {
                parent_writable: false,
                ..locked
            }
        ),
        InstallPlace::Locked
    );
    // The disk image and a translocated copy keep their own words (the move is offered there).
    let image = PlaceFacts {
        read_only_volume: true,
        ..locked
    };
    assert_eq!(
        classify(Path::new("/Volumes/Klipolo/Klipolo.app"), image),
        InstallPlace::DiskImage
    );
}

/// On a network volume this user can't write, the administrator can't either, so Klipolo never asks for a password
/// there. A network copy this user can write installs on Restart now, as any volume without an atomic swap does.
#[test]
fn a_network_copy_this_user_cant_write_is_never_tried_with_a_password() {
    let app = Path::new("/Volumes/Share/Apps/Klipolo.app");
    let network = PlaceFacts {
        network: true,
        atomic_swap: false,
        ..WRITABLE
    };
    assert_eq!(
        classify(
            app,
            PlaceFacts {
                parent_writable: false,
                ..network
            }
        ),
        InstallPlace::Network
    );
    assert_eq!(classify(app, network), InstallPlace::NonAtomic);
}

#[test]
fn a_package_managers_copy_is_managed_wherever_it_is() {
    let facts = PlaceFacts {
        managed: true,
        ..WRITABLE
    };
    assert_eq!(
        classify(Path::new("/app/bin/klipolo"), facts),
        InstallPlace::Managed
    );
}

#[test]
fn a_writable_copy_updates_in_place() {
    assert_eq!(
        classify(
            Path::new("/Users/someone/Applications/Klipolo.app"),
            WRITABLE
        ),
        InstallPlace::Writable
    );
}

#[cfg(not(windows))]
#[test]
fn a_bundle_in_a_writable_temporary_folder_is_read_as_writable() {
    // The backend's own reading (statfs or statvfs, access): a temporary folder is on a writable volume (APFS on a
    // Mac), so nothing holds an install back.
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("Klipolo.app");
    std::fs::create_dir(&app).unwrap();
    assert_eq!(
        klipolo_platform::install::place(&app),
        InstallPlace::Writable
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn where_the_os_has_no_signal_nothing_pauses() {
    let p = klipolo_platform::network::path();
    assert!(p.online && !p.constrained && !p.expensive);
    assert!(!klipolo_platform::power::low_power());
}

#[test]
fn running_from_never_lists_this_process() {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap();
    assert!(
        !klipolo_platform::process::running_from(dir)
            .unwrap()
            .iter()
            .any(|r| r.pid == std::process::id())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn running_from_finds_a_child_started_from_the_folder() {
    // A copy of `sleep` in a temp "bundle": the child is found by its executable's path, with no mode ("5").
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("worker");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    let mut child = std::process::Command::new(&exe).arg("5").spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let running = klipolo_platform::process::running_from(dir.path()).unwrap();
    assert!(
        running
            .iter()
            .any(|r| r.pid == child.id() && r.mode.is_none()),
        "{running:?}"
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn running_from_reads_a_childs_mode_argument() {
    // A copy of bash in a temp "bundle", started as `worker --norc` and left reading its stdin: found with its mode,
    // read from the OS's argument list (macOS KERN_PROCARGS2, Linux /proc/<pid>/cmdline), as `--mcp` would be.
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("worker");
    std::fs::copy("/bin/bash", &exe).unwrap();
    let mut child = std::process::Command::new(&exe)
        .arg("--norc")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Until it has exec'd, the child still runs the test's own executable: wait for it to appear (up to 5 s).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let found = loop {
        let running = klipolo_platform::process::running_from(dir.path()).unwrap();
        if running.iter().any(|r| r.pid == child.id()) || std::time::Instant::now() > deadline {
            break running;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        found
            .iter()
            .any(|r| r.pid == child.id() && r.mode.as_deref() == Some("--norc")),
        "{found:?}"
    );
}

#[test]
fn a_mode_is_argv1_only_when_it_starts_with_two_dashes() {
    use klipolo_platform::process::mode_of;
    assert_eq!(
        mode_of(&["/A/Klipolo.app/Contents/MacOS/klipolo", "--mcp"]),
        Some("--mcp".into())
    );
    assert_eq!(mode_of(&["/A/Klipolo.app/Contents/MacOS/klipolo"]), None);
    assert_eq!(mode_of(&["/A/llama-server", "-m", "x.gguf"]), None);
}
