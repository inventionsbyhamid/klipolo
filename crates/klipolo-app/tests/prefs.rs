//! The app's own settings file (`prefs.json` in the support folder): other apps' access is off until the person turns
//! it on, and a file that is missing, unreadable or not Klipolo's reads as off.

use klipolo_app::prefs::{self, FILE, Prefs};

#[test]
fn prefs_default_off_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(prefs::load(dir.path()), Prefs::default());
    assert!(!prefs::load(dir.path()).mcp_enabled, "off until turned on");

    // Saved into a folder that isn't there yet, and read back.
    let support = dir.path().join("Klipolo");
    let on = Prefs { mcp_enabled: true };
    prefs::save(&support, &on).unwrap();
    assert_eq!(prefs::load(&support), on);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(support.join(FILE)).unwrap()).unwrap();
    assert_eq!(written, serde_json::json!({"mcpEnabled": true}));
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
