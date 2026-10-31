use klipolo_capture::shell::{Level, StatusView, WorkerState};
use klipolo_capture::shell_view::{
    HotkeyRegistry, KeysController, Words, menu_lines, status_title,
};

#[test]
fn the_title_shows_the_state_and_elapsed_time() {
    let v = |state, elapsed_us| StatusView {
        state,
        elapsed_us,
        lines: vec![],
        level: Level::Ok,
        countdown_s: None,
        mic_dbfs: None,
        held: None,
        by_key: false,
        bubble: false,
    };
    assert_eq!(
        status_title(&v(WorkerState::Recording, 754_000_000)),
        "● 12:34"
    );
    assert_eq!(
        status_title(&v(WorkerState::Paused, 754_000_000)),
        "❚❚ 12:34"
    );
    assert_eq!(
        status_title(&v(WorkerState::Recording, 3_723_000_000)),
        "● 1:02:03"
    );
}

#[test]
fn words_use_the_ui_s_template_and_fall_back_to_english() {
    let w = Words::from([(
        "capture.menu.stop".to_string(),
        "Stoppen und sichern".to_string(),
    )]);
    assert_eq!(w.say("capture.menu.stop", &[]), "Stoppen und sichern");
    assert_eq!(
        w.say(
            "capture.menu.fps",
            &[("source", "Screen".into()), ("fps", "60".into())]
        ),
        "Screen: 60 fps"
    );
}

#[derive(Default)]
struct Fake {
    on: bool,
    calls: Vec<&'static str>,
}
impl HotkeyRegistry for Fake {
    fn register(&mut self, keys: &klipolo_capture::shell::KeyBindings) -> Result<(), String> {
        assert_eq!(keys.marker, "Ctrl+Alt+KeyM", "the person's own binding");
        self.on = true;
        self.calls.push("register");
        Ok(())
    }
    fn unregister(&mut self) {
        self.on = false;
        self.calls.push("unregister");
    }
}

#[test]
fn hotkeys_are_registered_only_while_armed() {
    let mut bindings =
        klipolo_capture::shell::KeyBindings::defaults(klipolo_platform::caps::OsFamily::Macos);
    bindings.marker = "Ctrl+Alt+KeyM".into();
    let mut keys = KeysController::new(Fake::default(), bindings);
    assert!(!keys.registry().on, "nothing at start");
    keys.apply(true);
    keys.apply(true);
    assert!(keys.registry().on);
    keys.apply(false);
    assert_eq!(keys.registry().calls, ["register", "unregister"]);
}

#[test]
fn the_menu_lists_the_sources_then_what_the_person_can_do() {
    use klipolo_capture::shell::MenuAction;
    use klipolo_capture::shell_view::menu_actions;
    let w = Words::from([(
        "capture.menu.stop".to_string(),
        "Stoppen und sichern".to_string(),
    )]);
    let view = |state| StatusView {
        state,
        elapsed_us: 0,
        lines: vec!["Screen: 60 fps".into(), "Mic".into()],
        level: Level::Ok,
        countdown_s: None,
        mic_dbfs: None,
        held: None,
        by_key: false,
        bubble: false,
    };
    let recording = menu_lines(&view(WorkerState::Recording), &w);
    assert_eq!(
        recording,
        [
            "Screen: 60 fps",
            "Mic",
            "Pause",
            "Mark this moment",
            "Stoppen und sichern",
            "Start over"
        ]
    );
    assert_eq!(
        menu_actions(WorkerState::Recording),
        [
            MenuAction::Pause,
            MenuAction::Marker,
            MenuAction::Stop,
            MenuAction::StartOver
        ]
    );
    assert_eq!(menu_lines(&view(WorkerState::Paused), &w)[2], "Resume");
    assert_eq!(menu_lines(&view(WorkerState::Paused), &w)[5], "Start over");
    // Before the gate a Stop leaves no take: the person cancels; there is nothing to start over.
    assert_eq!(menu_lines(&view(WorkerState::Armed), &w)[2..], ["Cancel"]);
    assert_eq!(menu_actions(WorkerState::Armed), [MenuAction::Stop]);
    assert_eq!(menu_actions(WorkerState::Countdown), [MenuAction::Stop]);
    // Saving: nothing to press, Start over included.
    assert!(menu_actions(WorkerState::Stopping).is_empty());
    assert!(menu_actions(WorkerState::Ready).is_empty());
}

#[test]
fn the_take_s_activity_is_held_only_while_a_started_take_runs() {
    use klipolo_capture::shell_view::TakeActivity;
    let mut activity = TakeActivity::default();
    for (state, held) in [
        (WorkerState::Armed, false),
        // A Cancel before Record goes through Stopping: it begins nothing.
        (WorkerState::Stopping, false),
        (WorkerState::Countdown, false),
        (WorkerState::Recording, true),
        (WorkerState::Paused, true),
        (WorkerState::Stopping, true),
        (WorkerState::Ready, false),
    ] {
        activity.apply(state);
        assert_eq!(activity.held(), held, "{state:?}");
    }
    let cancelled = StatusView {
        state: WorkerState::Stopping,
        elapsed_us: 0,
        lines: vec![],
        level: Level::Ok,
        countdown_s: None,
        mic_dbfs: None,
        held: None,
        by_key: false,
        bubble: false,
    };
    assert_eq!(status_title(&cancelled), "", "no 0:00");
}

