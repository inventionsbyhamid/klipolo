//! The engine's side of the one face answer: it shapes with exactly the face
//! the resolver picks, its fallbacks answer at every weight and style, CJK follows the text's language and draws
//! Adobe NotDef's boxes until its Noto Sans is downloaded, and a download resolves through the engine's own index.

use klipolo_engine::render::fonts::{
    self, AlreadyConfigured, FontDirs, configure_for_tests, family_for, test_fallback_face,
    test_index, test_shape_face, test_system_fallback_face,
};
use klipolo_fonts::cjk::CjkCover;
use klipolo_model::font::{FontRef, FontSource};

#[test]
fn the_engine_shapes_with_the_resolved_face_never_a_fallback_family() {
    configure_for_tests();
    let index = test_index();
    let choice = family_for(&index, &FontRef::bundled("Montserrat"), 700, false);
    assert_eq!(
        (choice.family.as_str(), choice.weight, choice.stand_in),
        ("kl:b:montserrat", 900, false)
    );
    // Shaping "Hello" with that choice uses Montserrat's face id, not Inter's (cosmic-text matches exact weights only).
    let (family_of_face, weight_of_face) = test_shape_face(&choice, "Hello");
    assert_eq!(
        (family_of_face.as_str(), weight_of_face),
        ("kl:b:montserrat", 900)
    );
}

#[test]
fn fallback_families_answer_every_weight_and_style() {
    configure_for_tests();
    // Bebas Neue has no Cyrillic: its Cyrillic letters come from the Latin fallback (Inter) at every weight and style,
    // never from an arbitrary face that happens to have that weight.
    for weight in (100..=900).step_by(100) {
        for italic in [false, true] {
            let (family, _) = test_fallback_face("kl:b:bebas-neue", "Привет", weight, italic);
            assert_eq!(family, "kl:fallback:latin", "{weight} {italic}");
        }
    }
}

#[test]
fn japanese_kanji_follow_the_texts_language() {
    use klipolo_engine::render::script_runs::runs;
    let ja = runs("今日は予算", "kl:b:inter", "ja", CjkCover::NONE);
    let zh = runs("今日是预算", "kl:b:inter", "zh-Hans", CjkCover::NONE);
    assert!(ja.iter().all(|(_, f)| &**f == "kl:fallback:jp"));
    assert!(zh.iter().all(|(_, f)| &**f == "kl:fallback:sc"));
    let mixed = runs("Hi 東京", "kl:b:montserrat", "ja", CjkCover::NONE);
    assert_eq!(
        mixed.iter().map(|(_, f)| &**f).collect::<Vec<_>>(),
        ["kl:b:montserrat", "kl:fallback:jp"]
    );
    // A Japanese title in an English project is Japanese, kanji included, because its run holds kana.
    assert!(
        runs("今日は予算", "kl:b:inter", "en", CjkCover::NONE)
            .iter()
            .all(|(_, f)| &**f == "kl:fallback:jp")
    );
    // A picked face that has these characters draws them itself (an installed Hiragino, say).
    let own = runs(
        "今日は",
        "Hiragino Sans",
        "en",
        CjkCover {
            han: true,
            kana: true,
            hangul: false,
        },
    );
    assert_eq!(
        own.iter().map(|(_, f)| &**f).collect::<Vec<_>>(),
        ["Hiragino Sans"]
    );
}

#[test]
fn cjk_without_its_noto_draws_notdefs_boxes_at_every_weight() {
    configure_for_tests();
    // Before a download the CJK aliases don't exist, so cosmic-text reaches Adobe NotDef, never another face.
    for weight in (100..=900).step_by(100) {
        let (family, _) = test_fallback_face("kl:fallback:jp", "今日。", weight, false);
        assert_eq!(family, "kl:fallback:notdef", "{weight}");
    }
    for script in klipolo_model::text_language::CjkScript::ALL {
        assert!(!fonts::cjk_registered(script), "{script:?}");
    }
    // Other scripts never reach it: Thai keeps the system's fallback (none in this managed-only test system).
    let (thai, _) = test_fallback_face("kl:b:inter", "สวัสดี", 400, false);
    assert_ne!(thai, "kl:fallback:notdef");
}

