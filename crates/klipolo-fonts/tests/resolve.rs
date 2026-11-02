use klipolo_fonts::index::FontIndex;
use klipolo_fonts::matching::nearest;
use klipolo_fonts::resolve::resolve;
use klipolo_model::font::{FontRef, FontSource};

fn shipped() -> FontIndex {
    FontIndex::shipped(Some(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts"),
    ))
}

#[test]
fn css_matching_picks_the_nearest_weight_then_the_style() {
    let faces = [(400, false), (700, false), (900, true)];
    assert_eq!(nearest(&faces, 400, false), Some(0));
    assert_eq!(
        nearest(&faces, 500, false),
        Some(0),
        "500 tries 500, then lighter"
    );
    assert_eq!(
        nearest(&faces, 600, false),
        Some(1),
        "above 500: heavier first"
    );
    assert_eq!(
        nearest(&faces, 300, false),
        Some(0),
        "below 400: lighter, else heavier"
    );
    assert_eq!(
        nearest(&faces, 900, false),
        Some(1),
        "the style before the weight"
    );
    assert_eq!(nearest(&faces, 400, true), Some(2), "only italic face");
    assert_eq!(nearest(&[], 400, false), None);
}

#[test]
fn a_family_missing_the_asked_weight_resolves_to_its_own_nearest_face() {
    // Montserrat ships only Black (900): a title at 700 draws Montserrat Black, never another family at 700.
    let r = resolve(&shipped(), &FontRef::bundled("Montserrat"), 700, false);
    assert_eq!(
        (
            r.face.alias.as_str(),
            r.face.weight,
            r.face.italic,
            r.stand_in
        ),
        ("kl:b:montserrat", 900, false, false)
    );
    assert!(
        r.face
            .file
            .as_ref()
            .unwrap()
            .path
            .ends_with("Montserrat/Montserrat-Black.ttf")
    );
    // Rubik ships only 900 italic: an upright request gets it (the style falls back after no upright face exists).
    let r = resolve(&shipped(), &FontRef::bundled("Rubik"), 700, false);
    assert_eq!((r.face.weight, r.face.italic), (900, true));
}

#[test]
fn fallbacks_point_at_a_real_file_at_every_weight_and_cjk_only_after_a_download() {
    use klipolo_fonts::fallback::Fallback;
    use klipolo_fonts::resolve::resolve_fallback;
    use klipolo_model::text_language::CjkScript;
    let latin = resolve_fallback(&shipped(), Fallback::Latin, 800, true).unwrap();
    // Declared at the asked weight and style (the alias exists at all eighteen), drawn from Inter's nearest file.
    assert_eq!(
        (latin.alias.as_str(), latin.weight, latin.italic),
        ("kl:fallback:latin", 800, true)
    );
    assert!(latin.file.unwrap().path.ends_with("Inter/Inter-Bold.ttf"));
    // No Noto downloaded: no CJK fallback at all, never a system face.
    for s in [CjkScript::Sc, CjkScript::Tc, CjkScript::Jp, CjkScript::Kr] {
        assert_eq!(
            resolve_fallback(&shipped(), Fallback::Cjk(s), 400, false),
            None
        );
    }
    // Each bundled script's Noto Sans, declared at the asked weight and style, drawn from its nearest file.
    use klipolo_fonts::fallback::NotoScript;
    let hindi = resolve_fallback(
        &shipped(),
        Fallback::Script(NotoScript::Devanagari),
        900,
        true,
    )
    .unwrap();
    assert_eq!(
        (hindi.alias.as_str(), hindi.weight, hindi.italic),
        ("kl:fallback:devanagari", 900, true)
    );
    assert!(
        hindi
            .file
            .unwrap()
            .path
            .ends_with("NotoSansDevanagari/NotoSansDevanagari-Bold.ttf")
    );
    let arabic =
        resolve_fallback(&shipped(), Fallback::Script(NotoScript::Arabic), 300, false).unwrap();
    assert!(
        arabic
            .file
            .unwrap()
            .path
            .ends_with("NotoSansArabic/NotoSansArabic-Regular.ttf")
    );
    // The last CJK fallback is the bundled Adobe NotDef, at any weight.
    let notdef = resolve_fallback(&shipped(), Fallback::Notdef, 700, true).unwrap();
    assert_eq!(
        (notdef.alias.as_str(), notdef.weight, notdef.italic),
        ("kl:fallback:notdef", 700, true)
    );
    assert!(
        notdef
            .file
            .unwrap()
            .path
            .ends_with("AdobeNotDef/AND-Regular.ttf")
    );
}

