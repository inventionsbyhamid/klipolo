//! The start screen's posters, made by the engine's poster maker from real footage, or by a stand-in that writes a
//! small one, while the holds say when.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_engine::media::key::{CacheKind, stem};
use klipolo_engine::media::thumbs::{self, Poster};
use klipolo_model::crop::Crop;
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_platform::durable::{Durability, Os};

use super::*;
use crate::project::recent::{RECENT_LIMIT, poster_wants, recent_in};
use crate::project::save;
use klipolo_app::analysis::holds::{Holds, SETTLE};

use crate::media::tests::fixture_set;

fn fixture(name: &str) -> PathBuf {
    let path = fixture_set::fixture_dir().join(name);
    assert!(
        path.exists(),
        "{name} is missing: run scripts/gen-engine-fixtures.sh"
    );
    path
}

fn video(id: &str, file: &Path) -> Asset {
    Asset {
        id: id.into(),
        kind: AssetKind::Video,
        path: file.to_string_lossy().into_owned(),
        name: file.file_name().unwrap().to_string_lossy().into_owned(),
        duration_us: 2_000_000,
        width: 320,
        height: 180,
        fps: 30.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
    }
}

/// A project named `name` whose one clip shows `file` (cropped by `crop`), saved in `library` and never opened.
fn saved(library: &Path, name: &str, file: &Path, crop: Option<Crop>) -> PathBuf {
    let mut project = Project::new(name);
    project.assets = vec![video("a", file)];
    let mut clip = Clip::new(ClipSource::Asset { id: "a".into() }, 0, 1_000_000);
    clip.crop = crop;
    project.timeline.main = vec![clip];
    let path = library.join(format!("{name}.klipolo"));
    save(&path, &project, Durability::Ordered).unwrap();
    path
}

