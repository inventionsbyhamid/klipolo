//! A separate cursor is never dropped silently: a take whose frames have no cursor and whose cursor can't be read, or
//! that the budget passes over, stops the export before anything is written and asks, once its footage is known to be
//! there; with a yes it exports without that cursor and says so, once however often it is tried. A run with nothing
//! left to draw never asks, nor one whose parts left never draw that cursor; one whose finished parts already show it
//! says so. Through the runner, with the worker run in this process (`worker.rs`).

use std::path::Path;
use std::sync::Arc;

use klipolo_engine::api::{CodedError, ExportEvent, Said};
use klipolo_engine::export::job::{ExportJob, PartStatus};
use klipolo_engine::export::prepare::load_cursors_into;
use klipolo_engine::export::protocol::ErrorCode;
use klipolo_engine::export::runner::Outcome;
use klipolo_engine::export::shared::checksum;
use klipolo_engine::media::cursor::CursorTracks;
use klipolo_engine::plan::frame_plan;
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::project::Project;

use crate::support::{fixture, secs};
use crate::worker::{batch, export, files, named, resume, the_job};
use crate::{add_take, cursors_of, glide, preview_ctx, preview_cursors, write_take};

/// Two takes of 10 s: take 1 burned in, its cursor file never written, which needs none; take 2 separate, whose cursor
/// file isn't there either. Their footage is in `dir` when `footage` (stand-in files: nothing here decodes them).
fn unreadable_take(dir: &Path, footage: bool) -> Project {
    std::fs::create_dir_all(dir).unwrap();
    let mut p = Project::new("Unreadable cursor");
    p.id = "project".into();
    for (k, capture) in [(1, CursorCapture::Burned), (2, CursorCapture::Separate)] {
        let screen = dir.join(format!("screen{k}.mp4"));
        if footage {
            std::fs::write(&screen, b"footage").unwrap();
        }
        let cursor = dir.join(format!("take{k}/cursor.jsonl"));
        let screen = screen.to_string_lossy();
        add_take(&mut p, k, &screen, 10.0, (&cursor, capture), (0.0, 10.0));
    }
    p
}