#[test]
fn a_missing_family_resolves_to_the_stand_in_and_says_so() {
    for missing in [
        FontRef {
            family: "Nowhere Sans".into(),
            source: FontSource::Installed,
        },
        FontRef {
            family: "Poppins".into(),
            source: FontSource::Google {
                version: "v22".into(),
            },
        },
        FontRef::bundled("Not Bundled"),
        // Adobe NotDef is a fallback only: naming it is a missing font.
        FontRef::bundled("Adobe NotDef"),
    ] {
        let r = resolve(&shipped(), &missing, 700, false);
        assert_eq!(
            (r.face.alias.as_str(), r.face.weight, r.stand_in),
            ("kl:b:inter", 700, true),
            "{missing:?}"
        );
    }
    // A broken install (no bundled folder) still answers: the system sans-serif, with no file.
    let r = resolve(&FontIndex::shipped(None), &FontRef::inter(), 700, false);
    assert_eq!(
        (r.face.alias.as_str(), r.face.file, r.stand_in),
        ("sans-serif", None, true)
    );
}

/// A download is drawn only at the version the project names, and its coverage travels with it.
#[test]
fn a_download_resolves_at_its_exact_version_with_its_coverage() {
    use klipolo_fonts::cjk::CjkCover;
    use klipolo_fonts::index::{Downloaded, DownloadedFace};
    use klipolo_fonts::resolve::FaceFile;
    let file = |name: &str| FaceFile {
        path: format!("/fonts/google/noto-sans-jp/v52/{name}").into(),
        index: 0,
    };
    let index = shipped().with_downloaded(vec![Downloaded {
        family: "Noto Sans JP".into(),
        version: "v52".into(),
        faces: vec![
            DownloadedFace {
                weight: 400,
                italic: false,
                file: file("400.ttf"),
            },
            DownloadedFace {
                weight: 700,
                italic: false,
                file: file("700.ttf"),
            },
        ],
        cover: CjkCover {
            han: true,
            kana: true,
            hangul: false,
        },
    }]);
    let at = |version: &str| FontRef {
        family: "Noto Sans JP".into(),
        source: FontSource::Google {
            version: version.into(),
        },
    };
    let r = resolve(&index, &at("v52"), 600, false);
    assert_eq!(
        (r.face.alias.as_str(), r.face.weight, r.stand_in),
        ("kl:g:noto-sans-jp:v52", 700, false)
    );
    assert_eq!(r.face.file, Some(file("700.ttf")));
    assert_eq!(
        index.cjk_cover(&r.face),
        CjkCover {
            han: true,
            kana: true,
            hangul: false
        }
    );
    // Another version is missing, never drawn in the one downloaded.
    let other = resolve(&index, &at("v51"), 600, false);
    assert!(other.stand_in);
    assert_eq!(index.cjk_cover(&other.face), CjkCover::NONE);
    // The webview names the face by its id: found while downloaded, gone once the download is replaced.
    let id = klipolo_fonts::index::face_id(&file("700.ttf"));
    assert_eq!(index.face_by_id(&id), Some(&r.face));
    assert_eq!(
        index.face_by_id("/fonts/google/noto-sans-jp/v52/700.ttf"),
        None,
        "a path without its index is no id"
    );
    assert_eq!(index.with_downloaded(Vec::new()).face_by_id(&id), None);
}

/// A downloaded Noto Sans is its script's CJK fallback, declared at every weight and style asked,
/// each from its nearest real face; another script's stays None (never a system face); nor is any other family one.
#[test]
fn a_downloaded_noto_is_its_scripts_cjk_fallback_at_every_weight() {
    use klipolo_fonts::cjk::CjkCover;
    use klipolo_fonts::fallback::Fallback;
    use klipolo_fonts::index::{Downloaded, DownloadedFace};
    use klipolo_fonts::resolve::{FaceFile, resolve_fallback};
    use klipolo_model::text_language::CjkScript;
    let file = |name: &str| FaceFile {
        path: format!("/fonts/google/noto-sans-jp/v52/{name}").into(),
        index: 0,
    };
    let face = |weight: u16, name: &str| DownloadedFace {
        weight,
        italic: false,
        file: file(name),
    };
    let noto = |family: &str, faces: Vec<DownloadedFace>| Downloaded {
        family: family.into(),
        version: "v52".into(),
        faces,
        cover: CjkCover::NONE,
    };
    let index = shipped().with_downloaded(vec![
        noto(
            "Noto Sans JP",
            vec![face(400, "400.ttf"), face(700, "700.ttf")],
        ),
        noto("Notable Sans JP", vec![face(400, "other.ttf")]),
    ]);
    for (weight, want) in [
        (100, "400.ttf"),
        (500, "400.ttf"),
        (600, "700.ttf"),
        (900, "700.ttf"),
    ] {
        for italic in [false, true] {
            let f = resolve_fallback(&index, Fallback::Cjk(CjkScript::Jp), weight, italic).unwrap();
            assert_eq!(
                (f.alias.as_str(), f.weight, f.italic, f.file),
                ("kl:fallback:jp", weight, italic, Some(file(want))),
                "{weight} {italic}"
            );
        }
    }
    assert_eq!(
        resolve_fallback(&index, Fallback::Cjk(CjkScript::Sc), 400, false),
        None
    );
}
