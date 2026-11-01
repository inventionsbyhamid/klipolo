//! The start screen's list, over a library in a temporary folder: every time is set by hand (a fixed clock), as
//! `recent.json` stamps or as a file's last save.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use klipolo_engine::media::key::{CacheKind, stem};
use klipolo_engine::media::sprite::Sprite;
use klipolo_engine::media::thumbs::{self, Fill};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::recording::{
    Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_platform::durable::{Durability, Os};

use super::*;
use crate::project::{ProjectSessionState, create_in, lock, new_project, open_in, save};

/// The library, the stamps file and the thumbnails' folder in `dir`, as the app has them: the library made, the
/// others not yet.
fn folders(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let library = dir.join("Klipolo");
    fs::create_dir_all(&library).unwrap();
    (
        library,
        dir.join("data").join("recent.json"),
        dir.join("cache").join("media").join("thumbs"),
    )
}

/// `project` saved as `<name>.klipolo` in `folder`, last saved at `saved_ms`.
fn saved(folder: &Path, project: &Project, saved_ms: u64) -> PathBuf {
    let file = folder.join(format!("{}.klipolo", project.name));
    save(&file, project, Durability::Ordered).unwrap();
    set_saved(&file, saved_ms);
    file
}

fn set_saved(file: &Path, ms: u64) {
    File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
        .unwrap();
}

/// `recent.json` as the app writes it: `{ path: opened_ms }`.
fn stamped(stamps: &Path, opened: &[(&Path, u64)]) {
    let map: BTreeMap<String, u64> = opened
        .iter()
        .map(|(file, ms)| (file.to_string_lossy().into_owned(), *ms))
        .collect();
    fs::create_dir_all(stamps.parent().unwrap()).unwrap();
    fs::write(stamps, serde_json::to_vec(&map).unwrap()).unwrap();
}

fn names(rows: &[ProjectRow]) -> Vec<(&str, u64)> {
    rows.iter()
        .map(|row| (row.name.as_str(), row.opened_ms))
        .collect()
}

/// A stand-in media file (its bytes are never read: a thumbnail's cache is keyed by the file's path, size and time).
fn media_file(dir: &Path, name: &str) -> PathBuf {
    let file = dir.join("media").join(name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, format!("not really {name}")).unwrap();
    file
}

fn asset(id: &str, kind: AssetKind, file: &Path) -> Asset {
    let video = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: file.to_string_lossy().into_owned(),
        name: file.file_name().unwrap().to_string_lossy().into_owned(),
        duration_us: 10_000_000,
        width: if video { 1280 } else { 0 },
        height: if video { 720 } else { 0 },
        fps: if video { 30.0 } else { 0.0 },
        rotation: None,
        has_audio: !video,
        audio_lead_us: None,
        made_with: None,
    }
}

fn clip(source: ClipSource, from_us: i64, to_us: i64) -> Clip {
    Clip::new(source, from_us, to_us)
}

fn of_asset(id: &str) -> ClipSource {
    ClipSource::Asset { id: id.into() }
}

/// Chunk `chunk` of `file`'s thumbnails, written as a scan writes it: its sprite's path.
fn cache_chunk(thumbs_root: &Path, file: &Path, chunk: u32, tile: (u32, u32)) -> String {
    let stem = stem(file.to_str().unwrap(), &CacheKind::Thumbs).unwrap();
    let sprite = Sprite::new(chunk, tile);
    thumbs::write_chunk(&Os, thumbs_root, &stem, &sprite, Fill::Keys).unwrap();
    thumbs::chunk_path(thumbs_root, &stem, chunk, Fill::Keys)
        .to_string_lossy()
        .into_owned()
}

/// Projects opened here come first, the latest open first, even before a file saved after them; the rest follow. A
/// stamp for a file that isn't in the library lists nothing.
#[test]
fn opened_projects_come_first_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let older = saved(&library, &Project::new("Opened before"), 1_000);
    let newer = saved(&library, &Project::new("Opened last"), 1_000);
    saved(&library, &Project::new("Saved later"), 9_000);
    let gone = dir.path().join("Elsewhere").join("Gone.klipolo");
    stamped(&stamps, &[(&older, 2_000), (&newer, 3_000), (&gone, 4_000)]);

    let rows = recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap();
    assert_eq!(
        names(&rows),
        [
            ("Opened last", 3_000),
            ("Opened before", 2_000),
            ("Saved later", 9_000)
        ]
    );
    assert_eq!(rows[0].path, newer.to_string_lossy());
}

