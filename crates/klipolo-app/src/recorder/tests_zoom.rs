//! A new take's auto zoom: a take with clicks lands as "Add recording", then "Add auto zoom" as a step of its own, its
//! items attached to the take's clip; a take without clicks, or whose cursor file can't be read, lands with no zoom
//! step; zooms are never added twice and never over a zoom already there; and the engine plays every click zoomed in.
//! Real fixture media and cursor files; nothing captures.

use std::sync::{Arc, Mutex};

use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::session::{ImportTarget, SessionDir};
use klipolo_capture::shell::WorkerState::*;
use klipolo_cursor::format::{Capture, Line, write_line};
use klipolo_cursor::{ClickSource, Us};
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::plan::zoom::ZoomTrack;
use klipolo_model::Op;
use klipolo_model::coded::CodedError;
use klipolo_model::cursor_style::FocusKind;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::ops::TrackTarget;
use klipolo_model::project::Project;
use klipolo_model::timeline::{ClipSource, Item, ItemContent, TrackKind, ZoomTarget};

use super::state::Opened;
use super::test_support::*;
use super::test_takes::*;
use super::types::TakeImport;
use super::zoom::zoom_take;
use crate::project::{build_and_apply, step, with_open};

const S: Us = 1_000_000;
/// The take's length.
const LENGTH_US: Us = 20 * S;
/// Its clicks (seconds into the take, x, y): two close together at the top left, then two more 8 s later at the bottom
/// right.
const CLICKS_AT: [(f64, f32, f32); 4] = [
    (2.0, 0.30, 0.40),
    (2.8, 0.32, 0.42),
    (10.0, 0.80, 0.70),
    (10.5, 0.82, 0.72),
];
/// Where the take's clip starts in `Sessions::with_project`: after its 1 s clip.
const CLIP_START: Us = S;

fn us(seconds: f64) -> Us {
    (seconds * 1e6).round() as Us
}

/// The take's cursor file: a 1440 × 900 screen at 30 Hz, the pointer reaching each of `CLICKS_AT`'s points 0.4 s
/// before its time and leaving 0.1 s after, gliding on to the next, and moving back and forth after the last (so it never
/// rests into a pause). With `clicks`, a primary press and release at each.
fn cursor_file(clicks: bool) -> Vec<u8> {
    let keys: Vec<(f64, f32, f32)> = (CLICKS_AT.iter())
        .flat_map(|&(t, x, y)| [(t - 0.4, x, y), (t + 0.1, x, y)])
        .collect();
    let at = |s: f64| {
        let i = keys.partition_point(|k| k.0 <= s);
        match (i.checked_sub(1).map(|i| keys[i]), keys.get(i)) {
            (Some(a), Some(b)) => {
                let u = ((s - a.0) / (b.0 - a.0)) as f32;
                (a.1 + (b.1 - a.1) * u, a.2 + (b.2 - a.2) * u)
            }
            (Some(a), None) => {
                let phase = ((s - a.0) * 2.0).fract() as f32;
                (a.1 + 0.1 * (1.0 - (2.0 * phase - 1.0).abs()), a.2)
            }
            (None, Some(b)) => (b.1, b.2),
            (None, None) => (0.5, 0.5),
        }
    };
    let mut lines: Vec<(Us, Line)> = (0..LENGTH_US / 33_333)
        .map(|i| {
            let t = i * 33_333;
            let (x, y) = at(t as f64 / 1e6);
            (t, Line::Move { t, x, y })
        })
        .collect();
    if clicks {
        for &(s, x, y) in &CLICKS_AT {
            for (t, d) in [(us(s), 1), (us(s) + 50_000, 0)] {
                let press = Line::Button { t, btn: 0, d, x, y };
                lines.push((t, press));
            }
        }
    }
    lines.sort_by_key(|(t, _)| *t);
    let mut bytes = Vec::new();
    let header = Line::Header {
        v: 2,
        capture: Capture::Separate,
        clicks: ClickSource::Events,
        w: 1440.0,
        h: 900.0,
    };
    write_line(&header, &mut bytes);
    for (_, line) in &lines {
        write_line(line, &mut bytes);
    }
    bytes
}