fn wait_until(done: impl Fn() -> bool, within: Duration) {
    let until = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < until, "still waiting after {within:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What the start screen heard.
type Heard = Arc<Mutex<Vec<ProjectPoster>>>;

fn posters(root: &Path, holds: Arc<Holds>, make: Arc<Make>) -> (Posters, Heard) {
    let heard: Heard = Arc::default();
    let hear = heard.clone();
    let posters = Posters::new(
        root.to_path_buf(),
        holds,
        make,
        Arc::new(move |poster| hear.lock().unwrap().push(poster)),
    );
    (posters, heard)
}

/// Never-opened projects get sharp cards: listing them asks for their first files' posters, which the engine's maker
/// makes one file at a time from the footage (a file two projects open with is made once, for both cards), each card
/// told with its own crop. Listing again shows them, and asks for nothing more. Nothing opens a project.
#[test]
fn posters_for_never_opened_projects_appear_without_opening_them() {
    let dir = tempfile::tempdir().unwrap();
    let (library, stamps, root) = (
        dir.path().join("Klipolo"),
        dir.path().join("data").join("recent.json"),
        dir.path().join("thumbs"),
    );
    fs::create_dir_all(&library).unwrap();
    let grey = fixture("ntsc-luma-10s.mp4");
    let phone = fixture("phone-portrait.mov");
    let cut = Crop {
        left: 0.1,
        top: 0.0,
        right: 0.1,
        bottom: 0.2,
    };
    let walk = saved(&library, "Walk", &grey, None);
    let again = saved(&library, "Walk again", &grey, None);
    let call = saved(&library, "Call", &phone, Some(cut));

    let rows = recent_in(&library, &stamps, Some(&root), RECENT_LIMIT).unwrap();
    assert!(
        rows.iter().all(|row| row.thumb.is_none()),
        "nothing made yet"
    );
    let wants = poster_wants(&rows, &root);
    let files: BTreeSet<&str> = wants.iter().map(|w| w.file.as_str()).collect();
    assert_eq!(files.len(), 2, "one want per file: {wants:?}");

    let made = Arc::new(AtomicUsize::new(0));
    let counted = made.clone();
    let make: Arc<Make> = Arc::new(move |root: &Path, file: &str| {
        counted.fetch_add(1, Ordering::SeqCst);
        klipolo_engine::media::poster::make_poster(root, &Os, file)
    });
    let (posters, heard) = posters(&root, Arc::new(Holds::default()), make);
    posters.want(wants);
    wait_until(|| !posters.pending(), Duration::from_secs(20));
    assert_eq!(made.load(Ordering::SeqCst), 2, "each file once");

    let heard = heard.lock().unwrap().clone();
    let told = |project: &Path| {
        heard
            .iter()
            .find(|p| p.path == project.to_string_lossy())
            .unwrap_or_else(|| panic!("{project:?} told: {heard:?}"))
            .thumb
            .clone()
    };
    for (project, crop, size) in [
        (&walk, None, (320, 180)),
        (&again, None, (320, 180)),
        (&call, Some(cut), (202, 360)),
    ] {
        let thumb = told(project);
        assert_eq!(
            (thumb.tile_w, thumb.tile_h, thumb.cols),
            (size.0, size.1, 1)
        );
        assert_eq!(thumb.crop, crop, "{project:?}");
        assert!(Path::new(&thumb.sprite).is_file());
    }

    let rows = recent_in(&library, &stamps, Some(&root), RECENT_LIMIT).unwrap();
    for row in &rows {
        let thumb = row.thumb.as_ref().expect("the poster");
        assert_eq!(thumb.cols, 1, "{}", row.name);
        assert_eq!(thumb.sprite, told(Path::new(&row.path)).sprite);
    }
    assert_eq!(
        poster_wants(&rows, &root),
        Vec::new(),
        "nothing more to make"
    );
    assert!(!stamps.exists(), "no project was opened");
}

/// A stand-in maker: writes a small current poster for `file` and counts the files it made, one at a time.
struct Stand {
    made: Mutex<Vec<String>>,
    busy: AtomicUsize,
    most: AtomicUsize,
}

impl Stand {
    fn new() -> Arc<Stand> {
        Arc::new(Stand {
            made: Mutex::default(),
            busy: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
        })
    }

    fn maker(self: &Arc<Self>) -> Arc<Make> {
        let stand = self.clone();
        Arc::new(move |root: &Path, file: &str| {
            let busy = stand.busy.fetch_add(1, Ordering::SeqCst) + 1;
            stand.most.fetch_max(busy, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            let key = stem(file, &CacheKind::Thumbs).ok_or("gone")?;
            let (luma, chroma) = (vec![100; 4 * 2], vec![128; 2]);
            let planes = [luma.as_slice(), chroma.as_slice(), chroma.as_slice()];
            thumbs::write_poster(&Os, root, &key, (4, 2), planes, [4, 2, 2])
                .map_err(|e| e.to_string())?;
            stand.made.lock().unwrap().push(file.to_owned());
            stand.busy.fetch_sub(1, Ordering::SeqCst);
            Ok(thumbs::poster(root, file))
        })
    }

    fn made(&self) -> Vec<String> {
        self.made.lock().unwrap().clone()
    }
}

/// Stand-in media files in `dir` (made once: a file written again is another file to the cache), and a want for each
/// (its own card).
fn wants_for(dir: &Path, names: &[&str]) -> Vec<PosterWant> {
    names
        .iter()
        .map(|name| {
            let file = dir.join(name);
            if !file.exists() {
                fs::write(&file, format!("not really {name}")).unwrap();
            }
            let file = file.to_string_lossy().into_owned();
            PosterWant {
                cards: vec![(format!("{file}.klipolo"), None)],
                file,
            }
        })
        .collect()
}

/// Posters are made one file at a time, in the list's order; a new list replaces what was still waiting.
#[test]
fn posters_are_made_one_at_a_time_in_the_lists_order() {
    let dir = tempfile::tempdir().unwrap();
    let stand = Stand::new();
    let holds = Arc::new(Holds::default());
    let (posters, heard) = posters(&dir.path().join("thumbs"), holds, stand.maker());
    let wants = wants_for(dir.path(), &["a.mp4", "b.mp4", "c.mp4"]);
    let order: Vec<String> = wants.iter().map(|w| w.file.clone()).collect();
    posters.want(wants);
    wait_until(|| !posters.pending(), Duration::from_secs(5));
    assert_eq!(stand.made(), order);
    assert_eq!(stand.most.load(Ordering::SeqCst), 1, "never two at once");
    assert_eq!(heard.lock().unwrap().len(), 3);
    // Asked again (the list read again): every poster is there, so nothing is made twice.
    posters.want(wants_for(dir.path(), &["a.mp4", "b.mp4", "c.mp4"]));
    wait_until(|| !posters.pending(), Duration::from_secs(5));
    assert_eq!(stand.made().len(), 3);
}

/// While the video plays (and for a moment after), while an export runs and while a take records, posters wait, and
/// keep their place; then they are made.
#[test]
fn posters_wait_while_the_video_plays_an_export_runs_or_a_take_records() {
    let dir = tempfile::tempdir().unwrap();
    let stand = Stand::new();
    let holds = Arc::new(Holds::default());
    let (posters, _) = posters(&dir.path().join("thumbs"), holds.clone(), stand.maker());
    let held = Duration::from_millis(300);

    holds.set_playing(true);
    posters.want(wants_for(dir.path(), &["played.mp4"]));
    std::thread::sleep(held);
    assert!(stand.made().is_empty(), "waits while the video plays");
    assert!(posters.pending(), "and keeps its place");
    holds.set_playing(false);
    wait_until(|| stand.made().len() == 1, SETTLE + Duration::from_secs(3));

    holds.set_exporting(true);
    posters.want(wants_for(dir.path(), &["exported.mp4"]));
    std::thread::sleep(held);
    assert_eq!(stand.made().len(), 1, "waits while an export runs");
    holds.set_exporting(false);
    wait_until(|| stand.made().len() == 2, Duration::from_secs(3));

    holds.set_recording(true);
    posters.want(wants_for(dir.path(), &["recorded.mp4"]));
    std::thread::sleep(held);
    assert_eq!(stand.made().len(), 2, "waits while a take records");
    holds.set_recording(false);
    wait_until(|| stand.made().len() == 3, Duration::from_secs(3));
}

/// A file without a picture, or whose poster can't be made, isn't tried again in this run: the start screen asks on
/// every listing.
#[test]
fn a_file_whose_poster_cant_be_made_is_not_tried_again() {
    let dir = tempfile::tempdir().unwrap();
    let tries = Arc::new(Mutex::new(Vec::new()));
    let tried = tries.clone();
    let make: Arc<Make> = Arc::new(
        move |_: &Path, file: &str| -> Result<Option<Poster>, String> {
            tried.lock().unwrap().push(file.to_owned());
            if file.ends_with("sound.m4a") {
                Ok(None)
            } else {
                Err("The file can't be read.".into())
            }
        },
    );
    let (posters, heard) = posters(&dir.path().join("thumbs"), Arc::new(Holds::default()), make);
    for _ in 0..2 {
        posters.want(wants_for(dir.path(), &["sound.m4a", "damaged.mp4"]));
        wait_until(|| !posters.pending(), Duration::from_secs(5));
    }
    let tried = tries.lock().unwrap().clone();
    assert_eq!(tried.len(), 2, "{tried:?}");
    assert!(heard.lock().unwrap().is_empty());
}
