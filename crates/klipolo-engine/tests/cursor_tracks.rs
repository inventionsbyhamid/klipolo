//! Cursor takes: every take with a cursor on the timeline, its facts and one smoothed path per smoothing its
//! clips use, built on the loader thread under the 64 MB budget, the same in preview and export.

mod support;

use std::sync::Arc;
use std::time::Duration;

use klipolo_cursor::format::{Capture, Line};
use klipolo_engine::media::cursor::{CursorTracks, Want, load_take, wanted};
use klipolo_model::cursor_style::{CursorCapture, CursorStyle, Smoothing};
use klipolo_model::project::Project;
use klipolo_model::timeline::{Clip, ClipSource};
use support::takes::{add_take, cursor_file, cursor_takes, gliding, header, write};

const WAIT: Duration = Duration::from_secs(30);

fn style(smoothing: Smoothing) -> Option<CursorStyle> {
    Some(CursorStyle {
        smoothing,
        ..CursorStyle::default()
    })
}

#[test]
fn every_take_on_the_timeline_is_wanted_with_its_styles_smoothings() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = cursor_takes(2, dir.path(), CursorCapture::Separate);
    // Clip 2 draws Strong; clip 3 plays take 2 again with the project's Medium, clip 4 take 1 again, then b-roll.
    p.timeline.main[1].cursor = style(Smoothing::Strong);
    let mut again = p.timeline.main[1].clone();
    (again.id, again.cursor) = ("clip3".into(), None);
    let mut first = p.timeline.main[0].clone();
    first.id = "clip4".into();
    let broll = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, 1_000_000);
    p.timeline.main.extend([again, first, broll]);
    // A burned-in take (nothing is drawn over its frames), a hidden cursor and one drawn as recorded: wanted for their
    // facts, with no smoothing.
    for (k, capture) in [
        (3, CursorCapture::Burned),
        (4, CursorCapture::Separate),
        (5, CursorCapture::Separate),
    ] {
        add_take(&mut p, k, &cursor_file(dir.path(), k), capture);
    }
    let n = p.timeline.main.len();
    p.timeline.main[n - 2].cursor = Some(CursorStyle {
        hidden: true,
        ..CursorStyle::default()
    });
    p.timeline.main[n - 1].cursor = Some(CursorStyle {
        as_recorded: true,
        ..CursorStyle::default()
    });
    let want = |k: usize, smoothings: Vec<Smoothing>| {
        let path = cursor_file(dir.path(), k).to_string_lossy().into_owned();
        (path, smoothings)
    };
    let smoothed = |w: Want| (w.path, w.smoothings);
    assert_eq!(
        wanted(&p).into_iter().map(smoothed).collect::<Vec<_>>(),
        [
            want(1, vec![Smoothing::Medium]),
            want(2, vec![Smoothing::Medium, Smoothing::Strong]),
            want(3, vec![]),
            want(4, vec![]),
            want(5, vec![]),
        ]
    );
}

#[tokio::test]
async fn a_take_loads_with_its_smoothings_off_the_render_thread_and_wakes_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = cursor_file(dir.path(), 1);
    // Ten minutes at 120 Hz: far more than `want` could read before it returns.
    write(&path, &gliding(Capture::Separate, 600.0));
    let mut p = Project::new("One");
    add_take(&mut p, 1, &path, CursorCapture::Separate);
    let wants = wanted(&p);
    let tracks = CursorTracks::default();
    let wake = tracks.wake();
    tracks.want(&wants);
    assert!(
        tracks.get(&wants[0].path).is_none(),
        "`want` only queues: the take is read and smoothed on the loader thread"
    );
    let built = |t: &CursorTracks| {
        t.get(&wants[0].path)
            .is_some_and(|take| take.smoothed(Smoothing::Medium).is_some())
    };
    tokio::time::timeout(WAIT, async {
        while !built(&tracks) {
            wake.notified().await;
        }
    })
    .await
    .expect("the session is woken once the take and its smoothing are in");
    let take = tracks.get(&wants[0].path).unwrap();
    assert_eq!(take.track.len(), 72_000);
    assert!(!take.smoothed(Smoothing::Medium).unwrap().is_empty());
    assert!(
        take.smoothed(Smoothing::Strong).is_none(),
        "only what its clips use"
    );
}