/// A 20 s take with a cursor file.
fn recorded() -> Recorded {
    Recorded {
        cursor: true,
        length_us: LENGTH_US,
        ..Recorded::default()
    }
}

/// The take in `dir` saved, its cursor file clicking when `clicks` says.
fn save(dir: &SessionDir, clicks: bool) {
    write_saved(dir, recorded());
    std::fs::write(dir.cursor(), cursor_file(clicks)).unwrap();
}

/// A saved 20 s take armed in `target`, its cursor file clicking when `clicks` says.
fn take(target: ImportTarget, clicks: bool) -> TakeFixture {
    let take = TakeFixture::new(recorded(), target);
    save(&take.dir, clicks);
    take
}

/// The open project, as its take's target.
fn armed_in(sessions: &Sessions) -> ImportTarget {
    ImportTarget::Project {
        id: sessions.project().id,
    }
}

/// `take` landed in `sessions`' open project.
fn land(sessions: &Sessions, take: &TakeFixture) -> TakeImport {
    sessions
        .land(&take.done(false, sessions.open_file()), None)
        .unwrap()
}

/// Every zoom item, by start.
fn zoom_items(project: &Project) -> Vec<Item> {
    let mut items: Vec<Item> = (project.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Zoom)
        .flat_map(|t| t.items.clone())
        .collect();
    items.sort_by_key(|i| i.start_us);
    items
}

fn undo_label(sessions: &Sessions) -> Option<String> {
    sessions.history().undo_label
}

fn redo(sessions: &Sessions) {
    with_open(&sessions.state, |open| step(open, true)).unwrap();
}

/// A take with clicks: "Add recording", then "Add auto zoom" with 2 items. One Undo takes back only the zooms, a second
/// the recording; Redo brings each back as it was.
#[test]
fn take_with_clicks_gets_its_own_auto_zoom_step() {
    let sessions = Sessions::with_project();
    let before = sessions.project();
    let take = take(armed_in(&sessions), true);
    let import = land(&sessions, &take);
    let zoomed = sessions.project();
    let zooms = zoom_items(&zoomed);
    assert_eq!(zooms.len(), 2, "{zooms:?}");
    assert_eq!(import.zooms, 2);
    assert_eq!(undo_label(&sessions).as_deref(), Some("undo.autoZoom"));

    sessions.undo();
    let recorded = sessions.project();
    assert!(zoom_items(&recorded).is_empty(), "the zooms are gone");
    assert_eq!(recorded.recordings.len(), 1, "the recording stays");
    assert_eq!(recorded.timeline.main.len(), 2);
    assert_eq!(undo_label(&sessions).as_deref(), Some("undo.record"));
    redo(&sessions);
    assert_eq!(zoom_items(&sessions.project()), zooms, "Redo returns them");

    sessions.undo();
    sessions.undo();
    let undone = sessions.project();
    assert!(undone.recordings.is_empty());
    assert_eq!(undone.timeline.main, before.timeline.main);
    redo(&sessions);
    redo(&sessions);
    assert_eq!(sessions.project(), zoomed);
}

/// A take whose pointer moves but never clicks, or that kept no cursor file, lands as "Add recording" alone.
#[test]
fn take_without_clicks_gets_no_zoom_step() {
    let sessions = Sessions::with_project();
    let take = take(armed_in(&sessions), false);
    let import = land(&sessions, &take);
    assert_eq!(import.zooms, 0);
    assert!(zoom_items(&sessions.project()).is_empty());
    assert_eq!(undo_label(&sessions).as_deref(), Some("undo.record"));
    sessions.undo();
    assert!(sessions.project().recordings.is_empty(), "one step only");

    let sessions = Sessions::with_project();
    let take = TakeFixture::saved(Recorded::default(), armed_in(&sessions));
    let import = land(&sessions, &take);
    assert_eq!(import.zooms, 0);
    assert_eq!(undo_label(&sessions).as_deref(), Some("undo.record"));
}

