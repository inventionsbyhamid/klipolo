//! The edges of the move: what a full disk, a failed Trash, the same version, a
//! Klipolo put at the target during the copy and an administrator's copy each leave.

use std::cmp::Ordering;

use super::*;

/// A disk without room for the copy is said to be full before anything is written, so the person reads "free some
/// space" rather than "drag it in Finder" (which would fail the same way).
#[test]
fn a_disk_without_room_for_the_copy_says_so_before_anything_is_written() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    let older = app(&p.apps, APP_NAME, ID, "1.4.0");
    let full = Folders {
        free: Some(3),
        ..p.mover()
    };
    let e = move_into(&full, &source, &p.apps, ID, &newer).unwrap_err();
    assert!(
        matches!(e, MoveError::Io(ref e) if crate::durable::classify(e) == crate::durable::DiskReason::Full),
        "{e:?}"
    );
    assert_eq!(listing(&p.apps), ["Klipolo.app"], "nothing was copied");
    assert_eq!(version_of(&older), "1.4.0");
    assert!(listing(&p.trash).is_empty());
}

/// The Trash refuses the older Klipolo (an ACL, App Management): the copy goes, and the older one stays.
#[test]
fn a_trash_that_refuses_leaves_the_older_klipolo_and_no_copy() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    let older = app(&p.apps, APP_NAME, ID, "1.4.0");
    let refusing = Folders {
        fail_trash: true,
        ..p.mover()
    };
    let e = move_into(&refusing, &source, &p.apps, ID, &newer).unwrap_err();
    assert!(
        matches!(e, MoveError::Io(ref e) if e.raw_os_error() == Some(13)),
        "{e:?}"
    );
    assert_eq!(listing(&p.apps), ["Klipolo.app"], "no hidden copy is left");
    assert_eq!(version_of(&older), "1.4.0");
}

/// This very version already in Applications (a second move from the disk image, a retry after work started
/// during the copy) is simply opened: nothing is copied, and nothing goes to the Trash.
#[test]
fn the_same_version_already_there_is_simply_opened() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    let there = app(&p.apps, APP_NAME, ID, "1.5.0");
    std::fs::write(there.join("Contents/installed"), b"the person's copy").unwrap();
    let moved = move_into(&p.mover(), &source, &p.apps, ID, &newer).unwrap();
    assert_eq!(moved, there);
    assert!(
        there.join("Contents/installed").exists(),
        "the copy there is kept"
    );
    assert_eq!(listing(&p.apps), ["Klipolo.app"]);
    assert!(listing(&p.trash).is_empty());
}

/// What is at the target is read again before it goes. A newer Klipolo put there during the copy stays, and the
/// copy is deleted; nothing goes to the Trash on the strength of what was there before.
#[test]
fn a_klipolo_put_there_during_the_copy_is_judged_as_it_is_now() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    app(&p.apps, APP_NAME, ID, "1.4.0");
    let raced = Folders {
        swapped_in: Some("1.6.0"),
        ..p.mover()
    };
    match move_into(&raced, &source, &p.apps, ID, &newer) {
        Err(MoveError::NewerInstalled(v)) => assert_eq!(v, "1.6.0"),
        other => panic!("{other:?}"),
    }
    assert_eq!(version_of(&p.apps.join(APP_NAME)), "1.6.0");
    assert_eq!(listing(&p.apps), ["Klipolo.app"], "the copy is deleted");
    assert!(listing(&p.trash).is_empty());
}

/// A standard account (it may not write /Applications) never gets a second Klipolo in ~/Applications beside an
/// administrator's: it is told where that one is. With no Klipolo there (or another app), the copy goes home.
#[test]
fn a_standard_account_never_gets_a_second_klipolo_beside_an_administrators() {
    let klipolo = |version: &str| {
        Some(BundleInfo {
            id: ID.into(),
            version: Some(version.into()),
        })
    };
    assert_eq!(
        folder_for(true, None, ID, &newer).unwrap(),
        Folder::System,
        "an administrator moves into /Applications"
    );
    assert_eq!(folder_for(false, None, ID, &newer).unwrap(), Folder::Home);
    let other = BundleInfo {
        id: "com.example.other".into(),
        version: None,
    };
    assert_eq!(
        folder_for(false, Some(other), ID, &newer).unwrap(),
        Folder::Home
    );
    for version in ["1.4.0", "1.5.0"] {
        assert!(matches!(
            folder_for(false, klipolo(version), ID, &newer),
            Err(MoveError::InApplications)
        ));
    }
    assert!(matches!(
        folder_for(false, klipolo("1.6.0"), ID, &|v: &str| if v == "1.6.0" {
            Ordering::Greater
        } else {
            Ordering::Less
        }),
        Err(MoveError::NewerInstalled(ref v)) if v == "1.6.0"
    ));
}

/// A copy of this very version that still carries a download's quarantine (put there by `cp` or an
/// unarchiver) would run translocated again, so it is replaced by a cleared copy, never just opened.
#[test]
fn a_quarantined_copy_of_the_same_version_is_replaced() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    let there = app(&p.apps, APP_NAME, ID, "1.5.0");
    std::fs::write(there.join("Contents/quarantined"), b"").unwrap();
    let moved = move_into(&p.mover(), &source, &p.apps, ID, &newer).unwrap();
    assert!(
        !moved.join("Contents/quarantined").exists(),
        "a cleared copy"
    );
    assert_eq!(
        listing(&p.trash),
        ["Klipolo.app"],
        "the quarantined one is in the Trash"
    );
}

/// What a killed attempt left is swept before the free space is judged, so it never makes a disk with
/// room look full.
#[test]
fn a_killed_attempts_leftovers_never_count_against_the_space() {
    let p = places();
    let source = app(&p.image, "Klipolo.app", ID, "1.5.0");
    let left = p.apps.join(".Klipolo.app.moving-999999");
    std::fs::create_dir(&left).unwrap();
    std::fs::write(left.join("big"), vec![0u8; 4096]).unwrap();
    let mover = Folders {
        free: Some(4096),
        ..p.mover()
    };
    let moved = move_into(&mover, &source, &p.apps, ID, &newer).unwrap();
    assert_eq!(moved, p.apps.join(APP_NAME));
    assert_eq!(listing(&p.apps), ["Klipolo.app"]);
}