/// With nothing opened yet (no `recent.json`), the library's project files list by their last save, newest first.
/// Other files, a save's temporary file, folders and projects in sub folders aren't listed.
#[test]
fn never_opened_files_follow_by_last_save() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    saved(&library, &Project::new("First"), 1_000);
    saved(&library, &Project::new("Third"), 3_000);
    saved(&library, &Project::new("Second"), 2_000);
    fs::write(library.join("notes.txt"), "a note").unwrap();
    fs::write(library.join("Second.klipolo.4242.tmp"), "half a save").unwrap();
    fs::create_dir_all(library.join("A folder.klipolo")).unwrap();
    saved(&library.join("Recordings"), &Project::new("Nested"), 4_000);

    let rows = recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap();
    assert_eq!(
        names(&rows),
        [("Third", 3_000), ("Second", 2_000), ("First", 1_000)]
    );
}

/// A file that doesn't read as a project is left out, and the next readable one takes its place; a `recent.json` that
/// doesn't read counts as no stamps.
#[test]
fn unreadable_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    saved(&library, &Project::new("Good"), 1_000);
    let text = library.join("Not a project.klipolo");
    fs::write(&text, "not a project").unwrap();
    set_saved(&text, 2_000);
    let bytes = library.join("Not text.klipolo");
    fs::write(&bytes, [0xff, 0xfe, 0x00]).unwrap();
    set_saved(&bytes, 3_000);
    fs::create_dir_all(stamps.parent().unwrap()).unwrap();
    fs::write(&stamps, "{ not json").unwrap();

    assert_eq!(
        names(&recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap()),
        [("Good", 1_000)]
    );
    assert_eq!(
        names(&recent_in(&library, &stamps, None, 1).unwrap()),
        [("Good", 1_000)],
        "skipped files don't use up the limit"
    );
}

/// The start screen lists the 30 latest projects.
#[test]
fn at_most_30_rows() {
    assert_eq!(RECENT_LIMIT, 30);
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    for n in 1..=35 {
        saved(
            &library,
            &Project::new(format!("Project {n:02}")),
            n * 1_000,
        );
    }

    let rows = recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap();
    assert_eq!(rows.len(), 30);
    assert_eq!(rows.first().unwrap().name, "Project 35");
    assert_eq!(rows.last().unwrap().name, "Project 06");
}

/// A card's picture is the first tile of its first main clip's cached thumbnails (chunk 0), never another clip's and
/// never a later chunk; None when there's no cache, no clip, or that chunk isn't scanned yet. Its length is the main
/// track's.
#[test]
fn thumb_is_the_first_clips_cached_tile_or_none() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, root) = folders(dir.path());
    let first = media_file(dir.path(), "first.mp4");
    let second = media_file(dir.path(), "second.mp4");
    let mut project = Project::new("Two clips");
    // The assets in another order than the clips: the first clip counts, not the first file.
    project.assets = vec![
        asset("second", AssetKind::Video, &second),
        asset("first", AssetKind::Video, &first),
    ];
    project.timeline.main = vec![
        clip(of_asset("first"), 0, 2_000_000),
        clip(of_asset("second"), 1_000_000, 4_000_000),
    ];
    saved(&library, &project, 2_000);
    saved(&library, &Project::new("No clips"), 1_000);
    let row = |thumbs: Option<&Path>| recent_in(&library, &stamps, thumbs, RECENT_LIMIT).unwrap();

    let rows = row(None);
    assert_eq!(
        rows.iter()
            .map(|r| (r.name.as_str(), r.duration_us, r.clips))
            .collect::<Vec<_>>(),
        [("Two clips", 5_000_000, 2), ("No clips", 0, 0)]
    );
    assert_eq!(rows[0].thumb, None, "no cache folder");
    assert_eq!(row(Some(&root))[0].thumb, None, "nothing scanned yet");

    cache_chunk(&root, &second, 0, (20, 10));
    cache_chunk(&root, &first, 1, (16, 10));
    assert_eq!(
        row(Some(&root))[0].thumb,
        None,
        "another clip's tile, or a later chunk, isn't the card's"
    );

    let sprite = cache_chunk(&root, &first, 0, (16, 10));
    let rows = row(Some(&root));
    assert_eq!(
        rows[0].thumb,
        Some(ThumbTile {
            sprite,
            tile_w: 16,
            tile_h: 10,
            cols: 8,
        })
    );
    assert_eq!(rows[1].thumb, None, "no clip, no picture");
}

