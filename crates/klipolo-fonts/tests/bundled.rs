use klipolo_fonts::bundled::{BUNDLED, Category, bundled_families};
use sha2::{Digest, Sha256};

fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts")
}

#[test]
fn every_bundled_face_is_the_pinned_static_file_named_as_its_family() {
    // Twelve families people pick, plus Adobe NotDef, the last CJK fallback.
    assert_eq!(BUNDLED.len(), 18);
    assert_eq!(bundled_families().count(), 13);
    assert_eq!(
        BUNDLED
            .iter()
            .filter(|f| f.category == Category::Fallback)
            .map(|f| f.family)
            .collect::<Vec<_>>(),
        ["Adobe NotDef"]
    );
    for f in BUNDLED {
        let bytes = std::fs::read(dir().join(f.file)).unwrap_or_else(|e| panic!("{}: {e}", f.file));
        assert_eq!(bytes.len() as u64, f.bytes, "{}", f.file);
        assert_eq!(hex::encode(Sha256::digest(&bytes)), f.sha256, "{}", f.file);
        let face = ttf_parser::Face::parse(&bytes, 0).unwrap();
        assert!(!face.is_variable(), "{} must be a static face", f.file);
        assert_eq!(face.weight().to_number(), f.weight, "{}", f.file);
        assert_eq!(face.is_italic(), f.italic, "{}", f.file);
        // Registered by the catalogue's name, never the file's ("Inter 18pt", a missing ID 16): the file's own family is
        // only checked to be the same family, so a wrong file is caught.
        let names: Vec<String> = face
            .names()
            .into_iter()
            .filter(|n| {
                n.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY
                    || n.name_id == ttf_parser::name_id::FAMILY
            })
            .filter_map(|n| n.to_string())
            .collect();
        assert!(
            names.iter().any(|n| n.starts_with(f.family)),
            "{}: {names:?}",
            f.file
        );
    }
}

/// cosmic-text takes glyph 0 as missing, so the missing-glyph boxes need Adobe NotDef to map CJK (and
/// every other code point) to a real glyph.
#[test]
fn adobe_notdef_maps_cjk_to_a_real_glyph() {
    let notdef = BUNDLED
        .iter()
        .find(|f| f.category == Category::Fallback)
        .unwrap();
    let bytes = std::fs::read(dir().join(notdef.file)).unwrap();
    let face = ttf_parser::Face::parse(&bytes, 0).unwrap();
    for c in ['今', 'あ', 'カ', '한', '。', 'ㄅ', '𠀀'] {
        let glyph = face
            .glyph_index(c)
            .unwrap_or_else(|| panic!("{c} has no glyph"));
        assert_ne!(glyph.0, 0, "{c}");
    }
}

#[test]
fn every_bundled_family_ships_its_licence() {
    for f in BUNDLED {
        let folder = dir().join(f.file).parent().unwrap().to_path_buf();
        assert!(
            folder.join(f.licence.file_name()).is_file(),
            "{}",
            folder.display()
        );
    }
}

#[test]
fn aliases_never_collide_with_installed_names() {
    use klipolo_fonts::alias::alias;
    use klipolo_model::font::{FontRef, FontSource};
    assert_eq!(alias(&FontRef::inter()), "kl:b:inter");
    assert_eq!(
        alias(&FontRef {
            family: "Playfair Display".into(),
            source: FontSource::Google {
                version: "v37".into()
            }
        }),
        "kl:g:playfair-display:v37"
    );
    assert_eq!(
        alias(&FontRef {
            family: "Avenir Next".into(),
            source: FontSource::Installed
        }),
        "Avenir Next"
    );
}

/// A project saves a font as its family and source; the alias is made from them only when the faces are registered, so
/// a saved project never holds one and opens with the aliases this build makes.
#[test]
fn a_saved_font_holds_its_family_and_source_never_its_alias() {
    use klipolo_fonts::alias::alias;
    use klipolo_model::font::{FontRef, FontSource};
    let google = FontRef {
        family: "Playfair Display".into(),
        source: FontSource::Google {
            version: "v37".into(),
        },
    };
    for (font, saved) in [
        (
            FontRef::inter(),
            r#"{"family":"Inter","source":{"type":"bundled"}}"#,
        ),
        (
            google,
            r#"{"family":"Playfair Display","source":{"type":"google","version":"v37"}}"#,
        ),
    ] {
        let json = serde_json::to_string(&font).unwrap();
        assert_eq!(json, saved);
        assert!(!json.contains(&alias(&font)), "{json}");
        assert_eq!(serde_json::from_str::<FontRef>(saved).unwrap(), font);
    }
}

/// Every built-in caption style draws in bundled faces, and its `charEm` (the model's width estimate for
/// paging) is within 10 % of its face's measured average advance, in the style's case.
#[test]
fn caption_styles_use_bundled_faces_and_their_char_em_is_measured() {
    use klipolo_model::captions::style::TextCase;
    use klipolo_model::captions::styles::CaptionStyles;
    let sample = "The quick brown fox jumps over the lazy dog 0123";
    // Every style is measured and printed before any is refused, so one run gives every file its value.
    let mut wrong = Vec::new();
    for def in CaptionStyles::builtin().defs() {
        let face = BUNDLED
            .iter()
            .find(|b| {
                b.family == def.font.family
                    && b.weight == def.font.weight
                    && b.italic == def.font.italic
            })
            .unwrap_or_else(|| {
                panic!(
                    "{}: {} {} isn't bundled",
                    def.id, def.font.family, def.font.weight
                )
            });
        for f in &def.fallbacks {
            assert!(
                BUNDLED
                    .iter()
                    .any(|b| b.family == f.family && b.weight == f.weight && b.italic == f.italic),
                "{}",
                def.id
            );
        }
        let bytes = std::fs::read(dir().join(face.file)).unwrap();
        let ttf = ttf_parser::Face::parse(&bytes, 0).unwrap();
        let text = if matches!(def.case, TextCase::Upper) {
            sample.to_uppercase()
        } else {
            sample.into()
        };
        let units: f64 = text
            .chars()
            .filter_map(|c| ttf.glyph_index(c))
            .map(|g| f64::from(ttf.glyph_hor_advance(g).unwrap_or(0)))
            .sum();
        let measured = units / f64::from(ttf.units_per_em()) / text.chars().count() as f64;
        println!("{}: charEm measured {measured:.4}", def.id);
        if (def.char_em - measured).abs() / measured > 0.10 {
            wrong.push(format!(
                "{}: charEm {} vs measured {measured:.3}",
                def.id, def.char_em
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