#[test]
fn a_partly_taken_set_of_shortcuts_never_stays_registered() {
    // Another app holds the second of the three shortcuts; global-hotkey stops there, the first one registered. Stop
    // releases it.
    #[derive(Default)]
    struct Partial {
        held: Vec<String>,
    }
    impl HotkeyRegistry for Partial {
        fn register(&mut self, keys: &klipolo_capture::shell::KeyBindings) -> Result<(), String> {
            self.held.push(keys.start_stop.clone());
            Err("⌥⌘P".into())
        }
        fn unregister(&mut self) {
            self.held.clear();
        }
    }
    let bindings =
        klipolo_capture::shell::KeyBindings::defaults(klipolo_platform::caps::OsFamily::Macos);
    let mut keys = KeysController::new(Partial::default(), bindings);
    keys.apply(true);
    assert_eq!(
        keys.registry().held.len(),
        1,
        "the free one works for the take"
    );
    keys.apply(false);
    assert!(keys.registry().held.is_empty(), "none outlives the take");
}

#[test]
fn a_taken_shortcut_is_a_line_in_the_person_s_words() {
    // The status line for a taken shortcut is a code like every other line.
    struct Taken;
    impl HotkeyRegistry for Taken {
        fn register(&mut self, _: &klipolo_capture::shell::KeyBindings) -> Result<(), String> {
            Err("⌥⌘P".into())
        }
        fn unregister(&mut self) {}
    }
    let bindings =
        klipolo_capture::shell::KeyBindings::defaults(klipolo_platform::caps::OsFamily::Macos);
    let mut keys = KeysController::new(Taken, bindings);
    keys.apply(true);
    let line = keys.problem().expect("noted");
    assert_eq!(line.code, "capture.keys.taken");
    assert_eq!(
        Words::default().word(line),
        "⌥⌘P is used by another app, so it won't work in this recording."
    );
    let german = Words::from([(
        "capture.keys.taken".to_string(),
        "{keys} gehört einer anderen App.".to_string(),
    )]);
    assert_eq!(german.word(line), "⌥⌘P gehört einer anderen App.");
}

#[test]
fn the_menus_word_every_value_as_the_english_catalogue_does() {
    // One renderer for both, so a moment, a length or a time never reads differently in the menu.
    let interrupted = klipolo_model::coded::CodedError::new("capture.recovered.interrupted")
        .with("timeAtMs", 1_760_600_000_000_i64)
        .with("durationUs", 1_500_000_i64);
    assert_eq!(
        Words::default().word(&interrupted),
        klipolo_capture::english::english(&interrupted)
    );
}

#[test]
fn menu_items_carry_their_actions_and_the_armed_line_names_the_os_s_shortcut() {
    use klipolo_capture::shell::MenuAction;
    use klipolo_capture::shell_view::{menu_items, ready_line, shortcut_text};
    use klipolo_platform::caps::OsFamily;
    let w = Words::from([(
        "capture.menu.ready".to_string(),
        "Bereit: {keys} drücken".to_string(),
    )]);
    let view = |state| StatusView {
        state,
        elapsed_us: 0,
        lines: vec!["Mic".into()],
        level: Level::Ok,
        countdown_s: None,
        mic_dbfs: None,
        held: None,
        by_key: false,
        bubble: false,
    };
    // The keys as each OS shows them.
    assert_eq!(shortcut_text("Alt+Super+KeyR", OsFamily::Macos), "⌥⌘R");
    assert_eq!(shortcut_text("Ctrl+Shift+Digit5", OsFamily::Macos), "⌃⇧5");
    assert_eq!(
        shortcut_text("Ctrl+Alt+KeyR", OsFamily::Windows),
        "Ctrl+Alt+R"
    );
    assert_eq!(shortcut_text("Super+F5", OsFamily::Linux), "Super+F5");
    // Armed: the ready line first, in the person's words; a click is found by its item's id.
    let ready = ready_line(WorkerState::Armed, &w, Some("⌥⌘R"));
    assert_eq!(ready.as_deref(), Some("Bereit: ⌥⌘R drücken"));
    assert_eq!(ready_line(WorkerState::Recording, &w, Some("⌥⌘R")), None);
    assert_eq!(
        ready_line(WorkerState::Armed, &w, None),
        None,
        "the key is taken"
    );
    let armed = menu_items(&view(WorkerState::Armed), &w, &[ready.unwrap()]);
    let texts: Vec<_> = armed.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["Bereit: ⌥⌘R drücken", "Mic", "Cancel"]);
    assert_eq!(armed[2].action, Some(MenuAction::Stop));
    // Recording: each action with its own words, whatever lines come before it.
    for item in menu_items(&view(WorkerState::Recording), &w, &[]) {
        let Some(action) = item.action else { continue };
        let own = match action {
            MenuAction::Pause => "Pause",
            MenuAction::Marker => "Mark this moment",
            MenuAction::Stop => "Stop and save",
            MenuAction::Resume => "Resume",
            MenuAction::StartOver => "Start over",
        };
        assert_eq!(item.text, own);
        assert_eq!(MenuAction::from_id(action.id()), Some(action));
    }
    assert_eq!(
        MenuAction::from_id("line-0"),
        None,
        "a plain line is no action"
    );
}
