//! The app's own settings file (`prefs.json` in the support folder): other apps' access is off until the person turns
//! it on, the theme is the system's until one is picked, and a file that is missing, unreadable or not Klipolo's reads
//! as those defaults.

use klipolo_app::prefs::{self, FILE, Prefs, Theme};

#[test]
fn prefs_default_off_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(prefs::load(dir.path()), Prefs::default());
    assert!(!prefs::load(dir.path()).mcp_enabled, "off until turned on");

    // Saved into a folder that isn't there yet, and read back.
    let support = dir.path().join("Klipolo");
    let on = Prefs {
        mcp_enabled: true,
        ..Prefs::default()
    };
    prefs::save(&support, &on).unwrap();
    assert_eq!(prefs::load(&support), on);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(support.join(FILE)).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({"mcpEnabled": true, "theme": "system"})
    );
    prefs::save(&support, &Prefs::default()).unwrap();
    assert_eq!(prefs::load(&support), Prefs::default());
    // Nothing but the file is left behind.
    let names: Vec<String> = std::fs::read_dir(&support)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [FILE]);

    // Unreadable, or not what Klipolo writes: off.
    for bad in ["", "not json", "[true]", r#"{"mcpEnabled": "yes"}"#] {
        std::fs::write(support.join(FILE), bad).unwrap();
        assert_eq!(prefs::load(&support), Prefs::default(), "{bad:?}");
    }
    // A key this Klipolo doesn't know is left alone; one it knows is read.
    std::fs::write(support.join(FILE), r#"{"mcpEnabled": true, "later": 1}"#).unwrap();
    assert!(prefs::load(&support).mcp_enabled);
    // A folder in its place.
    std::fs::remove_file(support.join(FILE)).unwrap();
    std::fs::create_dir(support.join(FILE)).unwrap();
    assert_eq!(prefs::load(&support), Prefs::default());
    assert!(prefs::save(&support, &on).is_err());
}

/// Settings' Appearance: the system's until the person picks Light or Dark. A value this Klipolo doesn't know reads as
/// the system's, and the other settings are read as they are.
#[test]
fn theme_defaults_to_the_system_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(prefs::load(dir.path()).theme, Theme::System);
    for theme in [Theme::Light, Theme::Dark, Theme::System] {
        let saved = Prefs {
            theme,
            ..Prefs::default()
        };
        prefs::save(dir.path(), &saved).unwrap();
        assert_eq!(prefs::load(dir.path()), saved, "{theme:?}");
    }
    let dark = Prefs {
        theme: Theme::Dark,
        ..Prefs::default()
    };
    prefs::save(dir.path(), &dark).unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(written["theme"], "dark");

    let others = Prefs {
        mcp_enabled: true,
        theme: Theme::System,
    };
    for unknown in [r#""sepia""#, "3", "null", r#"{"light": true}"#] {
        let file = format!(r#"{{"mcpEnabled": true, "theme": {unknown}}}"#);
        std::fs::write(dir.path().join(FILE), file).unwrap();
        assert_eq!(prefs::load(dir.path()), others, "{unknown}");
    }
}

/// One setting changed keeps the others: other apps' access switched leaves the theme as it was, and the reverse.
#[test]
fn update_changes_one_setting_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let support = dir.path().join("Klipolo");
    prefs::update(&support, |p| p.theme = Theme::Dark).unwrap();
    prefs::update(&support, |p| p.mcp_enabled = true).unwrap();
    assert_eq!(
        prefs::load(&support),
        Prefs {
            mcp_enabled: true,
            theme: Theme::Dark
        }
    );
    prefs::update(&support, |p| p.theme = Theme::Light).unwrap();
    assert!(prefs::load(&support).mcp_enabled);
}
