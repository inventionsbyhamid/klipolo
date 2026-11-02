//! A project's caption settings: no style until captions are first turned on (or a look
//! is picked), which writes the formats' default once; the checks of a look, a colour, a place and a language.

use klipolo_model::apply_ops;
use klipolo_model::captions::styles::{CaptionStyles, FALLBACK_STYLE};
use klipolo_model::contrib::ContribId;
use klipolo_model::ops::Op;
use klipolo_model::project::{CaptionSettings, FormatKind, Settings};

#[test]
fn the_first_enable_writes_the_default_and_a_new_format_never_changes_it() {
    use klipolo_model::ops::Op;
    // A landscape tutorial turns captions on without picking a style: Clean is written into the project.
    let p = klipolo_model::Project::new("talk");
    let on = CaptionSettings {
        enabled: true,
        ..CaptionSettings::default()
    };
    let p = klipolo_model::apply_ops(&p, &[Op::SetCaptions { captions: on }]).unwrap();
    assert_eq!(
        p.captions.style,
        Some(ContribId::from(FALLBACK_STYLE)),
        "landscape-only: Clean, written on the first enable"
    );
    // The person adds 9:16 to cut a Short: the landscape captions stay Clean.
    let settings = klipolo_model::project::Settings {
        formats: vec![FormatKind::Landscape, FormatKind::Portrait],
        ..p.settings.clone()
    };
    let p = klipolo_model::apply_ops(&p, &[Op::SetSettings { settings }]).unwrap();
    assert_eq!(
        p.captions.style,
        Some(ContribId::from(FALLBACK_STYLE)),
        "a new format never re-derives the style"
    );
    assert_eq!(
        CaptionStyles::builtin()
            .for_settings(&p.captions, &p.settings.formats)
            .0
            .id
            .as_str(),
        FALLBACK_STYLE
    );
    // Off and on again keeps it.
    let off = CaptionSettings {
        enabled: false,
        ..p.captions.clone()
    };
    let p = klipolo_model::apply_ops(&p, &[Op::SetCaptions { captions: off }]).unwrap();
    assert_eq!(p.captions.style, Some(ContribId::from(FALLBACK_STYLE)));
}

#[test]
fn caption_settings_name_a_style_and_places_per_format() {
    let json = serde_json::to_value(CaptionSettings::default()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "enabled": false, "place": {}, "adSafe": false }),
        "no style until one is chosen"
    );
}

/// A look that isn't the style's, a colour that isn't one, or a place off the video is refused with its code; a
/// format dropped from the project drops its place (and never the style).
#[test]
fn caption_settings_are_checked_and_a_dropped_format_drops_its_place() {
    use klipolo_model::ops::Op;
    let p = klipolo_model::Project::new("talk");
    let set = |captions: CaptionSettings| {
        klipolo_model::apply_ops(&p, &[Op::SetCaptions { captions }])
            .map_err(|e| e.coded().code.clone())
    };
    let boxed = Some(ContribId::builtin("caption.boxed"));
    let bad = [
        (
            CaptionSettings {
                style: boxed.clone(),
                variant: Some("plate".into()),
                ..CaptionSettings::default()
            },
            "model.captions.look",
        ),
        (
            CaptionSettings {
                accent: Some("yellow".into()),
                ..CaptionSettings::default()
            },
            "model.captions.color",
        ),
        (
            CaptionSettings {
                color: Some("#FFF".into()),
                ..CaptionSettings::default()
            },
            "model.captions.color",
        ),
        (
            CaptionSettings {
                size: Some(2.5),
                ..CaptionSettings::default()
            },
            "model.captions.size",
        ),
        (
            CaptionSettings {
                place: [(FormatKind::Landscape, 0.99)].into(),
                ..CaptionSettings::default()
            },
            "model.captions.place",
        ),
        (
            CaptionSettings {
                language: Some("ja jp".into()),
                ..CaptionSettings::default()
            },
            "model.captions.language",
        ),
    ];
    for (captions, code) in bad {
        assert_eq!(set(captions).unwrap_err(), code);
    }
    // A pack's look on a style this build doesn't have is kept, as the style is.
    set(CaptionSettings {
        style: Some(ContribId::from("com.acme.pack/caption.neon")),
        variant: Some("glow".into()),
        ..CaptionSettings::default()
    })
    .unwrap();

    let two = klipolo_model::project::Settings {
        formats: vec![FormatKind::Landscape, FormatKind::Portrait],
        ..p.settings.clone()
    };
    let placed = CaptionSettings {
        enabled: true,
        style: boxed,
        place: [(FormatKind::Landscape, 0.2), (FormatKind::Portrait, 0.3)].into(),
        ..CaptionSettings::default()
    };
    let p = klipolo_model::apply_ops(
        &p,
        &[
            Op::SetSettings { settings: two },
            Op::SetCaptions { captions: placed },
        ],
    )
    .unwrap();
    let one = klipolo_model::project::Settings {
        formats: vec![FormatKind::Portrait],
        primary_format: FormatKind::Portrait,
        ..p.settings.clone()
    };
    let p = klipolo_model::apply_ops(&p, &[Op::SetSettings { settings: one }]).unwrap();
    assert_eq!(p.captions.place, [(FormatKind::Portrait, 0.3)].into());
    assert_eq!(p.captions.style, Some(ContribId::builtin("caption.boxed")));
}

/// A look picked while captions are off pins the style it belongs to: adding 9:16 afterwards, or turning captions
/// on, keeps Clean with its plate, and neither edit is refused over the captions.
#[test]
fn a_look_picked_while_captions_are_off_pins_its_style() {
    let p = klipolo_model::Project::new("talk");
    let plate = CaptionSettings {
        variant: Some("plate".into()),
        ..CaptionSettings::default()
    };
    let p = apply_ops(&p, &[Op::SetCaptions { captions: plate }]).unwrap();
    assert_eq!(p.captions.style, Some(ContribId::from(FALLBACK_STYLE)));
    let settings = Settings {
        formats: vec![FormatKind::Landscape, FormatKind::Portrait],
        ..p.settings.clone()
    };
    let p = apply_ops(&p, &[Op::SetSettings { settings }])
        .expect("a new format is never refused over the captions' look");
    let on = CaptionSettings {
        enabled: true,
        ..p.captions.clone()
    };
    let p = apply_ops(&p, &[Op::SetCaptions { captions: on }]).unwrap();
    assert_eq!(p.captions.style, Some(ContribId::from(FALLBACK_STYLE)));
    assert_eq!(p.captions.variant.as_deref(), Some("plate"));
}