#[test]
fn preview_and_export_hold_the_same_takes_under_the_budget() {
    // The test, over takes: a budget of three and a half takes, each with its Medium path.
    let dir = tempfile::tempdir().unwrap();
    let p = cursor_takes(6, dir.path(), CursorCapture::Separate);
    let wants = wanted(&p);
    let one = load_take(std::path::Path::new(&wants[0].path)).unwrap();
    one.prepare(&wants[0].smoothings);
    let one = one.bytes();
    let budget = 3 * one + one / 2;
    let export = CursorTracks::with_budget(budget);
    export.load_all(&wants);
    let preview = CursorTracks::with_budget(budget);
    preview.want(&wants);
    assert!(preview.wait_ready(&wants, WAIT));
    let first_three: Vec<String> = wants[..3].iter().map(|w| w.path.clone()).collect();
    for tracks in [&export, &preview] {
        let held = tracks.fitting(&wants);
        let paths: Vec<String> = held.iter().map(|(path, _)| path.clone()).collect();
        assert_eq!(paths, first_three);
        assert!(held.iter().all(|(_, take)| take.bytes() == one));
        // The loader stops once the takes it holds pass the budget: the fourth loads, the last two never do.
        assert!(tracks.get(&wants[3].path).is_some());
        assert!(wants[4..].iter().all(|w| tracks.get(&w.path).is_none()));
    }
}

#[test]
fn a_style_change_builds_one_more_variant_and_a_variant_no_clip_uses_goes() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = cursor_takes(1, dir.path(), CursorCapture::Separate);
    let tracks = CursorTracks::default();
    let ready = |p: &Project| {
        let wants = wanted(p);
        tracks.want(&wants);
        assert!(tracks.wait_ready(&wants, WAIT));
        tracks.get(&wants[0].path).unwrap()
    };
    let medium = ready(&p);
    assert!(medium.smoothed(Smoothing::Medium).is_some());
    assert_eq!(medium.variants(), 1);
    let with_medium = medium.bytes();
    // Strong is built on the same take (never read again), and Medium, which no clip uses now, goes.
    p.cursor.smoothing = Smoothing::Strong;
    let strong = ready(&p);
    assert!(Arc::ptr_eq(&medium, &strong));
    assert!(strong.smoothed(Smoothing::Strong).is_some());
    assert!(strong.smoothed(Smoothing::Medium).is_none());
    assert_eq!(strong.variants(), 1);
    // A second clip of the take drawn Light: both are held (a take holds at most four, least recently used out first).
    let mut light = p.timeline.main[0].clone();
    (light.id, light.cursor) = ("light".into(), style(Smoothing::Light));
    p.timeline.main.push(light);
    let both = ready(&p);
    assert!(
        both.smoothed(Smoothing::Light).is_some() && both.smoothed(Smoothing::Strong).is_some()
    );
    assert_eq!(both.variants(), 2);
    // Off draws the raw path: no variant at all.
    p.cursor.smoothing = Smoothing::Off;
    p.timeline.main.truncate(1);
    let off = ready(&p);
    assert_eq!(off.variants(), 0);
    assert!(off.bytes() < with_medium);
    // The rest scan goes with its last path, so this take counts what export's fresh one does (the same
    // prefix of takes at the budget's edge).
    let fresh = load_take(&cursor_file(dir.path(), 1)).unwrap();
    fresh.prepare(&[]);
    assert_eq!(off.bytes(), fresh.bytes());
}

