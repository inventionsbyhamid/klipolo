//! More of the recorder item's actions, pure: Start over's shortcut in the menu, the Dock icon's menu, and
//! when the camera bubble shows.

use klipolo_capture::shell::WorkerState;
use klipolo_capture::shell_view::Words;

/// Start over has no key by default (⌥⌘N is Finder's New Smart Folder, and a stray press would
/// discard a take), so its menu item shows none; one the person sets shows beside it, and the clash check covers it.
/// The others show no key. Its item is paired with its action by id.
#[test]
fn start_over_shows_the_key_the_person_set_and_has_none_by_default() {
    use klipolo_capture::shell::{HotkeyAction, KeyBindings, MenuAction};
    use klipolo_capture::shell_view::{menu_shortcut, shortcut_text};
    use klipolo_platform::caps::OsFamily;
    for os in [OsFamily::Macos, OsFamily::Windows, OsFamily::Linux] {
        let keys = KeyBindings::defaults(os);
        assert_eq!(keys.start_over, None, "{os:?}");
        assert!(keys.distinct());
        assert_eq!(menu_shortcut(MenuAction::StartOver, &keys), None);
        assert!(
            !keys
                .all()
                .iter()
                .any(|(_, a)| *a == HotkeyAction::StartOver)
        );
    }
    let mut keys = KeyBindings::defaults(OsFamily::Macos);
    keys.start_over = Some("Control+Alt+Super+KeyN".into());
    let shown = menu_shortcut(MenuAction::StartOver, &keys).unwrap();
    assert_eq!(shortcut_text(shown, OsFamily::Macos), "⌃⌥⌘N");
    assert_eq!(keys.all().len(), 4);
    for other in [MenuAction::Stop, MenuAction::Pause, MenuAction::Marker] {
        assert_eq!(menu_shortcut(other, &keys), None, "{other:?}");
    }
    assert_eq!(
        MenuAction::from_id(MenuAction::StartOver.id()),
        Some(MenuAction::StartOver)
    );
    keys.start_over = Some(keys.pause.clone());
    assert!(!keys.distinct(), "a set key clashes like any other");
}

/// The Dock icon's menu: the item's own Pause or Resume, Mark and Stop and save while the take records or
/// is paused, in the same words; nothing before the gate or while saving.
#[test]
fn the_dock_offers_the_items_own_actions_only_while_the_take_runs() {
    use klipolo_capture::shell::MenuAction;
    use klipolo_capture::shell_view::{action_text, dock_actions};
    let w = Words::default();
    assert_eq!(
        dock_actions(WorkerState::Recording),
        [MenuAction::Pause, MenuAction::Marker, MenuAction::Stop]
    );
    let paused: Vec<String> = (dock_actions(WorkerState::Paused).iter())
        .map(|&a| action_text(a, WorkerState::Paused, &w))
        .collect();
    assert_eq!(paused, ["Resume", "Mark this moment", "Stop and save"]);
    for state in [
        WorkerState::Armed,
        WorkerState::Starting,
        WorkerState::Countdown,
        WorkerState::Stopping,
        WorkerState::Ready,
    ] {
        assert!(dock_actions(state).is_empty(), "{state:?}");
    }
}

/// The camera bubble's show and hide rules: with the switch on and the camera live, it shows while the
/// take counts down or records; it closes when the take pauses, stops or loses its camera, and never shows with the
/// switch off.
#[test]
fn the_camera_bubble_shows_only_while_a_live_camera_records() {
    use klipolo_capture::shell_view::bubble_shows;
    for (state, shown) in [
        (WorkerState::Armed, false),
        (WorkerState::Starting, false),
        (WorkerState::Countdown, true),
        (WorkerState::Recording, true),
        (WorkerState::Paused, false),
        (WorkerState::Stopping, false),
        (WorkerState::Ready, false),
        (WorkerState::Failed, false),
    ] {
        assert_eq!(bubble_shows(true, state, true), shown, "{state:?}");
        assert!(!bubble_shows(true, state, false), "{state:?}: camera lost");
        assert!(!bubble_shows(false, state, true), "{state:?}: switched off");
    }
}