/// Each zoom is an Auto zoom on the take's clicks at 2x, over the whole frame, attached to the take's clip and inside
/// it, the first over the first two clicks and the second over the last two.
#[test]
fn zooms_are_attached_to_the_recording_clip() {
    let sessions = Sessions::with_project();
    let take = take(armed_in(&sessions), true);
    let import = land(&sessions, &take);
    let zooms = zoom_items(&sessions.project());
    assert_eq!(zooms.len(), 2);
    let ids: Vec<&str> = zooms.iter().map(|z| z.id.as_str()).collect();
    assert!(ids[0] != ids[1] && !ids[0].is_empty(), "{ids:?}");
    for zoom in &zooms {
        let attach = zoom.attach.as_ref().expect("attached");
        assert_eq!(attach.clip_id, import.clip_id);
        assert_eq!(
            attach.source,
            ClipSource::Recording {
                id: "take-1".into()
            }
        );
        assert!(
            zoom.start_us >= CLIP_START && zoom.end_us() <= CLIP_START + LENGTH_US,
            "{zoom:?} inside the clip"
        );
        assert_eq!(
            zoom.content,
            ItemContent::Zoom {
                target: ZoomTarget::Auto {
                    using: vec![FocusKind::Clicks]
                },
                scale: 2.0,
            }
        );
        assert_eq!(zoom.layout, Layout::new(Anchor::Center, BoxN::FULL));
        assert!(zoom.effects.is_empty());
    }
    for (k, &(s, _, _)) in CLICKS_AT.iter().enumerate() {
        let zoom = &zooms[k / 2];
        let at = CLIP_START + us(s);
        assert!(
            (zoom.start_us..zoom.end_us()).contains(&at),
            "the click at {at} is in {zoom:?}"
        );
    }
}

/// What a test does to a take's cursor file.
type Spoil = fn(&std::path::Path);

/// A cursor file that is gone, is a folder, or holds no cursor line: the take lands with no zoom step and no error.
#[test]
fn unreadable_cursor_file_lands_the_take_without_zooms() {
    let spoil: [(&str, Spoil); 3] = [
        ("gone", |file| std::fs::remove_file(file).unwrap()),
        ("a folder", |file| {
            std::fs::remove_file(file).unwrap();
            std::fs::create_dir(file).unwrap();
        }),
        ("damaged", |file| {
            std::fs::write(file, b"\0\0\0 not a cursor line\n{\"k\":").unwrap()
        }),
    ];
    for (what, spoil) in spoil {
        let sessions = Sessions::with_project();
        let take = take(armed_in(&sessions), true);
        spoil(&take.dir.cursor());
        let import = land(&sessions, &take);
        assert_eq!(import.zooms, 0, "{what}");
        let project = sessions.project();
        assert_eq!(project.recordings.len(), 1, "{what}: landed");
        assert!(project.recordings[0].cursor.is_some(), "{what}");
        assert!(zoom_items(&project).is_empty(), "{what}");
        assert_eq!(undo_label(&sessions).as_deref(), Some("undo.record"));
    }
}