/// A download registered with the engine reaches the engine's own
/// index, with its exact version and its coverage, and draws for real: the app's preflight passes only when the engine
/// will draw.
#[test]
fn a_download_resolves_through_the_engines_own_index() {
    use klipolo_fonts::index::{Downloaded, DownloadedFace};
    use klipolo_fonts::resolve::FaceFile;
    configure_for_tests();
    // A downloaded family's file (a bundled file stands in for Google's bytes) that covers Han and kana.
    let font = |version: &str| FontRef {
        family: "Testa Sans".into(),
        source: FontSource::Google {
            version: version.into(),
        },
    };
    let path = test_index()
        .bundled_dir()
        .unwrap()
        .join("Caveat/Caveat-Bold.ttf");
    fonts::register(vec![Downloaded {
        family: "Testa Sans".into(),
        version: "v3".into(),
        faces: vec![DownloadedFace {
            weight: 700,
            italic: false,
            file: FaceFile { path, index: 0 },
        }],
        cover: CjkCover {
            han: true,
            kana: true,
            hangul: false,
        },
    }]);
    let choice = family_for(&fonts::index(), &font("v3"), 400, false);
    assert_eq!(
        (choice.family.as_str(), choice.weight, choice.stand_in),
        ("kl:g:testa-sans:v3", 700, false)
    );
    assert_eq!(
        choice.cjk,
        CjkCover {
            han: true,
            kana: true,
            hangul: false
        }
    );
    assert_eq!(
        test_shape_face(&choice, "Hello"),
        ("kl:g:testa-sans:v3".to_string(), 700)
    );
    // Another version is missing, and so is this one once it is removed.
    assert!(family_for(&fonts::index(), &font("v2"), 400, false).stand_in);
    fonts::unregister("kl:g:testa-sans:v3");
    assert!(family_for(&fonts::index(), &font("v3"), 400, false).stand_in);
}

/// The fonts are configured once; a second call is an error for its caller to log, never a silent swap.
#[test]
fn the_fonts_are_configured_once() {
    configure_for_tests();
    assert_eq!(
        fonts::configure(FontDirs::default()),
        Err(AlreadyConfigured)
    );
    assert!(fonts::index().bundled_dir().is_some());
}

/// An installed family covering Han, kana or Hangul goes on cosmic-text's forbidden list, so
/// its last stage never borrows PingFang or Hiragino for CJK text that didn't pick them.
#[test]
fn installed_cjk_families_are_never_borrowed_as_fallbacks() {
    use klipolo_fonts::coverage::needs;
    use klipolo_fonts::index::InstalledFamily;
    let fam = |name: &str, text: &str| InstalledFamily {
        family: name.into(),
        alias: name.into(),
        faces: vec![],
        coverage: needs(text),
    };
    let list = klipolo_engine::render::font_fallback::cjk_forbidden(&[
        fam("PingFang SC", "今日"),
        fam("Hiragino Sans", "ひらがな"),
        fam("Apple SD Gothic Neo", "한국어"),
        fam("Avenir Next", "Hello"),
    ]);
    assert_eq!(
        list,
        ["PingFang SC", "Hiragino Sans", "Apple SD Gothic Neo"]
    );
}

/// Rubik ships only Black Italic. It is shaped upright (its faces are registered upright too),
/// so the system's script faces, which are upright, still answer for the Thai it lacks; asked italic, none would.
#[test]
fn an_italic_only_face_keeps_the_systems_script_fallbacks() {
    configure_for_tests();
    let choice = family_for(&test_index(), &FontRef::bundled("Rubik"), 900, true);
    assert_eq!(
        (choice.family.as_str(), choice.weight, choice.italic),
        ("kl:b:rubik", 900, false)
    );
    assert_eq!(
        test_shape_face(&choice, "Hello").0,
        "kl:b:rubik",
        "its upright twin draws its own letters"
    );
    let (thai, _) = test_system_fallback_face("kl:b:rubik", "สวัสดี", 900, false);
    assert_eq!(thai, "Test Thai", "the system's upright Thai face answers");
    let (boxed, _) = test_system_fallback_face("kl:b:rubik", "สวัสดี", 900, true);
    assert_eq!(
        boxed, "kl:b:rubik",
        "asked italic, no system face answers: why it is asked upright"
    );
}

