//! What the person marked in the library, for every project: the items they starred (favorites) and when they last
//! added each to a timeline from the library (recently used). Kept in `library.json` in the app's config folder,
//! `{ "<key>": { "favorite": true, "usedMs": 1761955200000 } }`, each item by what names it in every project:
//! `pack:<item id>` for Klipolo's pack, `file:<path>` for a person's own file. A file that is missing or not marks as
//! Klipolo writes them reads as no marks, and the next change starts it over; one that can't be read, or is larger than
//! Klipolo writes, is shown as no marks but never written over: a change says why it can't be made. A favorite stays
//! until its star is taken off, even when its file has moved.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use klipolo_platform::durable::{Durability, write_atomic};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The file's name in the config folder.
pub const FILE: &str = "library.json";
/// The most items kept: past it, the items used longest ago that aren't favorites are let go.
pub const MOST_MARKS: usize = 5_000;

/// How one item is marked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Mark {
    /// Starred: it lists first on its tab.
    #[serde(default)]
    pub favorite: bool,
    /// When the library last added it to a timeline (ms since the Unix epoch); 0 for never.
    #[serde(default)]
    pub used_ms: u64,
}

/// Every marked item, by key.
pub type Marks = BTreeMap<String, Mark>;

/// The longest key: a file's path at its longest on a Mac (1,024 bytes), and its prefix.
const MOST_KEY: usize = 1_100;
/// The most of the file read: `MOST_MARKS` marks with keys of `MOST_KEY` fit in it, so all Klipolo writes is read.
const MOST_BYTES: u64 = 8 * 1024 * 1024;

/// The marks saved in `dir`; none when there are none Klipolo can read.
pub fn load(dir: &Path) -> Marks {
    read(dir).unwrap_or_else(|error| {
        tracing::warn!(%error, "the library's marks couldn't be read");
        Marks::new()
    })
}

/// The marks saved in `dir`: none when there is no file, or when it isn't marks as Klipolo writes them (a change then
/// starts it over). An error when the file can't be read or is larger than Klipolo writes, so no change writes over it.
fn read(dir: &Path) -> io::Result<Marks> {
    let file = match std::fs::File::open(dir.join(FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Marks::new()),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(file, MOST_BYTES + 1), &mut bytes)?;
    if bytes.len() as u64 > MOST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the library's marks are larger than Klipolo writes",
        ));
    }
    Ok(serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(%error, "the library's marks don't read as marks; they start over");
        Marks::new()
    }))
}

/// Stars item `key` (`on`) or takes its star off, saved in `dir`; the marks as they are then.
pub fn set_favorite(dir: &Path, key: &str, on: bool) -> io::Result<Marks> {
    change(dir, key, |mark| mark.favorite = on)
}

/// Notes that the library added item `key` to a timeline at `now_ms`, saved in `dir`; the marks as they are then.
pub fn mark_used(dir: &Path, key: &str, now_ms: u64) -> io::Result<Marks> {
    change(dir, key, |mark| mark.used_ms = now_ms)
}

/// Whether `key` names a pack item (`pack:<id>`) or a file (`file:<path>`), within `MOST_KEY`.
fn is_key(key: &str) -> bool {
    key.len() <= MOST_KEY
        && ["pack:", "file:"]
            .iter()
            .any(|kind| key.strip_prefix(kind).is_some_and(|rest| !rest.is_empty()))
}