/// What the page hears (`recorder-imported`) after a take through the recorder: how many zooms were added, and its
/// snapshot holds them with "Add auto zoom" on top; none for a take without clicks.
#[test]
fn imported_event_counts_the_zooms() {
    for (clicks, zooms, top) in [(true, 2, "undo.autoZoom"), (false, 0, "undo.record")] {
        let sessions = Arc::new(Sessions::with_project());
        let rec = fixture();
        *rec.project.lock().unwrap() = Some(Opened {
            id: sessions.project().id,
            file: sessions.open_file().unwrap(),
        });
        let landed: Arc<Mutex<Vec<Result<TakeImport, CodedError>>>> = Arc::default();
        *rec.sink.lands.lock().unwrap() = Some(landing_into(sessions.clone(), landed.clone()));
        let id = rec.arm(options_screen()).unwrap().id;
        rec.send(Command::Start).unwrap();
        rec.launch.emit(state(Recording));
        save(&SessionDir::at(&rec.take_dir(&id)), clicks);
        rec.send(Command::Stop).unwrap();
        rec.launch.emit(state(Stopping));
        rec.launch.emit(Event::Finished {
            session: id.clone(),
        });
        rec.launch.exit(Some(0));
        rec.settle();
        let import = landed.lock().unwrap()[0].clone().unwrap();
        assert_eq!(import.take, id);
        assert_eq!(import.zooms, zooms);
        let project = &import.snapshot.project;
        assert_eq!(zoom_items(project).len(), zooms as usize, "in its snapshot");
        assert_eq!(import.snapshot.history.undo_label.as_deref(), Some(top));
        assert_eq!(*project, sessions.project(), "the project as it is now");
    }
}

/// A take landed again (it was in its project already) adds no zoom step, even after the person took its zooms back.
#[test]
fn a_take_landed_again_gets_no_second_zoom_step() {
    let sessions = Sessions::with_project();
    let take = take(armed_in(&sessions), true);
    land(&sessions, &take);
    let revision = sessions.history().revision;
    let again = land(&sessions, &take);
    assert_eq!(again.zooms, 0);
    assert_eq!(zoom_items(&sessions.project()).len(), 2);
    assert_eq!(sessions.history().revision, revision, "no step");

    sessions.undo();
    let revision = sessions.history().revision;
    let again = land(&sessions, &take);
    assert_eq!(again.zooms, 0);
    assert!(
        zoom_items(&sessions.project()).is_empty(),
        "still taken back"
    );
    assert_eq!(sessions.history().revision, revision, "no step");
}

/// The zoom step goes only into the take's own project: with another open by then, it is refused and changes nothing.
#[test]
fn zooms_never_land_in_another_project() {
    let sessions = Sessions::with_project();
    let armed = sessions.project().id;
    let take = take(armed_in(&sessions), true);
    let import = land(&sessions, &take);
    sessions.open_new("Other");
    let other = sessions.project();
    let refused = zoom_take(&sessions.state, &armed, "take-1", &import.clip_id).unwrap_err();
    assert_eq!(code_of(&refused), "project.moved");
    assert_eq!(sessions.project(), other);
}

/// A zoom already there over part of the take (the person's own, from 10 s to 12 s on the timeline) keeps its place,
/// and the take's zoom that would overlap it is left out.
#[test]
fn a_zoom_already_there_is_left_alone() {
    let sessions = Sessions::with_project();
    let mine = Item {
        id: "mine".into(),
        start_us: 10 * S,
        duration_us: 2 * S,
        attach: None,
        layout: Layout::new(Anchor::Center, BoxN::FULL),
        content: ItemContent::Zoom {
            target: ZoomTarget::Rect {
                rect: BoxN {
                    x: 0.25,
                    y: 0.25,
                    w: 0.5,
                    h: 0.5,
                },
            },
            scale: 2.0,
        },
        effects: Vec::new(),
    };
    let add = Op::AddItem {
        track: TrackTarget::Auto,
        attach: false,
        item: mine.clone(),
    };
    build_and_apply(&sessions.state, None, None, "test.zoom", |_| Ok(vec![add])).unwrap();
    let take = take(armed_in(&sessions), true);
    let import = land(&sessions, &take);
    assert_eq!(import.zooms, 1);
    let zooms = zoom_items(&sessions.project());
    assert_eq!(zooms.len(), 2);
    assert_eq!(zooms[1], mine, "the person's zoom as it was");
    assert!(zooms[0].end_us() <= mine.start_us);
    assert!((zooms[0].start_us..zooms[0].end_us()).contains(&(CLIP_START + 2 * S)));
}

