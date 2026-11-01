//! Takes recorded by earlier versions still read: their `session.json` has no Start over key (and no
//! camera bubble, nor "Mirror my camera"), so Home still lists them, and they can be repaired and resumed.

#[path = "common/config.rs"]
mod config;

use klipolo_capture::session::{SessionDir, read_config};
use klipolo_model::project::SourceRole;

#[test]
fn an_older_session_json_without_start_over_still_reads() {
    let dir = tempfile::tempdir().unwrap();
    let take = SessionDir::at(&dir.path().join("old.klipolorec"));
    std::fs::create_dir_all(&take.root).unwrap();
    let cfg = config::config_with(&[("screen", SourceRole::Screen), ("mic", SourceRole::Mic)]);
    let mut json = serde_json::to_value(&cfg).unwrap();
    let keys = json["keys"].as_object_mut().unwrap();
    keys.remove("startOver");
    assert_eq!(keys.len(), 3, "an older take's three keys");
    json.as_object_mut().unwrap().remove("cameraBubble");
    json.as_object_mut().unwrap().remove("mirrorCamera");
    std::fs::write(take.config(), serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    let read = read_config(&take).expect("an old take's config reads");
    assert_eq!(read.keys.start_stop, cfg.keys.start_stop);
    assert_eq!(read.keys.start_over, None, "no Start over key");
    assert!(!read.camera_bubble);
    assert!(read.mirror_camera, "mirrored, the default");
}