/// Once a scan has written the first clip's poster (its first picture, bigger than a tile), the card shows that: the
/// whole picture, one to its sheet. A cache from before posters keeps showing the first tile.
#[test]
fn thumb_is_the_first_clips_poster_once_there_is_one() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, root) = folders(dir.path());
    let first = media_file(dir.path(), "first.mp4");
    let mut project = Project::new("One clip");
    project.assets = vec![asset("first", AssetKind::Video, &first)];
    project.timeline.main = vec![clip(of_asset("first"), 0, 2_000_000)];
    saved(&library, &project, 2_000);
    let sprite = cache_chunk(&root, &first, 0, (16, 10));
    let thumb = || {
        recent_in(&library, &stamps, Some(&root), RECENT_LIMIT).unwrap()[0]
            .thumb
            .clone()
    };
    assert_eq!(
        thumb(),
        Some(ThumbTile {
            sprite,
            tile_w: 16,
            tile_h: 10,
            cols: 8,
        }),
        "no poster yet: the first tile"
    );

    let stem = stem(first.to_str().unwrap(), &CacheKind::Thumbs).unwrap();
    let (luma, chroma) = (vec![128; 32 * 18], vec![128; 16 * 9]);
    thumbs::write_poster(
        &Os,
        &root,
        &stem,
        (32, 18),
        [&luma, &chroma, &chroma],
        [32, 16, 16],
    )
    .unwrap();
    assert_eq!(
        thumb(),
        Some(ThumbTile {
            sprite: thumbs::poster_path(&root, &stem, (32, 18))
                .to_string_lossy()
                .into_owned(),
            tile_w: 32,
            tile_h: 18,
            cols: 1,
        })
    );
}

/// A recording's card shows its main footage, the screen: not the camera, not the first source listed.
#[test]
fn a_recording_project_uses_its_screen_file_for_the_thumb() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, root) = folders(dir.path());
    let screen = media_file(dir.path(), "screen.mp4");
    let camera = media_file(dir.path(), "camera.mp4");
    let voice = media_file(dir.path(), "voice.m4a");
    let source = |id: &str, role: SourceRole, asset_id: &str| RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs: vec![SourceRun {
            asset_id: asset_id.into(),
            offset_us: 0,
            duration_us: 10_000_000,
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    };
    let mut project = Project::new("A take");
    project.assets = vec![
        asset("voice", AssetKind::Audio, &voice),
        asset("camera", AssetKind::Video, &camera),
        asset("screen", AssetKind::Video, &screen),
    ];
    project.recordings = vec![Recording {
        id: "take".into(),
        kind: RecordingKind::Screen,
        sources: vec![
            source("mic", SourceRole::Mic, "voice"),
            source("camera", SourceRole::Camera, "camera"),
            source("screen", SourceRole::Screen, "screen"),
        ],
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }];
    project.timeline.main = vec![clip(
        ClipSource::Recording { id: "take".into() },
        0,
        6_000_000,
    )];
    saved(&library, &project, 1_000);
    cache_chunk(&root, &camera, 0, (20, 10));
    let sprite = cache_chunk(&root, &screen, 0, (16, 10));

    let rows = recent_in(&library, &stamps, Some(&root), RECENT_LIMIT).unwrap();
    assert_eq!((rows[0].duration_us, rows[0].clips), (6_000_000, 1));
    assert_eq!(
        rows[0].thumb,
        Some(ThumbTile {
            sprite,
            tile_w: 16,
            tile_h: 10,
            cols: 8,
        })
    );
}

