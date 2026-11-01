//! How the cursor files Auto zooms plan from load: bounded and decimated, a damaged or
//! missing file never an error, off the actor, tried again, a project saved before the cursor layer, six takes and the
//! byte budget, the same in preview and export.

use std::io::Write;
use std::sync::Arc;

use klipolo_cursor::CursorTrack;
use klipolo_engine::media::cursor::{CursorTracks, Want, load_take, load_track, wanted};
use klipolo_engine::plan::PlanContext;
use klipolo_engine::plan::zoom::{auto_rect, zoom_items};
use klipolo_engine::registry::Registry;
use klipolo_model::cursor_style::RecordingCursor;
use klipolo_model::project::Project;
use klipolo_model::timeline::ZoomTarget;

use super::support;
use super::{in_safe_zone, track};

fn planned(p: &Project, tracks: &Arc<CursorTracks>) -> PlanContext {
    PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), tracks.clone())
}

#[test]
fn a_long_take_is_decimated_and_bounded() {
    let f = tempfile::NamedTempFile::new().unwrap();
    // Buffered: 1.3 million lines.
    let mut w = std::io::BufWriter::new(f.as_file());
    for i in 0..(3 * 3_600 * 120_i64) {
        // three hours at 120 Hz, a click of one sample every 5 s
        let t = i * 8_333;
        writeln!(w, r#"{{"k":"m","t":{t},"x":0.5,"y":0.5}}"#).unwrap();
        if i % 600 < 2 {
            let d = u8::from(i % 600 == 0);
            writeln!(w, r#"{{"k":"b","t":{t},"btn":0,"d":{d},"x":0.5,"y":0.5}}"#).unwrap();
        }
    }
    w.flush().unwrap();
    drop(w);
    let t = load_track(f.path()).unwrap();
    assert!(t.bytes() <= CursorTrack::MAX_BYTES);
    assert_eq!(t.clicks().count(), 3 * 3_600 * 120 / 600);
}

/// A cursor moving left to right over 3 s, sampled at 30 Hz, clicking from 1 s to 1.13 s.
fn moving_right() -> CursorTrack {
    let x = |i: i64| 0.1 + 0.8 * i as f32 / 90.0;
    let mut lines: Vec<String> = (0..=90)
        .map(|i| format!(r#"{{"k":"m","t":{},"x":{},"y":0.5}}"#, i * 33_333, x(i)))
        .collect();
    for (i, d) in [(30, 1), (34, 0)] {
        let (t, x) = (i * 33_333, x(i));
        lines.push(format!(
            r#"{{"k":"b","t":{t},"btn":0,"d":{d},"x":{x},"y":0.5}}"#
        ));
    }
    track(&lines)
}

/// `moving_right` as the first builds wrote it: format 1, one line per frame with no `k`, the primary button's bit
/// while it was held (here at 1–1.1 s).
fn moving_right_v1() -> CursorTrack {
    let lines: Vec<String> = (0..=90)
        .map(|i| {
            let (t, x) = (i * 33_333, 0.1 + 0.8 * i as f32 / 90.0);
            match i {
                30..=33 => format!(r#"{{"t":{t},"x":{x},"y":0.5,"b":1}}"#),
                _ => format!(r#"{{"t":{t},"x":{x},"y":0.5}}"#),
            }
        })
        .collect();
    track(&lines)
}

#[test]
fn a_project_saved_before_the_cursor_layer_still_follows_its_cursor() {
    // Early builds wrote the take's cursor file as `cursorPath`, and the file itself in format 1: both still read, so
    // its Auto zoom plans exactly as a take recorded now does.
    let (p, tracks) = support::recording::auto_zoom_project(moving_right(), 0.0, 3.0, 2.0);
    let (p1, tracks1) = support::recording::auto_zoom_project(moving_right_v1(), 0.0, 3.0, 2.0);
    let v1 = tracks1.get(support::recording::CURSOR_PATH).unwrap();
    assert_eq!((v1.track.len(), v1.track.clicks().count()), (91, 1));
    let mut earlier = serde_json::to_value(&p1).unwrap();
    let recording = earlier["recordings"][0].as_object_mut().unwrap();
    let cursor = recording.remove("cursor").unwrap();
    recording.insert("cursorPath".into(), cursor["path"].clone());
    let earlier = klipolo_model::load_project(&earlier.to_string()).unwrap();
    let (now, then) = (
        planned(&p, &Arc::new(tracks)),
        planned(&earlier, &Arc::new(tracks1)),
    );
    assert_eq!(now.zoom.cameras().len(), 1, "it plans its camera");
    assert_eq!(now.zoom.cameras()[0].0, then.zoom.cameras()[0].0);
    for t in (0..3_000_000).step_by(16_667) {
        assert_eq!(now.zoom.window(t), then.zoom.window(t), "at {t}");
    }
}

#[test]
fn a_damaged_cursor_file_keeps_its_good_lines_and_a_missing_one_zooms_on_the_centre() {
    // A crash can tear the last line, and a power cut can leave zeros: they're skipped, never an error.
    let zeros = "\0".repeat(4_096);
    let lines = [
        r#"{"k":"m","t":0,"x":0.2,"y":0.2}"#,
        r#"{"k":"b","t":0,"btn":0,"d":1,"x":0.2,"y":0.2}"#,
        "garbage",
        &zeros,
        r#"{"k":"m","t":100000,"x":0.4,"y":0.4}"#,
        r#"{"k":"m","t":200000,"x":0.6"#,
    ];
    let t = track(&lines.map(String::from));
    assert_eq!((t.len(), t.clicks().count()), (2, 1));
    // A cursor file that is gone: the Auto zoom stays one centred item, in preview and export alike.
    let (mut p, _) = support::recording::auto_zoom_project(CursorTrack::default(), 0.0, 3.0, 2.0);
    let gone = "/nonexistent/rec/cursor.jsonl";
    p.recordings[0].cursor = Some(RecordingCursor::burned_in(gone));
    let tracks = CursorTracks::default();
    tracks.load_all(&wanted(&p));
    let centre = ZoomTarget::Rect {
        rect: auto_rect(None, 2.0),
    };
    assert_eq!(zoom_items(&p, &tracks), [(0, 3_000_000, 2.0, centre)]);
}

#[tokio::test]
async fn a_track_loads_off_the_thread_and_says_so() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, r#"{{"k":"m","t":0,"x":0.9,"y":0.5}}"#).unwrap();
    let path = f.path().to_str().unwrap();
    let tracks = CursorTracks::default();
    let loaded = tracks.wake();
    tracks.want(&[only(path)]);
    tokio::time::timeout(std::time::Duration::from_secs(10), loaded.notified())
        .await
        .expect("the load says when it's done");
    assert_eq!(tracks.get(path).map(|t| t.track.len()), Some(1));
}

/// A take wanted for its facts alone (no smoothing).
fn only(path: &str) -> Want {
    Want {
        path: path.into(),
        ..Want::default()
    }
}

/// Waits until every one of `wants` is loaded in `tracks` (each background load wakes it).
async fn loaded(tracks: &CursorTracks, wants: &[Want]) {
    let wake = tracks.wake();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !wants.iter().all(|w| tracks.get(&w.path).is_some()) {
            wake.notified().await;
        }
    })
    .await
    .expect("the background loads finish");
}

#[tokio::test]
async fn six_takes_follow_the_cursor_in_every_take_in_preview_and_export() {
    // A tutorial from six takes, each its own recording with an Auto zoom (the plan once read only four).
    let dir = tempfile::tempdir().unwrap();
    let p = support::recording::auto_zoom_takes(6, dir.path());
    let wanted = wanted(&p);
    assert_eq!(wanted.len(), 6);
    // Export reads them before its first frame; the preview in the background, as the session asks for them.
    let export = Arc::new(CursorTracks::default());
    export.load_all(&wanted);
    let preview = Arc::new(CursorTracks::default());
    preview.want(&wanted);
    loaded(&preview, &wanted).await;
    for tracks in [&export, &preview] {
        let ctx = planned(&p, tracks);
        // One camera run per take, each framing its own click where that take's cursor rests.
        let runs = ctx.zoom.cameras();
        assert_eq!(runs.len(), 6);
        for (k, (_, run)) in (1..=6).zip(runs) {
            let (x, y) = support::recording::take_cursor(k);
            assert_eq!(run.start_us, support::secs((k - 1) as f64 * 10.0 + 1.0));
            let at = support::secs((k - 1) as f64 * 10.0 + 5.0);
            let w = ctx
                .zoom
                .window(at)
                .unwrap_or_else(|| panic!("take {k} not zoomed"));
            assert!(in_safe_zone(w, x.into(), y.into()), "take {k}: {w:?}");
        }
    }
}

#[test]
fn a_cursor_path_that_isnt_a_regular_file_is_refused_at_once() {
    // A crafted project naming a device (or a pipe) never hangs a preview, an export or chat.
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(load_track(std::path::Path::new("/dev/zero")).is_err());
    });
    let refused = wait
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("still reading /dev/zero after 5 s");
    assert!(refused);
}

#[tokio::test]
async fn a_track_that_couldnt_be_read_is_tried_again() {
    // A drive mounted after the project opened: the next request reads it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cursor.jsonl");
    let path_str = path.to_str().unwrap();
    let tracks = CursorTracks::default();
    let wake = tracks.wake();
    tracks.want(&[only(path_str)]);
    tokio::time::timeout(std::time::Duration::from_secs(10), wake.notified())
        .await
        .unwrap();
    assert!(tracks.get(path_str).is_none());
    std::fs::write(&path, "{\"k\":\"m\",\"t\":0,\"x\":0.3,\"y\":0.3}\n").unwrap();
    tracks.want(&[only(path_str)]);
    tokio::time::timeout(std::time::Duration::from_secs(5), wake.notified())
        .await
        .expect("tried again");
    assert_eq!(tracks.get(path_str).map(|t| t.track.len()), Some(1));
}

#[tokio::test]
async fn past_the_byte_budget_only_the_later_takes_stay_centred_in_preview_and_export_alike() {
    // A budget that holds three of the six takes' tracks: takes 1–3 follow, 4–6 look at the centre, in both.
    let dir = tempfile::tempdir().unwrap();
    let p = support::recording::auto_zoom_takes(6, dir.path());
    let wanted = wanted(&p);
    let one = load_take(std::path::Path::new(&wanted[0].path))
        .unwrap()
        .bytes();
    let budget = 3 * one + one / 2;
    let export = Arc::new(CursorTracks::with_budget(budget));
    export.load_all(&wanted);
    let preview = Arc::new(CursorTracks::with_budget(budget));
    preview.want(&wanted);
    // The loader stops once the tracks it holds pass the budget: the first four load, the last two never do.
    loaded(&preview, &wanted[..4]).await;
    for tracks in [&export, &preview] {
        let ctx = planned(&p, tracks);
        let planned: Vec<i64> = ctx.zoom.cameras().iter().map(|(_, r)| r.start_us).collect();
        assert_eq!(
            planned,
            [1.0, 11.0, 21.0].map(support::secs),
            "takes 1–3 plan their cameras"
        );
        let centred = zoom_items(&p, tracks);
        assert_eq!(
            centred.len(),
            3,
            "takes 4–6 look at the centre: {centred:?}"
        );
        assert!(wanted[4..].iter().all(|w| tracks.get(&w.path).is_none()));
    }
}