#[tokio::test(flavor = "multi_thread")]
async fn a_separate_take_whose_cursor_cant_be_read_stops_the_export_and_asks() {
    let dir = tempfile::tempdir().unwrap();
    // Its footage gone too (a drive unplugged): that is what it says, not the cursor.
    let gone = dir.path().join("gone");
    let (outcome, _) = export(&unreadable_take(&gone, false), &gone, false).await;
    assert!(
        matches!(&outcome, Outcome::Failed(Said::Text(why)) if why.contains("missing")),
        "{outcome:?}"
    );
    // The footage there: the question, naming the recording, before anything is written, so the job holds its
    // manifest, project and caption cues alone (`captions.json`), and it waits for the answer (not retried).
    let (outcome, events) = export(&unreadable_take(dir.path(), true), dir.path(), false).await;
    let asked = Said::Coded(named("exports.cursorMissing", 2));
    assert!(
        matches!(&outcome, Outcome::Failed(said) if *said == asked),
        "{outcome:?}"
    );
    assert!(
        events.contains(&ExportEvent::Failed { message: asked }),
        "{events:?}"
    );
    assert!(!events.contains(&ExportEvent::Retrying));
    assert_eq!(
        files(&the_job(dir.path())),
        ["captions.json", "job.json", "project.json"],
        "nothing written"
    );

    // A separate take the budget passes over can't be drawn either, and the question says why. Here the budget holds
    // take 1's cursor alone.
    let mut p = Project::new("Over the budget");
    for k in 1..=2 {
        let lines = glide((0.2, 0.2), (0.8, 0.8), 0.0, 10.0).collect();
        let take = write_take(dir.path(), k + 2, &[], lines);
        add_take(
            &mut p,
            k,
            "/media/screen",
            10.0,
            (&take, CursorCapture::Separate),
            (0.0, 10.0),
        );
    }
    let budget = || CursorTracks::with_budget(1);
    let loaded = load_cursors_into(budget(), &p);
    let refused = loaded.question().expect("asks");
    assert_eq!(refused.code, ErrorCode::CursorMissing);
    let said: CodedError = serde_json::from_str(&refused.message).unwrap();
    assert_eq!(said, named("exports.cursorOverBudget", 2));
    // Allowed, it draws take 1's cursor and not take 2's, as the preview with that budget does.
    assert_eq!(loaded.notes(), [named("exports.note.cursorOverBudget", 2)]);
    let export = klipolo_engine::plan::PlanContext::with_cursors(
        Arc::new(p.clone()),
        klipolo_engine::registry::Registry::builtin(),
        loaded.cursors,
    );
    let preview = preview_ctx(&p, &preview_cursors(&p, budget()));
    for (t, drawn) in [(secs(5.0), 1), (secs(15.0), 0)] {
        let plan = frame_plan(&export, t, Landscape);
        assert_eq!(cursors_of(&plan).len(), drawn, "at {t} µs");
        assert_eq!(plan, frame_plan(&preview, t, Landscape));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn allowed_it_exports_without_that_cursor_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let p = unreadable_take(dir.path(), true);
    // Each run here stops right after its note, so the runner tries three times (and it is said once).
    let (outcome, events) = export(&p, dir.path(), true).await;
    let note = ExportEvent::Note {
        message: Said::Coded(named("exports.note.cursorDropped", 2)),
    };
    assert_eq!(
        events.iter().filter(|e| **e == note).count(),
        1,
        "{events:?}"
    );
    assert!(events.contains(&ExportEvent::Retrying), "{events:?}");
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    // The job keeps the answer, so a resume doesn't ask again.
    assert!(
        ExportJob::open(&the_job(dir.path()))
            .unwrap()
            .allow_missing_cursor
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_with_nothing_left_to_render_finishes_without_asking() {
    // Every segment and the sound were finished and checked when the app quit; then the drive with the
    // footage and the take's cursor file was unplugged. The resume only joins and delivers, so it must not ask (nor
    // say that a cursor was dropped: every frame was drawn with it).
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone");
    let mut p = Project::new("Rendered");
    p.id = "project".into();
    p.settings.fps = 30;
    let screen = gone.join("screen1.mp4").to_string_lossy().into_owned();
    let cursor = gone.join("take1/cursor.jsonl");
    add_take(
        &mut p,
        1,
        &screen,
        2.0,
        (&cursor, CursorCapture::Separate),
        (0.0, 2.0),
    );
    let out = dir.path().join("Exports");
    std::fs::create_dir_all(&out).unwrap();
    let mut job = ExportJob::create(&batch(&p, &out, false), 2).unwrap();
    assert_eq!(job.outputs[0].segments.len(), 1);
    // Its one segment (2 s at 30 fps) and its sound, as a run finished them: the fixture files, checked by size and
    // CRC like any finished part.
    let segment = job.path(&job.outputs[0].segments[0].file);
    let sound = job.path(&job.audio.file);
    std::fs::copy(fixture("solid-red.mp4"), &segment).unwrap();
    std::fs::copy(fixture("bars-30fps-0.5gop.mp4"), &sound).unwrap();
    let done = |path: &Path| {
        (
            std::fs::metadata(path).unwrap().len(),
            checksum(path).unwrap(),
        )
    };
    let s = &mut job.outputs[0].segments[0];
    (s.bytes, s.checksum) = done(&segment);
    s.status = PartStatus::Done;
    (job.audio.bytes, job.audio.checksum) = done(&sound);
    job.audio.status = PartStatus::Done;
    job.save().unwrap();

    let (outcome, events) = resume(&job.dir).await;
    let Outcome::Finished(files) = &outcome else {
        panic!("{outcome:?}: {events:?}");
    };
    assert!(files.len() == 1 && files[0].is_file(), "{files:?}");
    assert!(events.contains(&ExportEvent::Finished { notes: None }));
    let said = |e: &ExportEvent| matches!(e, ExportEvent::Note { .. } | ExportEvent::Failed { .. });
    assert!(!events.iter().any(said), "{events:?}");
}

/// Takes of `(capture, seconds)` one after the other on the main track, each with its footage in `dir` (stand-in
/// files) and its cursor file never written.
fn takes_in(dir: &Path, takes: &[(CursorCapture, f64)]) -> Project {
    std::fs::create_dir_all(dir).unwrap();
    let mut p = Project::new("Parts left");
    p.id = "project".into();
    p.settings.fps = 30;
    for (k, &(capture, seconds)) in (1..).zip(takes) {
        let screen = dir.join(format!("screen{k}.mp4"));
        std::fs::write(&screen, b"footage").unwrap();
        let cursor = dir.join(format!("take{k}/cursor.jsonl"));
        let screen = screen.to_string_lossy();
        add_take(
            &mut p,
            k,
            &screen,
            seconds,
            (&cursor, capture),
            (0.0, seconds),
        );
    }
    p
}

/// A job exporting `p` into `dir`'s Exports in parts of 2 s, its first part finished as a run left it: a fixture file
/// (2 s at 30 fps), checked by size and CRC like any finished part.
fn first_part_done(p: &Project, dir: &Path) -> ExportJob {
    let out = dir.join("Exports");
    std::fs::create_dir_all(&out).unwrap();
    let mut job = ExportJob::create(&batch(p, &out, false), 2).unwrap();
    assert!(job.outputs[0].segments.len() >= 2, "parts left");
    let file = job.path(&job.outputs[0].segments[0].file);
    std::fs::copy(fixture("solid-red.mp4"), &file).unwrap();
    let s = &mut job.outputs[0].segments[0];
    (s.bytes, s.checksum) = (
        std::fs::metadata(&file).unwrap().len(),
        checksum(&file).unwrap(),
    );
    s.status = PartStatus::Done;
    job.save().unwrap();
    job
}

#[tokio::test(flavor = "multi_thread")]
async fn the_question_covers_only_the_parts_left_to_draw() {
    // Take 1's cursor can't be read, and the one part that draws it is finished: what is left (take 2, burned in)
    // draws no cursor of its own, so the resume neither asks nor says a cursor was dropped.
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    let p = takes_in(
        &media,
        &[(CursorCapture::Separate, 2.0), (CursorCapture::Burned, 2.0)],
    );
    let job = first_part_done(&p, dir.path());
    let (outcome, events) = resume(&job.dir).await;
    let asks = |said: &Said| matches!(said, Said::Coded(c) if c.code.starts_with("exports.cursor"));
    assert!(
        !matches!(&outcome, Outcome::Failed(said) if asks(said)),
        "{outcome:?}"
    );
    let noted = |e: &ExportEvent| matches!(e, ExportEvent::Note { .. });
    assert!(!events.iter().any(noted), "{events:?}");
    // Where a part left draws it, it asks, saying how many finished parts already show it, so the person can start
    // over rather than get a file with the cursor in some parts only.
    let dir = tempfile::tempdir().unwrap();
    let p = takes_in(&dir.path().join("media"), &[(CursorCapture::Separate, 4.0)]);
    let job = first_part_done(&p, dir.path());
    let (outcome, _) = resume(&job.dir).await;
    let mixed = Said::Coded(named("exports.cursorMissing", 1).with("drawnParts", 1usize));
    assert!(
        matches!(&outcome, Outcome::Failed(said) if *said == mixed),
        "{outcome:?}"
    );
}