/// A take from the start screen lands in a new project with its zooms: two steps, the project's only ones.
#[test]
fn a_take_from_the_start_screen_gets_its_zooms_too() {
    let sessions = Sessions::start_screen();
    let take = take(ImportTarget::New, true);
    let import = sessions.land(&take.done(false, None), None).unwrap();
    assert_eq!(import.zooms, 2);
    assert_eq!(zoom_items(&sessions.project()).len(), 2);
    for zoom in zoom_items(&sessions.project()) {
        let attach = zoom.attach.expect("attached");
        assert_eq!(attach.clip_id, import.clip_id);
    }
    sessions.undo();
    assert!(zoom_items(&sessions.project()).is_empty());
    assert_eq!(sessions.project().timeline.main.len(), 1);
    sessions.undo();
    assert!(sessions.project().timeline.main.is_empty());
    assert!(!sessions.history().can_undo);
}

/// Should the person close the take's project, or open another, right after its step and before its zooms, the project
/// opens again first and the zooms land there: never lost.
#[test]
fn zooms_land_even_when_the_project_closes_right_after_the_take() {
    for close in [true, false] {
        let sessions = Sessions::with_project();
        let take = take(armed_in(&sessions), true);
        let done = take.done(false, sessions.open_file());
        let library = || -> Result<std::path::PathBuf, klipolo_model::coded::Said> {
            Err(klipolo_model::coded::Said::code("project.noVideosFolder"))
        };
        let mut probe = |path: &std::path::Path, _key: &str| crate::media::probe_asset(path);
        let to = super::destination::Destination {
            target: &done.target,
            file: done.project_file.as_deref(),
            library: &library,
            today: OCT_31,
        };
        let landed =
            super::import::import_take(&sessions.state, &take.dir, &to, &mut probe).unwrap();
        if close {
            crate::project::close_in(&sessions.state).unwrap();
        } else {
            sessions.open_new("Other");
        }
        let import = super::import::after_landing(&sessions.state, &done, landed).unwrap();
        assert_eq!(import.zooms, 2, "closed: {close}");
        assert_eq!(
            zoom_items(&import.snapshot.project).len(),
            2,
            "in its snapshot"
        );
        assert_eq!(
            import.snapshot.history.undo_label.as_deref(),
            Some("undo.autoZoom")
        );
        assert_eq!(
            sessions.project().id,
            import.snapshot.project.id,
            "open again"
        );
        assert_eq!(zoom_items(&sessions.project()).len(), 2);
    }
}

/// (x, y) inside `w`'s safe zone: 15 % of its width in from each side, a side on the frame's edge reaching it.
fn in_safe_zone(w: BoxN, x: f64, y: f64) -> bool {
    let (m, e) = (0.15 * w.w, 1e-6);
    let lo = |p: f64| if p <= e { 0.0 } else { p + m };
    let hi = |p: f64, s: f64| if p + s >= 1.0 - e { 1.0 } else { p + s - m };
    x >= lo(w.x) - e && x <= hi(w.x, w.w) + e && y >= lo(w.y) - e && y <= hi(w.y, w.h) + e
}

/// The engine plans inside each zoom from the same cursor file, as the preview and export draw it: every click plays
/// zoomed in, inside the view's safe zone, and the view is the whole frame between the two zooms.
#[test]
fn the_engine_plays_every_click_zoomed_in() {
    let sessions = Sessions::with_project();
    let take = take(armed_in(&sessions), true);
    land(&sessions, &take);
    let project = Arc::new(sessions.project());
    let tracks = CursorTracks::default();
    tracks.load_all(&wanted(&project));
    let zoom = ZoomTrack::new(&project, &tracks);
    assert_eq!(zoom.cameras().len(), 2, "one camera run per zoom");
    for &(s, x, y) in &CLICKS_AT {
        let at = CLIP_START + us(s);
        let w = zoom
            .window(at)
            .unwrap_or_else(|| panic!("zoomed in at {at}"));
        assert!(w.w < 1.0, "{w:?}");
        assert!(
            in_safe_zone(w, x.into(), y.into()),
            "the click at {at} is in {w:?}"
        );
    }
    assert_eq!(zoom.window(CLIP_START + 7 * S), None, "between the zooms");
}