/// Listing reads the files and nothing else: the open project stays open as it was, no file in the library is
/// written, the stamps aren't touched and nothing is scanned for thumbnails.
#[test]
fn listing_never_opens_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, root) = folders(dir.path());
    let open = saved(&library, &Project::new("Open now"), 2_000);
    let other = saved(&library, &Project::new("Another"), 1_000);
    let state = ProjectSessionState::default();
    open_in(&state, &open).unwrap();
    stamped(&stamps, &[(&open, 3_000)]);
    let session = |state: &ProjectSessionState| {
        let held = lock(state);
        let held = held.as_ref().expect("a project is open");
        (
            held.file.clone(),
            held.document.project().id.clone(),
            held.document.revision(),
            state.opens.load(Ordering::SeqCst),
        )
    };
    let on_disk = || {
        let mut files: Vec<(PathBuf, Vec<u8>, SystemTime)> = fs::read_dir(&library)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let modified = fs::metadata(&path).unwrap().modified().unwrap();
                (path.clone(), fs::read(&path).unwrap(), modified)
            })
            .collect();
        files.sort();
        (files, fs::read(&stamps).unwrap())
    };
    let before = (session(&state), on_disk());

    let rows = recent_in(&library, &stamps, Some(&root), RECENT_LIMIT).unwrap();
    assert_eq!(names(&rows), [("Open now", 3_000), ("Another", 1_000)]);
    assert_eq!(rows[1].path, other.to_string_lossy());
    assert_eq!((session(&state), on_disk()), before);
    assert!(!root.exists(), "nothing was scanned");
}

/// The project open now is marked, so the project menu's Open recent can leave it out: by its file as written, or by
/// another way to the same file (a link to the folder). Listing alone, or with none open, marks none.
#[test]
fn the_open_project_is_marked() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let open = saved(&library, &Project::new("Open now"), 2_000);
    saved(&library, &Project::new("Another"), 1_000);
    let marked = |rows: &[ProjectRow]| {
        rows.iter()
            .map(|row| (row.name.clone(), row.open))
            .collect::<Vec<_>>()
    };
    let none = [
        ("Open now".to_owned(), false),
        ("Another".to_owned(), false),
    ];
    let mut rows = recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap();
    assert_eq!(marked(&rows), none);
    mark_open(&mut rows, None);
    assert_eq!(marked(&rows), none);

    mark_open(&mut rows, Some(&open));
    assert_eq!(
        marked(&rows),
        [("Open now".to_owned(), true), ("Another".to_owned(), false)]
    );

    #[cfg(unix)]
    {
        let link = dir.path().join("Library link");
        std::os::unix::fs::symlink(&library, &link).unwrap();
        mark_open(&mut rows, None);
        mark_open(&mut rows, Some(&link.join("Open now.klipolo")));
        assert_eq!(
            marked(&rows),
            [("Open now".to_owned(), true), ("Another".to_owned(), false)]
        );
    }
}

/// `recent.json` as it is on the disk.
fn on_disk(stamps: &Path) -> BTreeMap<String, u64> {
    serde_json::from_slice(&fs::read(stamps).unwrap()).unwrap()
}

fn key(file: &Path) -> String {
    file.to_string_lossy().into_owned()
}

/// Every new project and every open is stamped in `recent.json` beside the last-project memo, so it lists first;
/// the file keeps the 50 latest, and stamping a file again moves it rather than adding it twice.
#[test]
fn opening_or_creating_stamps_recent_json_and_keeps_50() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let earlier = saved(&library, &Project::new("Made earlier"), 1_000);
    let state = ProjectSessionState::default();
    state.remember_last_in(stamps.with_file_name("last-project"));
    let start = now_ms();
    create_in(&state, &library, new_project("Made now", vec![])).unwrap();
    let made = library.join("Made now.klipolo");
    open_in(&state, &earlier).unwrap();

    let opened = on_disk(&stamps);
    assert_eq!(opened.len(), 2);
    let (made_ms, earlier_ms) = (opened[&key(&made)], opened[&key(&earlier)]);
    assert!(start <= made_ms && made_ms <= earlier_ms && earlier_ms <= now_ms());
    assert_eq!(
        names(&recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap()),
        [("Made earlier", earlier_ms), ("Made now", made_ms)]
    );

    // Sixty opens long ago, a millisecond apart, beside the two above: the next stamp keeps the 50 latest, so the two
    // above and 48 of these stay.
    let old = |n: u64| library.join(format!("Old {n:02}.klipolo"));
    let olds: Vec<PathBuf> = (0..60).map(old).collect();
    let mut opened: Vec<(&Path, u64)> = olds
        .iter()
        .zip(100..)
        .map(|(file, ms)| (file.as_path(), ms))
        .collect();
    opened.extend([(made.as_path(), made_ms), (earlier.as_path(), earlier_ms)]);
    stamped(&stamps, &opened);
    stamp(&stamps, &earlier, now_ms()).unwrap();
    let kept = on_disk(&stamps);
    assert_eq!(kept.len(), 50);
    let mut expected: Vec<String> = (12..60).map(|n| key(&old(n))).collect();
    expected.extend([key(&made), key(&earlier)]);
    expected.sort();
    assert_eq!(kept.keys().cloned().collect::<Vec<_>>(), expected);
    assert!(kept[&key(&earlier)] > made_ms);

    // Opened again, a file moves to the top rather than being added twice.
    stamp(&stamps, &old(12), now_ms()).unwrap();
    let again = on_disk(&stamps);
    assert_eq!(again.len(), 50);
    assert!(again[&key(&old(12))] > again[&key(&earlier)]);
}

