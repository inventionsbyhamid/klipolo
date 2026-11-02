//! Caption styles as data: one JSON file per style, listed once, checked when it loads,
//! and the person's look and colour over it. The settings that name a style are `tests/caption_settings.rs`.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use klipolo_model::captions::style::{CaptionStyleDef, Paint};
use klipolo_model::captions::styles::{
    BuiltinCaptionStyle, CaptionStyles, BUILTIN_FILES, DEFAULT_STYLE, FALLBACK_STYLE,
};
use klipolo_model::coded::english;
use klipolo_model::contrib::ContribId;
use klipolo_model::messages::MESSAGES;
use klipolo_model::project::{CaptionSettings, FormatKind};

#[test]
fn every_style_file_is_listed_and_valid() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("captions/styles");
    let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            e.ok()?
                .file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(String::from)
        })
        .collect();
    let listed: BTreeSet<String> = BUILTIN_FILES.iter().map(|(k, _)| (*k).to_owned()).collect();
    assert_eq!(
        on_disk, listed,
        "a style file must be listed in BUILTIN_FILES (and the list must have its file)"
    );
    for (key, json) in BUILTIN_FILES {
        let def: CaptionStyleDef =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("{key}.json: {e}"));
        def.check().unwrap_or_else(|e| panic!("{key}.json: {e}"));
        assert_eq!(def.id.as_str(), format!("klipolo.builtin/caption.{key}"));
    }
    assert_eq!(CaptionStyles::builtin().defs().count(), 9);
    // The one list is also the tool's and the UI's word for each style.
    for s in BuiltinCaptionStyle::ALL {
        assert_eq!(serde_json::to_value(s).unwrap(), s.key());
        assert!(CaptionStyles::builtin().get(&s.id()).is_some(), "{s:?}");
    }
}

#[test]
fn style_names_are_neutral() {
    const BANNED: [&str; 8] = [
        "tiktok", "mrbeast", "beast", "hormozi", "abdaal", "iman", "capcut", "submagic",
    ];
    for def in CaptionStyles::builtin().defs() {
        let names = std::iter::once(def.name.to_lowercase())
            .chain(def.variants.iter().map(|v| v.name.to_lowercase()));
        for n in names {
            assert!(!BANNED.iter().any(|b| n.contains(b)), "{n}");
        }
    }
}

#[test]
fn a_variant_then_the_persons_accent_apply_over_the_style() {
    let boxed = CaptionStyles::builtin()
        .get(&ContribId::builtin("caption.boxed"))
        .unwrap();
    let dark = boxed.with(Some("dark"), None);
    assert_eq!(dark.accent, "#000000");
    assert_eq!(
        dark.rgb(&Paint("contrast".into()), true),
        [255, 255, 255],
        "white text on a black box"
    );
    let red = boxed.with(Some("dark"), Some("#EA403F"));
    assert_eq!(red.accent, "#EA403F");
}

#[test]
fn an_unknown_style_draws_in_clean_and_says_so() {
    let s = CaptionSettings {
        style: Some(ContribId::from("com.acme.pack/caption.neon")),
        ..CaptionSettings::default()
    };
    let (style, missing) = CaptionStyles::builtin().for_settings(&s, &[FormatKind::Portrait]);
    assert!(missing);
    assert_eq!(style.id.as_str(), FALLBACK_STYLE);
}

#[test]
fn a_style_never_chosen_follows_the_projects_formats() {
    let none = CaptionSettings::default();
    let (style, _) = CaptionStyles::builtin()
        .for_settings(&none, &[FormatKind::Landscape, FormatKind::Portrait]);
    assert_eq!(
        style.id.as_str(),
        DEFAULT_STYLE,
        "9:16 among the formats: Bold"
    );
    let (style, _) = CaptionStyles::builtin().for_settings(&none, &[FormatKind::Square]);
    assert_eq!(style.id.as_str(), DEFAULT_STYLE);
    let (style, _) = CaptionStyles::builtin().for_settings(&none, &[FormatKind::Landscape]);
    assert_eq!(
        style.id.as_str(),
        "klipolo.builtin/caption.clean",
        "landscape-only: Clean"
    );
    assert_eq!(
        klipolo_model::captions::styles::default_style(&[FormatKind::Landscape]),
        FALLBACK_STYLE
    );
}

#[test]
fn a_bad_style_is_refused_with_its_reason() {
    let mut def: CaptionStyleDef = serde_json::from_str(BUILTIN_FILES[0].1).unwrap();
    def.fill = Paint("red".into());
    assert!(def.check().unwrap_err().contains("fill"));
    let mut def: CaptionStyleDef = serde_json::from_str(BUILTIN_FILES[0].1).unwrap();
    def.pages.max_lines = 0;
    assert!(def.check().unwrap_err().contains("maxLines"));
}

/// Packs will carry style files under a schema version: a newer one is refused as needing a newer Klipolo, even with
/// fields this build doesn't know, never as a broken file. The current schema with an unknown field is refused, naming
/// it.
#[test]
fn a_newer_style_file_needs_a_newer_klipolo() {
    let mut v: serde_json::Value = serde_json::from_str(BUILTIN_FILES[0].1).unwrap();
    v["schema"] = 2.into();
    v["sparkle"] = true.into();
    let err = CaptionStyleDef::from_json(&v.to_string()).unwrap_err();
    assert_eq!(err.code, "model.captions.styleNeedsNewer");
    v["schema"] = 1.into();
    let err = CaptionStyleDef::from_json(&v.to_string()).unwrap_err();
    assert_eq!(err.code, "model.captions.styleUnreadable");
    assert!(english(&[MESSAGES], &err).contains("sparkle"), "{err:?}");
    let bold = CaptionStyleDef::from_json(BUILTIN_FILES[0].1).unwrap();
    assert_eq!(bold.id.as_str(), DEFAULT_STYLE);
}

/// `pnpm bindings` runs this (its name matches `export_bindings`): `apps/desktop/src/lib/caption-styles.json`, every
/// built-in style for the UI's tiles, keys sorted.
#[test]
fn export_bindings_caption_styles() {
    let defs: Vec<serde_json::Value> = CaptionStyles::builtin()
        .defs()
        .map(|d| common::sorted_keys(serde_json::to_value(&**d).unwrap()))
        .collect();
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/lib/caption-styles.json"),
        serde_json::to_string_pretty(&defs).unwrap() + "\n",
    )
    .unwrap();
}

/// The shorts' two styles sit in 9:16's lower third, under a speaker's chin, inside the band the apps keep clear (the
/// engine's organic-safe 0.15 to 0.72).
#[test]
fn the_shorts_styles_sit_in_the_lower_third_of_9_16() {
    for key in ["word-pop", "bold-box"] {
        let style = CaptionStyles::builtin()
            .get(&ContribId::builtin(&format!("caption.{key}")))
            .unwrap();
        let y = style.position.get(FormatKind::Portrait).y;
        assert!(
            (0.667..=0.72).contains(&y),
            "{key}: centred at {y} of the height"
        );
    }
}