#[test]
fn a_style_change_draws_the_path_held_until_the_new_one_lands() {
    // While Strong builds, the take still holds Medium (until `keep`), and that is what draws, so a
    // paused frame never steps to the raw path and back.
    let dir = tempfile::tempdir().unwrap();
    cursor_takes(1, dir.path(), CursorCapture::Separate);
    let take = load_take(&cursor_file(dir.path(), 1)).unwrap();
    take.prepare(&[Smoothing::Medium]);
    let t = 2_000_000;
    let drawn = |s| take.with_drawn(s, |path| path.position(&take.track, t));
    let medium = take.smoothed(Smoothing::Medium).unwrap();
    let raw = take.track.position(t);
    assert_ne!(medium.position(&take.track, t), raw, "they differ there");
    assert_eq!(drawn(Smoothing::Strong), medium.position(&take.track, t));
    assert_eq!(drawn(Smoothing::Off), raw, "Off draws the raw path");
    take.prepare(&[Smoothing::Strong]);
    let strong = take.smoothed(Smoothing::Strong).unwrap();
    assert_eq!(drawn(Smoothing::Strong), strong.position(&take.track, t));
    // Nothing held: the raw path.
    take.prepare(&[]);
    assert_eq!(drawn(Smoothing::Light), raw);
}

/// A PNG's first 24 bytes (signature, IHDR) for a `w` × `h` image: all a take reads of it.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    b.extend(b"IHDR");
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    b.extend([8, 6, 0, 0, 0]);
    b
}

#[test]
fn a_shape_file_that_is_missing_too_large_or_outside_the_take_is_not_ok() {
    let dir = tempfile::tempdir().unwrap();
    let file = cursor_file(dir.path(), 1);
    let cursors = file.parent().unwrap().join("cursors");
    std::fs::create_dir_all(&cursors).unwrap();
    std::fs::write(cursors.join("0.png"), png(32, 32)).unwrap();
    // 1: missing.
    std::fs::write(cursors.join("2.png"), png(300, 300)).unwrap();
    // 3: a link out of the take (a crafted project); without links it is missing.
    std::fs::write(dir.path().join("outside.png"), png(32, 32)).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("outside.png"), cursors.join("3.png")).unwrap();
    std::fs::write(cursors.join("4.png"), b"not a picture at all").unwrap();
    std::fs::create_dir(cursors.join("5.png")).unwrap();
    let mut lines = vec![header(Capture::Separate)];
    lines.extend((0..6).map(|id| Line::ShapeDef {
        id,
        key: format!("{:016x}", id + 1),
        file: format!("cursors/{id}.png"),
        pw: 32,
        ph: 32,
        hx: 1.0,
        hy: 1.0,
        pts: 16.0,
        sys: None,
    }));
    lines.push(Line::Move {
        t: 0,
        x: 0.5,
        y: 0.5,
    });
    write(&file, &lines);
    let take = load_take(&file).unwrap();
    assert_eq!(take.track.shapes().len(), 6);
    let usable: Vec<bool> = take.shape_files.iter().map(Option::is_some).collect();
    assert_eq!(usable, [true, false, false, false, false, false]);
    assert!(take.shape_usable(0) && !take.shape_usable(3) && !take.shape_usable(40));
}

#[test]
fn an_unreadable_file_is_none_and_tried_again_on_the_next_publish() {
    // A drive mounted after the project opened.
    let dir = tempfile::tempdir().unwrap();
    let path = cursor_file(dir.path(), 1);
    let mut p = Project::new("One");
    add_take(&mut p, 1, &path, CursorCapture::Separate);
    let wants = wanted(&p);
    let tracks = CursorTracks::default();
    tracks.want(&wants);
    assert!(
        tracks.wait_ready(&wants, WAIT),
        "settled: the take draws nothing"
    );
    assert!(tracks.get(&wants[0].path).is_none());
    assert!(tracks.fitting(&wants).is_empty());
    write(&path, &gliding(Capture::Separate, 2.0));
    tracks.want(&wants);
    assert!(tracks.wait_ready(&wants, WAIT));
    let take = tracks.get(&wants[0].path).expect("tried again");
    assert!(take.smoothed(Smoothing::Medium).is_some());
}
