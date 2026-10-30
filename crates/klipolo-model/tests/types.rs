mod common;
use common::*;
use klipolo_model::project::*;
use klipolo_model::timeline::*;

#[test]
fn new_project_defaults() {
    let p = Project::new("Demo");
    assert_eq!(p.version, PROJECT_VERSION);
    assert_eq!(p.settings.fps, 30);
    assert_eq!(p.settings.formats, vec![FormatKind::Landscape]);
    assert!(p.timeline.main.is_empty() && p.timeline.tracks.is_empty());
    assert!(!p.captions.enabled);
    // Platform-ready sound unless the user turns it off.
    assert_eq!(p.audio.loudness, Loudness::Youtube);
}

#[test]
fn json_uses_camel_case_and_type_tags() {
    let mut p = fixture();
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items.push(text_item("t", 1.0, 2.0));
    p.timeline.tracks.push(track);
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["settings"]["primaryFormat"], "landscape");
    assert_eq!(json["timeline"]["main"][0]["source"]["type"], "recording");
    assert_eq!(json["timeline"]["main"][0]["sourceOutUs"], 20_000_000);
    assert_eq!(
        json["timeline"]["main"][0]["camera"]["modeOverrides"]["portrait"],
        "splitBottom"
    );
    let item = &json["timeline"]["tracks"][0]["items"][0];
    assert_eq!(item["content"]["type"], "text");
    assert_eq!(
        item["layout"]["motion"]["enter"]["preset"],
        "klipolo.builtin/motion.fade"
    );
    assert_eq!(item["layout"]["anchor"], "topLeft");
    assert_eq!(
        json["recordings"][0]["sources"][1]["runs"][0]["offsetUs"],
        200_000
    );
}

#[test]
fn json_round_trips() {
    let mut p = fixture();
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    let mut text = text_item("t", 1.0, 2.0);
    // A computed position: without exact float parsing it would reopen one bit off.
    text.layout.rect.y = 0.013_530_864_197_530_865;
    track.items.push(text);
    track.items.push(video_item("v", 4.0, 2.0, 1.0));
    p.timeline.tracks.push(track);
    let text = serde_json::to_string(&p).unwrap();
    let back: Project = serde_json::from_str(&text).unwrap();
    assert_eq!(back, p);
}

#[test]
fn derived_values() {
    let p = fixture();
    assert_eq!(p.timeline.main[0].duration_us(), secs(20.0));
    assert_eq!(FormatKind::Portrait.size(), (1080, 1920));
    assert_eq!(
        p.recording("rec")
            .unwrap()
            .primary_source()
            .map(|s| s.id.as_str()),
        Some("screen")
    );
    let cam = CameraLayout::default();
    assert_eq!(cam.mode_for(FormatKind::Landscape), CameraMode::Bubble);
    assert_eq!(cam.mode_for(FormatKind::Portrait), CameraMode::SplitBottom);
    assert_eq!(text_item("t", 1.0, 2.0).end_us(), secs(3.0));
}

/// A camera shows as in a mirror by default, and a layout saved unmirrored stays so.
#[test]
fn the_camera_is_mirrored_by_default() {
    assert!(CameraLayout::default().mirror);
    let mut json = serde_json::to_value(CameraLayout::default()).unwrap();
    json["mirror"] = false.into();
    let saved: CameraLayout = serde_json::from_value(json).unwrap();
    assert!(!saved.mirror);
}
