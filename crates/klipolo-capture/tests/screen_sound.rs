//! The sound that depends on a screen's capture, in virtual time: an app's or game's sound that
//! comes on its screen's stream ends with a note when the screen is given up, and the computer's sound, which macOS
//! records from a display's stream, waits for a sleeping display to wake rather than spending its restarts.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg};
use klipolo_capture::pipe::PipeEvent;
use klipolo_capture::protocol::Event;
use klipolo_capture::run::core::{Effect, PipeCmd};
use klipolo_capture::session::{SourceConfig, SourceSpec};
use klipolo_capture::shell::WorkerState;
use klipolo_model::project::SourceRole;

fn sent(h: &Harness, source: &str, cmd: PipeCmd) -> usize {
    h.effects()
        .iter()
        .filter(|e| matches!(e, Effect::Pipe { source: s, cmd: c } if s == source && *c == cmd))
        .count()
}

fn lost(h: &Harness, source: &str) -> usize {
    h.events()
        .iter()
        .filter(|e| {
            matches!(e, Event::Health { note, source: Some(s), .. }
                if note.code == "capture.health.lost" && s == source)
        })
        .count()
}

#[test]
fn a_game_s_sound_ends_with_a_note_when_its_screen_is_given_up() {
    let mut h = Harness::recording(&[Src::Screen, Src::GameSound, Src::Mic]);
    h.feed_for(2.0);
    // Four real stops in ten minutes: three restarts, then the screen is given up for the take.
    for _ in 0..4 {
        h.pipe_says("screen", PipeEvent::StoppedBySystem);
        h.feed_for(4.0);
    }
    assert_eq!(
        sent(&h, "screen", PipeCmd::Stop),
        1,
        "the screen was given up"
    );
    assert_eq!(
        sent(&h, "game-sound", PipeCmd::Stop),
        1,
        "its sound ends with it"
    );
    assert_eq!(
        (lost(&h, "screen"), lost(&h, "game-sound")),
        (1, 1),
        "each noted"
    );
    assert_eq!(h.state(), WorkerState::Recording, "the mic records on");
}

#[test]
fn the_computer_s_sound_waits_for_a_sleeping_display_to_wake() {
    let mut c = cfg(&[Src::Screen, Src::Mic], 3);
    c.sources.push(SourceConfig {
        id: "computerSound".into(),
        role: SourceRole::SystemAudio,
        label: "capture.source.computerSound".into(),
        spec: SourceSpec::System,
    });
    let mut h = Harness::recording_cfg(c);
    h.feed_for(2.0);
    h.display_awake = false;
    h.tick();
    for _ in 0..5 {
        h.pipe_says("computerSound", PipeEvent::StoppedBySystem);
        h.feed_for(1.0);
    }
    assert_eq!(
        sent(&h, "computerSound", PipeCmd::Restart),
        0,
        "no restart spent while the display sleeps"
    );
    h.display_awake = true;
    h.feed_for(3.0);
    assert_eq!(sent(&h, "computerSound", PipeCmd::Restart), 1);
    assert_eq!(lost(&h, "computerSound"), 0, "never given up");
}
