//! The values a clip's Speed and Volume take (the clip card's, the video layer card's and chat's `setClip`'s), and
//! which clips have sound for Volume and Mute.

mod common;

use std::path::PathBuf;

use common::*;
use klipolo_model::clip_settings::{
    SPEEDS, VOLUME_MAX, VOLUME_MIN, VOLUME_STEP, is_speed, is_volume,
};
use klipolo_model::query::clip_has_sound;
use klipolo_model::timeline::{Clip, ClipSource};

#[test]
fn the_speed_list_is_the_cards_and_nothing_else() {
    assert_eq!(SPEEDS, [0.5, 0.75, 1.0, 1.25, 1.5, 2.0]);
    for speed in SPEEDS {
        assert!(is_speed(speed), "{speed}");
    }
    for speed in [0.0, 0.25, 0.6, 1.1, 3.0, -1.0, f64::NAN] {
        assert!(!is_speed(speed), "{speed}");
    }
}

#[test]
fn a_volume_is_within_0_and_2_in_hundredths() {
    assert_eq!((VOLUME_MIN, VOLUME_MAX, VOLUME_STEP), (0.0, 2.0, 0.01));
    for volume in [0.0, 0.07, 0.29, 0.5, 0.7, 1.0, 1.33, 1.99, 2.0] {
        assert!(is_volume(volume), "{volume}");
    }
    for volume in [-0.01, 2.01, 0.333, 0.005, 10.0, f64::NAN] {
        assert!(!is_volume(volume), "{volume}");
    }
}

#[test]
fn a_clip_has_sound_from_its_file_or_its_recordings_sound_sources() {
    let p = fixture();
    // The screen recording has a mic; "broll" has its own sound.
    assert!(clip_has_sound(&p, &p.timeline.main[0]));
    assert!(clip_has_sound(&p, &p.timeline.main[1]));
    // The camera file has none, nor does a picture.
    for file in ["camera", "logo"] {
        let clip = Clip::new(ClipSource::Asset { id: file.into() }, 0, secs(1.0));
        assert!(!clip_has_sound(&p, &clip), "{file}");
    }
    // A recording whose screen file carries sound is heard from it; without that and its mic, it has none.
    let mut silent = p.clone();
    silent.recordings[0].sources.retain(|s| s.id != "mic");
    assert!(clip_has_sound(&silent, &silent.timeline.main[0]));
    silent.assets[0].has_audio = false;
    assert!(!clip_has_sound(&silent, &silent.timeline.main[0]));
    // A file or recording that's gone has none.
    let gone = Clip::new(ClipSource::Asset { id: "gone".into() }, 0, secs(1.0));
    assert!(!clip_has_sound(&p, &gone));
}

/// `pnpm bindings` runs this (its name matches `export_bindings`): `apps/desktop/src/lib/clip-settings.json`, the speeds
/// and the volume range the clip card and the video layer card offer.
#[test]
fn export_bindings_clip_settings() {
    let settings = serde_json::json!({
        "speeds": SPEEDS,
        "volume": {"min": VOLUME_MIN, "max": VOLUME_MAX, "step": VOLUME_STEP},
    });
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/lib/clip-settings.json"),
        serde_json::to_string_pretty(&settings).unwrap() + "\n",
    )
    .unwrap();
}
