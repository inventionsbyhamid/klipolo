//! What the pass does with a screen it can't read: a
//! minute whose pictures can't be read is passed over and the minutes after it are made (a later pass makes it), and
//! the editor holds them as export does; a drive gone mid-pass ends the pass at once; a file that can't be read now is
//! never marked; one marked as a file that can't be analysed (an export couldn't) is planned without by every store
//! sharing the folder, the editor's too.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use klipolo_engine::media::activity::ActivityStore;
use klipolo_engine::media::cursor::Screens;

use super::synthetic::{
    Log, Screen, blank_opener, damaged_opener, opener, unavailable_opener, vanishing_opener,
};
use super::{S, activity_events, file, never, screen_file, stores, wait_until};

const SCREEN: Screen = Screen {
    duration_us: 180 * S,
    per_frame: Duration::ZERO,
};

#[test]
fn an_unreadable_minute_is_passed_over_and_the_rest_are_made() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let log = Arc::new(Log::default());
    let damaged = damaged_opener(SCREEN, log.clone(), (70 * S, 71 * S));
    let (_t, store) = stores(dir.path(), Some(damaged));
    let track = store.ensure(&video, &never(), &|_| {}).unwrap();
    // Minutes 0 and 2 are made; minute 1 is left for a later pass (it may read another time: a drive back).
    assert!(!track.is_complete());
    assert_eq!(track.complete_through(), 60 * S);
    assert!(track.tick_at(150 * S).is_some() && track.tick_at(65 * S).is_none());
    assert_eq!(log.opened_at(), vec![0, 120 * S]);
    assert!(file(dir.path(), &video, true).exists() && !file(dir.path(), &video, false).exists());
    // The next pass, the minute readable now, makes it alone.
    let log = Arc::new(Log::default());
    let (_t, next) = stores(dir.path(), Some(opener(SCREEN, log.clone())));
    let whole = next.ensure(&video, &never(), &|_| {}).unwrap();
    assert!(whole.is_complete());
    assert_eq!(log.opened_at(), vec![60 * S]);
    assert_eq!(log.frames(), 601);
}

#[test]
fn a_file_an_export_couldnt_analyse_is_planned_without_by_the_editor_too() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    // The editor holds its first minute (its pass stopped there).
    let log = Arc::new(Log::default());
    let (thumbs, editor) = stores(dir.path(), Some(opener(SCREEN, log.clone())));
    let events = activity_events(&thumbs);
    let cancel = never();
    let _ = editor.ensure(&video, &cancel, &|done| {
        if done > 0.3 {
            cancel.store(true, Ordering::SeqCst);
        }
    });
    assert!(editor.get(&video).is_some());
    let seen = *events.lock().unwrap();
    // An export (another process: its own store on the folder) found it can't be analysed.
    let opens = Arc::new(AtomicUsize::new(0));
    let export = ActivityStore::detached(
        dir.path().join("activity"),
        blank_opener(180 * S, opens.clone()),
    );
    export.fail(&video);
    assert!(export.ensure(&video, &never(), &|_| {}).is_err());
    assert_eq!(opens.load(Ordering::SeqCst), 0, "never tried again");
    // The editor lets its track go when it next looks at the file, says so, and decodes nothing more.
    editor.want(std::slice::from_ref(&video));
    wait_until("the editor plans without it", || {
        editor.get(&video).is_none() && !editor.pending()
    });
    assert!(*events.lock().unwrap() > seen, "said");
    assert_eq!(log.opened_at(), vec![0]);
}

/// The editor held minute 0 when an export passed over minute 1 and made minute 2. Its next
/// look holds minutes 0 and 2 as well, so the preview plans with the same activity as the exported file.
#[test]
fn the_editor_holds_the_minutes_an_export_made_past_one_it_couldnt_read() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let bad = (70 * S, 71 * S);
    let (_t, editor) = stores(
        dir.path(),
        Some(damaged_opener(SCREEN, Arc::default(), bad)),
    );
    let cancel = never();
    let _ = editor.ensure(&video, &cancel, &|done| {
        if done > 0.3 {
            cancel.store(true, Ordering::SeqCst);
        }
    });
    assert_eq!(
        editor.get(&video).map(|t| t.complete_through()),
        Some(60 * S)
    );
    let export = ActivityStore::detached(
        dir.path().join("activity"),
        damaged_opener(SCREEN, Arc::default(), bad),
    );
    let exported = export.ensure(&video, &never(), &|_| {}).unwrap();
    assert!(exported.tick_at(150 * S).is_some());
    editor.want(std::slice::from_ref(&video));
    wait_until("the editor's pass ends", || !editor.pending());
    let held = editor.get(&video).expect("held");
    assert!(held.tick_at(150 * S).is_some(), "minute 2 held");
    let print = |track| {
        let of: klipolo_engine::media::cursor::ActivityOf =
            Arc::new(move |_: &str| Some(Arc::clone(&track)));
        Screens::read(&[(0, video.clone())], Some(&of)).print
    };
    assert_eq!(print(held), print(exported));
}

/// A screen that can't be read now (an OS error) is never marked by the editor's pass: it gives up
/// for this opening only, and the next opening analyses it.
#[test]
fn a_screen_that_cant_be_read_now_is_never_marked() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let opens = Arc::new(AtomicUsize::new(0));
    let (_t, editor) = stores(dir.path(), Some(unavailable_opener(opens.clone())));
    editor.want(std::slice::from_ref(&video));
    wait_until("the editor gives up for now", || {
        !editor.pending() && opens.load(Ordering::SeqCst) >= 3
    });
    let mark = file(dir.path(), &video, false).with_extension("activity.failed");
    assert!(!mark.exists(), "never marked");
    let (_t, next) = stores(dir.path(), Some(opener(SCREEN, Arc::default())));
    assert!(
        next.ensure(&video, &never(), &|_| {})
            .unwrap()
            .is_complete()
    );
}

/// A drive gone mid-pass (its pictures fail, then no decoder opens) ends the pass after one try
/// at the next minute, not one try and one warning a minute left.
#[test]
fn a_drive_gone_mid_pass_ends_the_pass() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let log = Arc::new(Log::default());
    let long = Screen {
        duration_us: 300 * S,
        per_frame: Duration::ZERO,
    };
    let (_t, store) = stores(
        dir.path(),
        Some(vanishing_opener(long, log.clone(), 70 * S)),
    );
    let _ = store.ensure(&video, &never(), &|_| {});
    assert_eq!(
        log.opened_at(),
        vec![0, 120 * S],
        "one try after the drive went"
    );
    assert_eq!(
        store.get(&video).map(|t| t.complete_through()),
        Some(60 * S),
        "minute 0 kept"
    );
}
