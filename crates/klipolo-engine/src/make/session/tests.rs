//! The walk that measures what a worker wrote fails closed: what it can't read counts as over the
//! cap. The listing is injected, so this holds whoever runs it (root reads every folder).

use std::io;
use std::path::{Path, PathBuf};

use super::tree::{Entry, Listing, walk};

fn roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/s/tmp"), PathBuf::from("/s/out")]
}

fn listing(entries: Vec<io::Result<Entry>>) -> io::Result<Listing> {
    Ok(Box::new(entries.into_iter()))
}

#[test]
fn a_folder_or_entry_the_walk_cant_read_counts_as_over_the_cap() {
    // tmp/ holds a folder that can't be listed.
    let locked = |dir: &Path| {
        if dir.ends_with("locked") {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if dir.ends_with("tmp") {
            return listing(vec![Ok(Entry {
                path: "/s/tmp/locked".into(),
                folder: true,
                len: 0,
            })]);
        }
        listing(Vec::new())
    };
    assert!(walk(&roots(), 10, locked).unreadable);
    // An entry whose metadata can't be read.
    let entry = |dir: &Path| {
        if dir.ends_with("out") {
            return listing(vec![Err(io::ErrorKind::PermissionDenied.into())]);
        }
        listing(Vec::new())
    };
    assert!(walk(&roots(), 10, entry).unreadable);
    // What vanished meanwhile (the worker deleted it) is no evasion.
    let gone = |dir: &Path| {
        if dir.ends_with("tmp") {
            return Err(io::ErrorKind::NotFound.into());
        }
        listing(vec![Err(io::ErrorKind::NotFound.into())])
    };
    let written = walk(&roots(), 10, gone);
    assert!(!written.unreadable, "{written:?}");
    // Bytes and entries count as before.
    let files = |dir: &Path| {
        if dir.ends_with("tmp") {
            return listing(vec![
                Ok(Entry {
                    path: "/s/tmp/a".into(),
                    folder: false,
                    len: 7,
                }),
                Ok(Entry {
                    path: "/s/tmp/b".into(),
                    folder: false,
                    len: 5,
                }),
            ]);
        }
        listing(Vec::new())
    };
    let written = walk(&roots(), 10, files);
    assert_eq!(
        (written.bytes, written.entries, written.unreadable),
        (12, 2, false)
    );
}

/// A chain of folders deeper than 32 counts as over the limit, however few entries it has; 32 deep
/// is fine.
#[test]
fn a_chain_deeper_than_32_counts_as_over_the_cap() {
    let chain = |depth: usize| {
        move |dir: &Path| {
            let level = dir.components().count() - 3; // "/", "s", "tmp" (or "out")
            if dir.starts_with("/s/tmp") && level < depth {
                return listing(vec![Ok(Entry {
                    path: dir.join("d"),
                    folder: true,
                    len: 0,
                })]);
            }
            listing(Vec::new())
        }
    };
    let deep = walk(&roots(), 10_000, chain(33));
    assert!(deep.too_deep, "{deep:?}");
    let fine = walk(&roots(), 10_000, chain(32));
    assert!(!fine.too_deep && !fine.unreadable, "{fine:?}");
}

/// The removal streams its listings and keeps only folders; a root that is already gone
/// is done, and a tree deeper than the hoist goes whole.
#[test]
fn a_removal_takes_the_whole_tree_and_a_missing_root_is_done() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("session");
    let mut deep = root.join("tmp");
    for i in 0..12 {
        deep = deep.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    for i in 0..50 {
        std::fs::write(deep.join(format!("f{i}")), b"x").unwrap();
        std::fs::write(root.join("tmp").join(format!("g{i}")), b"y").unwrap();
    }
    super::tree::remove_tree(&root, false).unwrap();
    assert!(!root.exists());
    super::tree::remove_tree(&root, true).expect("a root that is already gone is done");
}