/// A projects folder that can't be listed (here, a file where the folder should be) fails, so the start screen says so
/// instead of showing an empty library; an empty folder is an empty list.
#[test]
fn a_library_that_cant_be_listed_fails() {
    let dir = tempfile::tempdir().unwrap();
    let stamps = dir.path().join("recent.json");
    let not_a_folder = dir.path().join("Klipolo");
    fs::write(&not_a_folder, "a file").unwrap();
    assert!(recent_in(&not_a_folder, &stamps, None, RECENT_LIMIT).is_err());

    let empty = dir.path().join("Empty");
    fs::create_dir_all(&empty).unwrap();
    assert!(
        recent_in(&empty, &stamps, None, RECENT_LIMIT)
            .unwrap()
            .is_empty()
    );
}

/// A link to a project isn't listed: its first save would replace the link with a file and leave what it pointed at
/// behind.
#[cfg(unix)]
#[test]
fn links_are_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let target = saved(
        &dir.path().join("Elsewhere"),
        &Project::new("Linked"),
        2_000,
    );
    std::os::unix::fs::symlink(&target, library.join("Linked.klipolo")).unwrap();
    saved(&library, &Project::new("Here"), 1_000);

    assert_eq!(
        names(&recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap()),
        [("Here", 1_000)]
    );
}

/// A project just opened is always stamped newest, so it lists first and is never the stamp let go: stamps written
/// while the clock ran ahead (here, a day) come back to now first, a millisecond apart in the order they were written,
/// and the new stamp is a millisecond after the latest. Until then a time later than now reads as now, and the list
/// keeps the order the times were written in.
#[test]
fn a_stamp_is_always_the_newest_and_future_times_read_as_now() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let ahead = saved(
        &library,
        &Project::new("Opened while the clock ran ahead"),
        1_000,
    );
    let before = saved(&library, &Project::new("Opened just before it"), 1_000);
    let after = saved(&library, &Project::new("Opened after"), 1_000);
    let now = now_ms();
    let tomorrow = now + 86_400_000;
    // 48 projects opened "tomorrow" too (no longer in the library), a millisecond apart, before the two above.
    let others: Vec<PathBuf> = (0..48)
        .map(|n| dir.path().join(format!("Moved {n:02}.klipolo")))
        .collect();
    let mut opened: Vec<(&Path, u64)> = others
        .iter()
        .zip(tomorrow - 100..)
        .map(|(file, ms)| (file.as_path(), ms))
        .collect();
    opened.extend([
        (before.as_path(), tomorrow - 1),
        (ahead.as_path(), tomorrow),
    ]);
    stamped(&stamps, &opened);

    let rows = recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        [
            "Opened while the clock ran ahead",
            "Opened just before it",
            "Opened after"
        ]
    );
    let read_at = now_ms();
    assert!(
        rows[..2]
            .iter()
            .all(|row| now <= row.opened_ms && row.opened_ms <= read_at),
        "a time later than now reads as now"
    );

    stamp(&stamps, &after, now).unwrap();
    let written = on_disk(&stamps);
    assert_eq!(written.len(), 50);
    assert_eq!(
        [
            written[&key(&others[1])],
            written[&key(&before)],
            written[&key(&ahead)],
            written[&key(&after)]
        ],
        [now - 48, now - 1, now, now + 1],
        "back to now in their order, and the new stamp after them"
    );
    assert!(
        !written.contains_key(&key(&others[0])),
        "the oldest stamp went, not the new one"
    );
    assert_eq!(
        names(&recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap())
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        [
            "Opened after",
            "Opened while the clock ran ahead",
            "Opened just before it"
        ]
    );
}

