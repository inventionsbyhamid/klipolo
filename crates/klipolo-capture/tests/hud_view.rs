//! The HUD: hidden by default, a 2 s flash on the take's events, at most two redraws a second, and the
//! countdown only when the person's start key began the take; and the core's status carrying what it shows.

mod common;

use std::time::{Duration, Instant};

use klipolo_capture::shell::{Flash, Level, StatusView, WorkerState};
use klipolo_capture::shell_view::{HudFrame, HudView, Words};

fn view(state: WorkerState, elapsed_s: i64) -> StatusView {
    StatusView {
        state,
        elapsed_us: elapsed_s * 1_000_000,
        lines: vec![],
        level: Level::Ok,
        countdown_s: None,
        mic_dbfs: Some(-30.0),
        held: None,
        by_key: false,
        bubble: false,
    }
}

fn shown(text: &str, big: bool) -> Option<HudFrame> {
    Some(HudFrame::Shown {
        text: text.into(),
        meter: Some(0.5),
        big,
    })
}

#[test]
fn a_flash_shows_for_two_seconds_and_the_hud_redraws_at_most_twice_a_second() {
    let (w, t0) = (Words::default(), Instant::now());
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut hud = HudView::default();
    hud.status(view(WorkerState::Recording, 5), at(0));
    assert_eq!(hud.frame(&w, at(0)), None, "hidden by default");
    hud.flash(Flash::Marker, at(0));
    assert_eq!(hud.frame(&w, at(0)), shown("⚑ 0:05", false));
    // A second's status 300 ms later is held back to the half second.
    hud.status(view(WorkerState::Recording, 6), at(300));
    assert_eq!(hud.frame(&w, at(300)), None);
    assert_eq!(hud.next_due(), Some(at(500)));
    assert_eq!(hud.frame(&w, at(500)), shown("⚑ 0:06", false));
    assert_eq!(hud.next_due(), Some(at(2_000)));
    assert_eq!(hud.frame(&w, at(2_000)), Some(HudFrame::Hidden));
    assert_eq!(hud.next_due(), None, "nothing more to draw");
    // A pause flashes with the take's state, worded by the session (the pill's "● REC" is a code).
    hud.status(view(WorkerState::Paused, 6), at(2_100));
    hud.flash(Flash::Paused, at(2_100));
    assert_eq!(hud.frame(&w, at(2_100)), shown("❚❚ 0:06", false));
    hud.status(view(WorkerState::Recording, 6), at(2_700));
    hud.flash(Flash::Resumed, at(2_700));
    assert_eq!(hud.frame(&w, at(2_700)), shown("● REC 0:06", false));
}

#[test]
fn the_countdown_shows_only_when_the_person_s_start_key_began_the_take() {
    let (w, t0) = (Words::default(), Instant::now());
    let at = |s: u64| t0 + Duration::from_secs(s);
    let counting = |left| StatusView {
        countdown_s: Some(left),
        ..view(WorkerState::Countdown, 0)
    };
    // Record in the app: the recorder screen shows the countdown, the HUD stays hidden.
    let mut hud = HudView::default();
    hud.status(view(WorkerState::Armed, 0), at(0));
    hud.status(counting(3), at(1));
    assert_eq!(hud.frame(&w, at(1)), None);
    // ⌥⌘R in the game: the seconds, large, then the take beginning flashes.
    let mut hud = HudView::default();
    hud.status(view(WorkerState::Armed, 0), at(0));
    hud.start_key();
    hud.status(counting(3), at(1));
    assert_eq!(hud.frame(&w, at(1)), shown("3", true));
    hud.status(counting(2), at(2));
    assert_eq!(hud.frame(&w, at(2)), shown("2", true));
    hud.status(view(WorkerState::Recording, 0), at(4));
    assert_eq!(hud.frame(&w, at(4)), shown("● REC 0:00", false));
    assert_eq!(hud.frame(&w, at(6)), Some(HudFrame::Hidden));
}

/// A countdown the start key began that is held (a muted mic, a source not sending) says why, never a digit that has
/// stopped; a problem flashing while it runs marks the seconds.
#[test]
fn a_held_countdown_shows_why_and_a_problem_marks_the_seconds() {
    let (w, t0) = (Words::default(), Instant::now());
    let at = |s: u64| t0 + Duration::from_secs(s);
    let counting = |left, held: Option<&str>| StatusView {
        countdown_s: Some(left),
        held: held.map(str::to_owned),
        ..view(WorkerState::Countdown, 0)
    };
    let mut hud = HudView::default();
    hud.status(view(WorkerState::Armed, 0), at(0));
    hud.start_key();
    hud.status(counting(3, None), at(1));
    assert_eq!(hud.frame(&w, at(1)), shown("3", true));
    let muted = "Your microphone is sending no sound. It may be muted or blocked.";
    hud.status(counting(2, Some(muted)), at(2));
    hud.flash(Flash::Problem, at(2));
    assert_eq!(hud.frame(&w, at(2)), shown(&format!("⚠ {muted}"), false));
    assert_eq!(
        hud.frame(&w, at(9)),
        None,
        "still held after the flash: the reason stays"
    );
    // Free to run again, then low space flashes: the seconds go on, marked.
    hud.status(counting(2, None), at(10));
    hud.flash(Flash::LowSpace, at(10));
    assert_eq!(hud.frame(&w, at(10)), shown("⚠ 2", true));
    assert_eq!(hud.frame(&w, at(12)), shown("2", true));
}

