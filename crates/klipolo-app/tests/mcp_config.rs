//! What Settings' "Copy config" gives an MCP app: one JSON block that starts this same Klipolo
//! with `--mcp`, and whether Klipolo must be moved first (run from a disk image, or translocated by macOS, its path
//! changes on the next launch).

use std::path::Path;

use klipolo_app::mcp_config::{config_json, needs_move};

#[test]
fn config_json_uses_the_running_executable() {
    let exe = Path::new("/Applications/Klipolo.app/Contents/MacOS/klipolo-app");
    assert_eq!(
        config_json(exe),
        r#"{
  "mcpServers": {
    "klipolo": {
      "command": "/Applications/Klipolo.app/Contents/MacOS/klipolo-app",
      "args": [
        "--mcp"
      ]
    }
  }
}"#
    );
    // The running executable, as the app copies it: its path, whatever it holds, comes back as it is.
    let running = std::env::current_exe().unwrap();
    let config: serde_json::Value = serde_json::from_str(&config_json(&running)).unwrap();
    assert_eq!(
        config,
        serde_json::json!({"mcpServers": {"klipolo": {
            "command": running.to_string_lossy(),
            "args": ["--mcp"],
        }}})
    );
    let odd = Path::new("/Users/me/Apps/\"Klipolo\" \\ copy.app/Contents/MacOS/klipolo-app");
    let config: serde_json::Value = serde_json::from_str(&config_json(odd)).unwrap();
    assert_eq!(
        config["mcpServers"]["klipolo"]["command"],
        odd.to_string_lossy().as_ref()
    );
}

#[test]
fn translocated_or_dmg_path_needs_move() {
    for moved in [
        "/private/var/folders/x1/abc/T/AppTranslocation/7F2A-1B/d/Klipolo.app/Contents/MacOS/klipolo-app",
        "/Volumes/Klipolo/Klipolo.app/Contents/MacOS/klipolo-app",
        "/Volumes/Klipolo 1/Klipolo.app/Contents/MacOS/klipolo-app",
    ] {
        assert!(needs_move(Path::new(moved)), "{moved}");
    }
    for stays in [
        "/Applications/Klipolo.app/Contents/MacOS/klipolo-app",
        "/Users/me/Applications/Klipolo.app/Contents/MacOS/klipolo-app",
        "/Users/me/Volumes/Klipolo.app/Contents/MacOS/klipolo-app",
        "/Users/me/src/klipolo/target/debug/klipolo-app",
    ] {
        assert!(!needs_move(Path::new(stays)), "{stays}");
    }
}
