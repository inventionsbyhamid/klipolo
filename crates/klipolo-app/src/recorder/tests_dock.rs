//! The Dock icon's menu while a take records: the menu-bar item's own Pause or Resume, "Mark this moment" and "Stop and
//! save", so the person can mark or stop when the item is hidden behind the notch. The fake backend and fake workers
//! only.

use klipolo_capture::protocol::Command;
use klipolo_capture::shell::WorkerState::*;

use super::test_support::*;

fn ids(rec: &Fixture) -> Vec<String> {
    rec.rec().dock_items().into_iter().map(|i| i.id).collect()
}

fn titles(rec: &Fixture) -> Vec<String> {
    rec.rec()
        .dock_items()
        .into_iter()
        .map(|i| i.title)
        .collect()
}

/// Nothing while armed; Pause, Mark this moment and Stop and save once it records; a click sends that command to the
/// take, and its Stop is the person's, so the take is never resumed after it.
#[test]
fn the_dock_menu_pauses_marks_and_stops_the_running_take() {
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    rec.settle();
    assert!(ids(&rec).is_empty(), "armed: nothing to stop yet");
    rec.send(Command::Start).unwrap();
    for s in [Armed, Starting, Countdown, Recording] {
        rec.launch.emit(state(s));
    }
    rec.settle();
    assert_eq!(
        ids(&rec),
        ["klipolo.pause", "klipolo.marker", "klipolo.stop"]
    );
    assert_eq!(titles(&rec), ["Pause", "Mark this moment", "Stop and save"]);
    rec.rec().dock_click("klipolo.pause");
    rec.launch.emit(state(Paused));
    rec.settle();
    assert_eq!(
        titles(&rec),
        ["Resume", "Mark this moment", "Stop and save"]
    );
    rec.rec().dock_click("klipolo.marker");
    // Not one of the recorder's: nothing goes.
    rec.rec().dock_click("klipolo.startOver");
    rec.rec().dock_click("elsewhere");
    rec.rec().dock_click("klipolo.stop");
    assert_eq!(
        rec.launch.commands(),
        [
            Command::Start,
            Command::Pause,
            Command::Marker,
            Command::Stop
        ]
    );
    rec.launch.exit(Some(1));
    rec.settle();
    assert_eq!(
        rec.launch.spawned(),
        1,
        "never resumed after the Dock's Stop"
    );
}

/// Without a take, or once it stops, the menu holds nothing of the recorder's and a click sends nothing.
#[test]
fn the_dock_menu_is_empty_without_a_running_take() {
    let rec = fixture();
    assert!(ids(&rec).is_empty());
    rec.rec().dock_click("klipolo.marker");
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    for s in [Armed, Starting, Countdown, Recording, Stopping] {
        rec.launch.emit(state(s));
    }
    rec.settle();
    assert!(ids(&rec).is_empty(), "stopping: nothing to mark or stop");
    assert_eq!(rec.launch.commands(), [Command::Start]);
}