/// Item `key`'s mark changed by `f` and saved in `dir` (made when needed), whole or not at all; nothing when the marks
/// there can't be read (`read`). A mark left with no star and no use goes; past `MOST_MARKS`, the oldest uses that
/// aren't favorites go too.
fn change(dir: &Path, key: &str, f: impl FnOnce(&mut Mark)) -> io::Result<Marks> {
    if !is_key(key) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only a pack item or a file can be marked",
        ));
    }
    let mut marks = read(dir)?;
    let mark = marks.entry(key.to_owned()).or_default();
    f(mark);
    if *mark == Mark::default() {
        marks.remove(key);
    }
    let over = marks.len().saturating_sub(MOST_MARKS);
    if over > 0 {
        let mut oldest: Vec<(u64, String)> = (marks.iter())
            .filter(|(_, mark)| !mark.favorite)
            .map(|(key, mark)| (mark.used_ms, key.clone()))
            .collect();
        oldest.sort_unstable();
        for (_, key) in oldest.into_iter().take(over) {
            marks.remove(&key);
        }
    }
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec(&marks).map_err(io::Error::other)?;
    write_atomic(&dir.join(FILE), &json, Durability::Ordered)?;
    Ok(marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(favorite: bool, used_ms: u64) -> Mark {
        Mark { favorite, used_ms }
    }

    /// A star and a use are saved, and read back as they were: by another project, or after the app starts again.
    #[test]
    fn favorites_and_uses_are_kept_for_every_project() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_empty());
        set_favorite(dir.path(), "pack:whoosh", true).unwrap();
        let marks = mark_used(dir.path(), "file:/Movies/lofi.mp3", 1_000).unwrap();
        assert_eq!(
            marks,
            Marks::from([
                ("file:/Movies/lofi.mp3".into(), on(false, 1_000)),
                ("pack:whoosh".into(), on(true, 0)),
            ])
        );
        assert_eq!(load(dir.path()), marks);
        // Used again later: the newer time. Starred too: both.
        mark_used(dir.path(), "pack:whoosh", 2_000).unwrap();
        mark_used(dir.path(), "file:/Movies/lofi.mp3", 3_000).unwrap();
        let marks = load(dir.path());
        assert_eq!(marks["pack:whoosh"], on(true, 2_000));
        assert_eq!(marks["file:/Movies/lofi.mp3"], on(false, 3_000));
    }

    /// Taking a star off keeps when the item was used; an item neither starred nor used isn't kept.
    #[test]
    fn an_unstarred_item_keeps_its_use_or_goes() {
        let dir = tempfile::tempdir().unwrap();
        set_favorite(dir.path(), "pack:pop", true).unwrap();
        set_favorite(dir.path(), "pack:ding", true).unwrap();
        mark_used(dir.path(), "pack:ding", 5_000).unwrap();
        set_favorite(dir.path(), "pack:pop", false).unwrap();
        let marks = set_favorite(dir.path(), "pack:ding", false).unwrap();
        assert_eq!(marks, Marks::from([("pack:ding".into(), on(false, 5_000))]));
        assert_eq!(load(dir.path()), marks);
    }

    /// Only a pack item or a file is marked, by a key of sensible length; anything else is refused and nothing is
    /// written.
    #[test]
    fn only_pack_items_and_files_are_marked() {
        let dir = tempfile::tempdir().unwrap();
        let long = format!("file:/{}", "a".repeat(5_000));
        for key in ["", "whoosh", "pack:", "file:", "project:abc", long.as_str()] {
            let error = set_favorite(dir.path(), key, true).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{key:?}");
            assert!(mark_used(dir.path(), key, 1).is_err(), "{key:?}");
        }
        assert!(!dir.path().join(FILE).exists());
    }

    /// A file that isn't marks as Klipolo writes them (damaged, a list, too big) reads as none, and the next mark
    /// starts it over.
    #[test]
    fn a_file_that_isnt_marks_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        for body in [
            b"not json".to_vec(),
            b"[1, 2]".to_vec(),
            br#"{"pack:pop": 3}"#.to_vec(),
        ] {
            std::fs::write(dir.path().join(FILE), body).unwrap();
            assert!(load(dir.path()).is_empty());
        }
        // A field Klipolo doesn't know yet is no reason to lose the rest.
        std::fs::write(
            dir.path().join(FILE),
            br#"{"pack:pop": {"favorite": true, "colour": "red"}}"#,
        )
        .unwrap();
        assert_eq!(load(dir.path())["pack:pop"], on(true, 0));
        std::fs::write(dir.path().join(FILE), b"not json").unwrap();
        let marks = set_favorite(dir.path(), "pack:pop", true).unwrap();
        assert_eq!(marks, Marks::from([("pack:pop".into(), on(true, 0))]));
    }

    /// A file Klipolo can't read, or one larger than it writes, is left as it is: a change says so and writes nothing,
    /// so no star in it is lost.
    #[cfg(unix)]
    #[test]
    fn a_file_that_cant_be_read_is_never_written_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        // Larger than Klipolo writes: a star, then more than `MOST_BYTES` of spaces before the end.
        let mut big = br#"{"pack:pop": {"favorite": true}"#.to_vec();
        big.resize(big.len() + MOST_BYTES as usize, b' ');
        big.push(b'}');
        std::fs::write(&path, &big).unwrap();
        assert!(set_favorite(dir.path(), "pack:ding", true).is_err());
        assert!(mark_used(dir.path(), "pack:ding", 1).is_err());
        assert!(std::fs::read(&path).unwrap() == big);
        // Not allowed to read it (as anyone but root).
        std::fs::write(&path, br#"{"pack:pop": {"favorite": true}}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_err() {
            assert!(set_favorite(dir.path(), "pack:ding", true).is_err());
            assert!(load(dir.path()).is_empty());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(load(dir.path())["pack:pop"], on(true, 0));
        }
    }

    /// Past `MOST_MARKS`, the items used longest ago that aren't favorites are let go; favorites stay.
    #[test]
    fn the_oldest_uses_go_first_when_there_are_too_many() {
        let dir = tempfile::tempdir().unwrap();
        let mut marks: Marks = (0..MOST_MARKS as u64)
            .map(|i| (format!("pack:item-{i}"), on(i == 0, i + 1)))
            .collect();
        std::fs::write(dir.path().join(FILE), serde_json::to_vec(&marks).unwrap()).unwrap();
        let after = mark_used(dir.path(), "pack:new", 1_000_000).unwrap();
        assert_eq!(after.len(), MOST_MARKS);
        // item-0 is the oldest, but starred; item-1 is the oldest of the rest.
        assert!(after.contains_key("pack:item-0") && after.contains_key("pack:new"));
        assert!(!after.contains_key("pack:item-1"));
        marks.remove("pack:item-1");
        marks.insert("pack:new".into(), on(false, 1_000_000));
        assert_eq!(after, marks);
    }
}
