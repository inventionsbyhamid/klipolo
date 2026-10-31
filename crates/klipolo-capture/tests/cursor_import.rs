//! A take's cursor into format 4: import takes the manifest's own values (never a placeholder),
//! and a game take keeps a cursor only when its profile asked for one, burned in, never followed.
#![cfg(feature = "synthetic")]

mod common;

use common::fake::{manifest_with, probe_ok};
use klipolo_capture::import::{clip_for, manifest_peaks, recording_from};
use klipolo_capture::manifest::ManifestCursor;
use klipolo_cursor::ClickSource;
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::framing::default_framing;
use klipolo_model::project::{FormatKind, Project, RecordingKind, SourceRole};

#[test]
fn import_takes_the_manifests_cursor_and_its_file_in_the_take() {
    let (mut m, dir) = manifest_with(&[("screen", SourceRole::Screen, &[(0, 10.0)])]);
    m.cursor = Some(ManifestCursor {
        file: "cursor.jsonl".into(),
        capture: CursorCapture::BurnedFrom { at_us: 4_000_000 },
        clicks: ClickSource::Sampled,
    });
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    let cursor = imported.recording.cursor.expect("the take's cursor");
    assert_eq!(
        (cursor.capture, cursor.clicks),
        (
            CursorCapture::BurnedFrom { at_us: 4_000_000 },
            ClickSource::Sampled
        )
    );
    assert_eq!(std::path::Path::new(&cursor.path), dir.path("cursor.jsonl"));
    // A key that would leave the take's folder gives no cursor.
    m.cursor.as_mut().unwrap().file = "../cursor.jsonl".into();
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    assert!(imported.recording.cursor.is_none());
}

#[test]
fn a_game_keeps_a_burned_in_cursor_only_when_asked_and_never_follows_it() {
    let (mut m, dir) = manifest_with(&[("game", SourceRole::Game, &[(0, 10.0)])]);
    m.kind = RecordingKind::Gameplay;
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    assert!(
        imported.recording.cursor.is_none(),
        "a game whose profile has no cursor"
    );
    m.cursor = Some(ManifestCursor {
        file: "cursor.jsonl".into(),
        capture: CursorCapture::Burned,
        clicks: ClickSource::Sampled,
    });
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    let recording = imported.recording;
    assert_eq!(
        recording.cursor.as_ref().map(|c| c.capture),
        Some(CursorCapture::Burned)
    );
    let clip = clip_for(&recording, false);
    let mut project = Project::new("game");
    project.recordings.push(recording);
    let framing = default_framing(&project, &clip, FormatKind::Portrait);
    assert_eq!(framing.follow, None, "a game take is never followed");
}
