//! What Sync reads off the engine. The probe's sound lead, creation time and audio format (fixtures from
//! scripts/gen-engine-fixtures.sh, ignored by default; CI runs them), and the Sync hold on thumbnail scans. The sound
//! cache's cancellable builds are unit tests (`audio::pcm::sync_tests`): their block hooks exist only in test builds.
mod support;

use std::sync::Arc;
use std::time::Duration;

use klipolo_engine::audio::pcm::{PcmKey, PcmStore};
use klipolo_engine::media::store::{Hold, ThumbStore, Want};
use klipolo_engine::media::thumbs::strip;
use klipolo_engine::render::probe::{AudioFormat, MediaKind, probe_media};
use klipolo_platform::durable::Os;
use support::fixture;

/// 2026-10-16T10:00:00Z, the fixture's stated creation time.
const CREATED_MS: i64 = 1_792_144_800_000;

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn the_probe_reports_a_sound_that_starts_40_ms_after_the_picture() {
    let info = probe_media(&fixture("lead-40ms.mp4")).unwrap();
    assert_eq!(info.kind, MediaKind::Video);
    assert!(
        (info.audio_lead_us - 40_000).abs() <= 1_000,
        "{}",
        info.audio_lead_us
    );
    assert_eq!(
        info.audio,
        Some(AudioFormat {
            codec: "aac".into(),
            rate: 48_000,
            channels: 2,
        })
    );
    assert_eq!(info.created_ms, Some(CREATED_MS));
}

/// The AAC encoder's priming rides the MOV edit list; it never reads as a lead.
#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn aac_in_mov_without_an_offset_has_no_lead() {
    let info = probe_media(&fixture("aac-in-mov.mov")).unwrap();
    assert!(info.audio_lead_us.abs() <= 1_000, "{}", info.audio_lead_us);
    assert_eq!(info.audio.map(|a| a.codec), Some("aac".into()));
    assert_eq!(info.created_ms, None, "the fixture states none");
}

/// The first frame of `fixture`'s sound cache whose level passes a quarter of the file's loudest sample.
fn first_loud_frame(fixture_name: &str) -> usize {
    let cache = tempfile::tempdir().unwrap();
    let path = fixture(fixture_name).to_string_lossy().to_string();
    let pcm = PcmStore::new(cache.path().to_path_buf())
        .build(&PcmKey { path })
        .unwrap();
    let channels = pcm.channels();
    let mut samples = vec![0.0f32; pcm.frames() * channels];
    pcm.read(0, &mut samples);
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.01, "{fixture_name} has sound");
    samples
        .iter()
        .position(|s| s.abs() > peak / 4.0)
        .map(|i| i / channels)
        .unwrap()
}

/// The sound cache's frame 0 is where the probe's sound start is, so `audio_lead_us` places the cache
/// keeps its AAC priming in both). lead-40ms.mp4's tone begins 61.333 ms after its picture
/// and its stream 40 ms after it: 1,024 frames in. aac-in-mov.mov's edit list skips the priming: frame 0. Either tone
/// is a 440 Hz sine from 0, which first reaches a quarter of its peak `RISE` frames in.
#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn the_sound_cache_starts_where_the_probe_says_the_sound_does() {
    const RISE: f64 = 4.39;
    let (late, at_zero) = (
        first_loud_frame("lead-40ms.mp4"),
        first_loud_frame("aac-in-mov.mov"),
    );
    let off = |frame: usize, tone: f64| (frame as f64 - tone - RISE).abs();
    assert!(off(late, 1_024.0) <= 4.0, "{late} (and {at_zero})");
    assert!(off(at_zero, 0.0) <= 4.0, "{at_zero} (and {late})");
}

/// `Hold::Sync` holds thumbnail scans on its own bit: another hold's resume leaves it held, and the scans finish
/// where they were once it goes. The activity half joins when the screen activity pass lands (it obeys these holds).
#[test]
fn a_sync_hold_pauses_thumbnail_scans_and_the_activity_pass() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("scan-130s-720p.mp4").to_string_lossy().to_string();
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    store.pause(Hold::Sync);
    store.pause(Hold::Playback);
    store.want(vec![Want {
        path: video.clone(),
        duration_us: 130_000_000,
        at_us: 0,
    }]);
    store.resume(Hold::Playback);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!store.scanning(), "nothing scans while Sync listens");
    assert!(strip(root.path(), &video).is_none_or(|s| s.chunks.is_empty()));
    store.resume(Hold::Sync);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while strip(root.path(), &video).is_none_or(|s| s.chunks.len() != 3) {
        assert!(std::time::Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