/// In a take the start key began (its editor is hidden), the held countdown's question says how to
/// answer it from the game, in the shell's words: the start key, or the item's Cancel.
#[test]
fn a_held_countdown_says_how_to_answer_it_from_the_game() {
    use klipolo_capture::backend::IndicatorPlace;
    use klipolo_capture::shell_view::gate_answer;
    let (w, t0) = (Words::default(), Instant::now());
    let at = |s: u64| t0 + Duration::from_secs(s);
    let answer = gate_answer(&w, "⌥⌘R", IndicatorPlace::MenuBar);
    assert_eq!(
        answer,
        "Press ⌥⌘R to record without it, or Cancel from the menu bar."
    );
    let waiting = "Microphone isn't sending anything yet.";
    let held = StatusView {
        countdown_s: Some(2),
        held: Some(waiting.into()),
        ..view(WorkerState::Countdown, 0)
    };
    let mut hud = HudView::default();
    hud.answer(answer.clone());
    hud.status(view(WorkerState::Armed, 0), at(0));
    hud.start_key();
    hud.status(held, at(1));
    assert_eq!(
        hud.frame(&w, at(1)),
        shown(&format!("⚠ {waiting} {answer}"), false)
    );
}

/// The core's status carries the countdown's seconds while it runs and the mic's level while counting down or
/// recording, and neither once paused.
#[cfg(feature = "synthetic")]
#[test]
fn the_status_carries_the_countdown_and_the_mic_s_level() {
    use common::harness::{Harness, Src, cfg};
    use klipolo_capture::protocol::Command;
    use klipolo_capture::shell::ShellOutput;
    use klipolo_capture::synthetic::Signal;
    let statuses = |h: &Harness| -> Vec<StatusView> {
        (h.shell_out.iter())
            .filter_map(|o| match o {
                ShellOutput::Status(v) => Some(v.clone()),
                _ => None,
            })
            .collect()
    };
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.start_recording();
    let seconds: Vec<u32> = (statuses(&h).iter())
        .filter_map(|v| v.countdown_s.filter(|_| v.state == WorkerState::Countdown))
        .collect();
    assert_eq!(seconds.first(), Some(&3), "{seconds:?}");
    assert!(seconds.contains(&1), "{seconds:?}");
    h.shell_out.clear();
    h.set_signal(
        "mic",
        Signal::Tone {
            hz: 440.0,
            dbfs: -20.0,
        },
    );
    h.feed_for(3.0);
    let last = statuses(&h).pop().expect("a status a second");
    let level = last.mic_dbfs.expect("the mic's level while recording");
    // The −20 dBFS tone, or the synthetic mic's louder blip each second.
    assert!((-20.5..=0.0).contains(&level), "{level} dBFS");
    assert_eq!(last.countdown_s, None);
    h.command(Command::Pause);
    let paused = statuses(&h).pop().unwrap();
    assert_eq!((paused.state, paused.mic_dbfs), (WorkerState::Paused, None));
}

/// The core's status says why a countdown is held: the mic rule's note while the mic sends nothing, the gate's
/// "isn't sending anything yet" for a source with no frame; and nothing once it runs again.
#[cfg(feature = "synthetic")]
#[test]
fn the_status_says_why_the_countdown_is_held() {
    use common::harness::{Harness, Src, cfg};
    use klipolo_capture::protocol::Command;
    use klipolo_capture::shell::ShellOutput;
    let held = |h: &Harness| -> Vec<Option<String>> {
        (h.shell_out.iter())
            .filter_map(|o| match o {
                ShellOutput::Status(v) if v.state == WorkerState::Countdown => Some(v.held.clone()),
                _ => None,
            })
            .collect()
    };
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.mic_signal_zeros();
    h.command(Command::Start);
    h.feed(0.0, 6.0);
    let muted = "Your microphone is sending no sound. It may be muted or blocked.";
    assert_eq!(
        held(&h).last(),
        Some(&Some(muted.to_owned())),
        "{:?}",
        held(&h)
    );
    assert_eq!(held(&h).first(), Some(&None), "not held while it ran");

    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.first_frame_after("screen", 5.0);
    h.command(Command::Start);
    h.feed(0.0, 4.5);
    let waiting = held(&h).last().cloned().flatten();
    assert_eq!(
        waiting.as_deref(),
        Some("Screen isn't sending anything yet.")
    );
    h.feed(4.5, 9.0);
    assert_eq!(
        held(&h).last(),
        Some(&None),
        "running again once the screen sends"
    );
}
