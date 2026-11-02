//! A sound layer that ducks under speech (`ItemContent::Audio::duck`): lowered by that many dB, -30 to 0, while someone
//! talks. Off is left out of the JSON, so projects saved before it load unchanged.
mod common;
use common::*;
use klipolo_model::coded::CodedError;
use klipolo_model::project::Project;
use klipolo_model::timeline::*;
use klipolo_model::{load_project, validate, ModelError};

/// The fixture with a music bed on an audio track, ducking by `duck`.
fn with_music(duck: Option<f64>) -> Project {
    let mut p = fixture();
    let mut music = Track::new(TrackKind::Audio, "Music");
    music.id = "music-track".into();
    music.items.push(Item {
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 0.3,
            muted: false,
            fade_in_us: secs(0.5),
            fade_out_us: secs(2.0),
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck,
            volume_keys: Vec::new(),
        },
        ..still_item("bed", 0.0, 30.0)
    });
    p.timeline.tracks.push(music);
    p
}

fn bed_json(json: &str) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    v["timeline"]["tracks"][0]["items"][0]["content"].clone()
}

fn duck_of(p: &Project) -> Option<f64> {
    match &p.timeline.tracks[0].items[0].content {
        ItemContent::Audio { duck, .. } => *duck,
        other => panic!("not a sound layer: {other:?}"),
    }
}

#[test]
fn duck_roundtrips_and_old_projects_load() {
    // On: saved in dB and read back as it was.
    let ducked = with_music(Some(-12.0));
    validate(&ducked).unwrap();
    let json = serde_json::to_string(&ducked).unwrap();
    assert_eq!(bed_json(&json)["duck"], serde_json::json!(-12.0));
    let loaded = load_project(&json).unwrap();
    assert_eq!(loaded, ducked);
    assert_eq!(duck_of(&loaded), Some(-12.0));

    // Off: no key at all, so a file is byte for byte what it was before ducking existed.
    let plain = with_music(None);
    let json = serde_json::to_string(&plain).unwrap();
    assert!(bed_json(&json).get("duck").is_none(), "{json}");
    assert_eq!(load_project(&json).unwrap(), plain);

    // A file saved before ducking existed (its sound layer has no `duck`) loads, not ducking.
    let mut old: serde_json::Value = serde_json::to_value(&ducked).unwrap();
    old["timeline"]["tracks"][0]["items"][0]["content"]
        .as_object_mut()
        .unwrap()
        .remove("duck");
    let loaded = load_project(&old.to_string()).unwrap();
    assert_eq!(duck_of(&loaded), None);
}

#[test]
fn duck_out_of_range_is_rejected() {
    for ok in [-30.0, -12.0, -0.5, 0.0] {
        validate(&with_music(Some(ok))).unwrap_or_else(|e| panic!("{ok} dB: {e:?}"));
    }
    for bad in [
        -30.5,
        -60.0,
        0.5,
        6.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert_eq!(
            validate(&with_music(Some(bad))),
            Err(ModelError::Invalid(CodedError::new("model.audio.duck"))),
            "{bad} dB"
        );
    }
    // The refusal has words of its own.
    assert_eq!(
        validate(&with_music(Some(-31.0))).unwrap_err(),
        refused("Ducking can lower a sound by 0 to 30 dB.")
    );
}
