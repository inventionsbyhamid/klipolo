//! The clock under the separate cursor: clocks are per stream, a screen's frames count from its first one (every
//! decoder numbers them from `render::probe::stream_start`), and the cursor file is on the take's clock, so the cursor
//! drawn over a frame is the one captured with it.

use klipolo_model::cursor_style::Smoothing;
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::project::{Asset, AssetKind, RecordingSource, SourceRole, SourceRun};

use crate::support::recording::cursor_project;
use crate::support::secs;
use crate::{cursor_at, export_ctx, footage, glide, mapped, near, style, take, the_take};

/// Where the screen's first frame is on the take's clock: its sound (the mic) starts at 0.
const VIDEO_START: f64 = 0.5;

/// A 60.5 s mic from the take's start.
fn mic() -> (Asset, RecordingSource) {
    let asset = Asset {
        id: "mic".into(),
        kind: AssetKind::Audio,
        path: "/media/mic".into(),
        name: "mic".into(),
        duration_us: secs(60.5),
        width: 0,
        height: 0,
        fps: 0.0,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    };
    let source = RecordingSource {
        id: "mic".into(),
        role: SourceRole::Mic,
        label: "capture.source.mic".into(),
        runs: vec![SourceRun {
            asset_id: "mic".into(),
            offset_us: 0,
            duration_us: secs(60.5),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: Default::default(),
        synced: None,
    };
    (asset, source)
}

/// A take whose picture starts after its sound: the cursor at take time t is drawn over the screen's frame
/// round((t − VIDEO_START)·fps), the one captured at t, and never at the file's own time t (a clock counted from the
/// take's start, or from the container's, would draw the cursor `VIDEO_START` early).
#[test]
fn a_take_whose_picture_starts_late_draws_the_cursor_on_its_frame() {
    let dir = tempfile::tempdir().unwrap();
    // A glide, so a frame off by one shows the cursor somewhere else.
    let lines = take(glide((0.1, 0.2), (0.9, 0.8), VIDEO_START, 9.0));
    let mut p = cursor_project(dir.path(), &lines, style(|s| s.smoothing = Smoothing::Off));
    p.recordings[0].sources[0].runs[0].offset_us = secs(VIDEO_START);
    let (asset, source) = mic();
    p.assets.push(asset);
    p.recordings[0].sources.push(source);
    klipolo_model::validate(&p).expect("a valid take");
    let ctx = export_ctx(p);
    let take = the_take(&ctx);
    for t in [1.0, 2.37, 5.5, 8.99] {
        let v = footage(&ctx, t, Landscape);
        assert_eq!(v.source.time_us, secs(t) - secs(VIDEO_START), "at {t} s");
        let frame = (v.source.time_us as f64 * v.source.fps / 1e6).round();
        assert_eq!(
            frame,
            ((t - VIDEO_START) * v.source.fps).round(),
            "at {t} s"
        );
        let c = cursor_at(&ctx, t).expect("drawn");
        let at_t = take.track.position(secs(t)).unwrap();
        near(c.hotspot, mapped(&v, at_t), 1e-9);
        let file_time = take.track.position(secs(t - VIDEO_START)).unwrap();
        assert!(
            (at_t.0 - file_time.0).abs() > 0.01,
            "the glide moved meanwhile"
        );
    }
}