/// A stamp written while the clock ran far ahead (a year) doesn't drag the stamps after it ahead with it: once it is
/// back to now, each later open is stamped at its own time.
#[test]
fn a_stamp_ahead_of_the_clock_drags_no_later_stamp() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let ahead = saved(&library, &Project::new("Ahead"), 1_000);
    let first = saved(&library, &Project::new("First"), 1_000);
    let second = saved(&library, &Project::new("Second"), 1_000);
    // A fixed clock in the past, so the list shows every time as written.
    let now: u64 = 1_700_000_000_000;
    stamped(&stamps, &[(&ahead, now + 365 * 86_400_000)]);

    stamp(&stamps, &first, now).unwrap();
    stamp(&stamps, &second, now + 60_000).unwrap();
    let written = on_disk(&stamps);
    assert_eq!(
        [
            written[&key(&ahead)],
            written[&key(&first)],
            written[&key(&second)]
        ],
        [now, now + 1, now + 60_000]
    );
    assert_eq!(
        names(&recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap()),
        [("Second", now + 60_000), ("First", now + 1), ("Ahead", now)]
    );
}

/// A file whose time is unknown (0) lists after every project with a time, stamped or not.
#[test]
fn unknown_times_list_last() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let opened = saved(&library, &Project::new("Opened"), 1_000);
    let zero_stamp = saved(&library, &Project::new("Stamped at 0"), 3_000);
    saved(&library, &Project::new("Saved long ago"), 1);
    saved(&library, &Project::new("No time"), 0);
    stamped(&stamps, &[(&opened, 5_000), (&zero_stamp, 0)]);

    assert_eq!(
        names(&recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap()),
        [
            ("Opened", 5_000),
            ("Saved long ago", 1),
            ("Stamped at 0", 0),
            ("No time", 0)
        ]
    );
}

/// Projects that share a name ("Untitled project" twice, after New project twice) are named by their files, so no two
/// cards read the same; a name of its own stays.
#[test]
fn projects_that_share_a_name_show_their_files() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    for (file, ms) in [("Untitled project", 2_000), ("Untitled project 2", 3_000)] {
        let file = library.join(format!("{file}.klipolo"));
        save(
            &file,
            &Project::new("Untitled project"),
            Durability::Ordered,
        )
        .unwrap();
        set_saved(&file, ms);
    }
    let trip = library.join("Holiday.klipolo");
    save(&trip, &Project::new("Trip"), Durability::Ordered).unwrap();
    set_saved(&trip, 1_000);

    assert_eq!(
        names(&recent_in(&library, &stamps, None, RECENT_LIMIT).unwrap()),
        [
            ("Untitled project 2", 3_000),
            ("Untitled project", 2_000),
            ("Trip", 1_000)
        ]
    );
}

/// Stamping over a `recent.json` that doesn't read as stamps starts it over with the one stamp.
#[test]
fn stamping_rewrites_a_damaged_recent_json() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    fs::create_dir_all(stamps.parent().unwrap()).unwrap();
    fs::write(&stamps, "{ not json").unwrap();
    let file = library.join("Opened.klipolo");

    stamp(&stamps, &file, 5_000).unwrap();
    assert_eq!(on_disk(&stamps), BTreeMap::from([(key(&file), 5_000)]));
}

/// A `recent.json` that can't be read at all (no permission, a failing disk) is left as it is: the stamp is skipped
/// rather than wiping the others.
#[cfg(unix)]
#[test]
fn stamping_leaves_a_recent_json_it_cant_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, _) = folders(dir.path());
    let kept = library.join("Kept.klipolo");
    stamped(&stamps, &[(&kept, 5_000)]);
    fs::set_permissions(&stamps, fs::Permissions::from_mode(0o000)).unwrap();
    // Run as root, the file still reads: nothing to check.
    if fs::read(&stamps).is_ok() {
        return;
    }

    assert!(stamp(&stamps, &library.join("New.klipolo"), 6_000).is_err());
    fs::set_permissions(&stamps, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(on_disk(&stamps), BTreeMap::from([(key(&kept), 5_000)]));
}