/// Punctuation standing alone ("Hello！") asks for the text language's Noto Sans, else Adobe
/// NotDef, as the webview's stack does; never cosmic-text's last stage, where the engine and WebKit would each pick a
/// needs no download: no frame code.
#[test]
fn punctuation_standing_alone_draws_the_same_box_as_the_webview_until_its_noto() {
    use klipolo_engine::render::script_runs::runs;
    configure_for_tests();
    let r = runs("Hello！", "kl:b:inter", "en", CjkCover::NONE);
    assert_eq!(
        r.iter()
            .map(|(range, f)| (&"Hello！"[range.clone()], &**f))
            .collect::<Vec<_>>(),
        [("Hello", "kl:b:inter"), ("！", "kl:fallback:notdef")]
    );
    for weight in [400, 700, 900] {
        let (face, _) = test_fallback_face("kl:fallback:notdef", "！", weight, false);
        assert_eq!(face, "kl:fallback:notdef", "{weight}");
    }
    // A face that covers Han draws it itself (a picked Hiragino).
    let own = CjkCover {
        han: true,
        kana: true,
        hangul: false,
    };
    assert_eq!(runs("Hello！", "Hiragino Sans", "en", own).len(), 1);
}

/// The scan's grouping: a family's faces together, each with its file, weight and style, and the family
/// with the scripts its faces cover, read once from the files.
#[test]
fn the_scan_groups_faces_by_family_with_their_coverage() {
    use klipolo_fonts::coverage::{ARABIC, CJK, CYRILLIC, HEBREW, LATIN};
    let dir = test_index().bundled_dir().unwrap().to_path_buf();
    let files = [
        dir.join("Caveat/Caveat-Bold.ttf"),
        dir.join("Rubik/Rubik-BlackItalic.ttf"),
    ];
    let mut families = fonts::test_group_installed(&files);
    families.sort_by(|a, b| a.family.cmp(&b.family));
    let caveat = &families[0];
    assert_eq!(caveat.family, "Caveat");
    assert_eq!(
        (
            caveat.faces[0].weight,
            caveat.faces[0].italic,
            caveat.faces[0].path.clone()
        ),
        (700, false, files[0].clone())
    );
    assert_eq!(caveat.coverage & (LATIN | CYRILLIC), LATIN | CYRILLIC);
    assert_eq!(caveat.coverage & CJK, 0);
    let rubik = &families[1];
    assert_eq!(rubik.family, "Rubik");
    assert!(rubik.faces[0].italic && !rubik.faces[0].variable);
    assert_eq!(rubik.coverage & (ARABIC | HEBREW), ARABIC | HEBREW);
}

/// An installed face that isn't normal width (Impact, DIN Condensed) draws itself, never the Latin
/// fallback: cosmic-text keeps only faces of the asked width, so the drawer asks for its width twin, the same face
/// normal width under its own alias (it keeps the system's script fallbacks too).
#[test]
fn a_condensed_only_family_draws_its_face_not_inter() {
    configure_for_tests();
    let file = test_index()
        .bundled_dir()
        .unwrap()
        .join("BebasNeue/BebasNeue-Regular.ttf");
    let (choice, drawn) = fonts::test_installed_draw(
        &[("Test Condensed", file.clone(), 400, 3)],
        "Test Condensed",
        400,
    );
    assert_eq!((choice.stand_in, choice.stretch), (false, 5));
    assert_eq!(choice.family, "kl:w:Test Condensed:3");
    assert_eq!(
        drawn, choice.family,
        "its own face, never kl:fallback:latin"
    );
    // A family with both widths (Futura): its heavy weights snap to its normal faces, and its condensed faces draw
    // under their own entry, drawn by their width twin (the family's name plus the width).
    let normal = test_index()
        .bundled_dir()
        .unwrap()
        .join("Inter/Inter-Bold.ttf");
    let faces = [("Test Mixed", normal, 700, 5), ("Test Mixed", file, 800, 3)];
    let (choice, drawn) = fonts::test_installed_draw(&faces, "Test Mixed", 900);
    assert_eq!(
        (choice.weight, choice.stretch, drawn.as_str()),
        (700, 5, "Test Mixed")
    );
    let (choice, drawn) = fonts::test_installed_draw(&faces, "Test Mixed Condensed", 900);
    assert_eq!(
        (choice.weight, choice.stretch, drawn.as_str()),
        (800, 5, "kl:w:Test Mixed:3")
    );
}
