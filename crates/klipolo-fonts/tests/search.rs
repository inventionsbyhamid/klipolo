use klipolo_fonts::coverage::{Coverage, needs};
use klipolo_fonts::index::{FontIndex, InstalledFace, InstalledFamily};
use klipolo_fonts::search::{RowState, SearchFilter, search};
use klipolo_model::font::{FontRef, FontSource};

fn installed(name: &str, weights: &[u16], coverage: Coverage) -> InstalledFamily {
    InstalledFamily {
        family: name.into(),
        alias: name.into(),
        faces: weights
            .iter()
            .map(|&w| InstalledFace {
                weight: w,
                italic: false,
                stretch: 5,
                path: format!("/f/{name}.ttf").into(),
                index: 0,
                variable: false,
                bytes: 0,
            })
            .collect(),
        coverage,
    }
}

fn index(families: Vec<InstalledFamily>) -> FontIndex {
    FontIndex::shipped(klipolo_fonts::bundled::bundled_dir()).with_installed(families)
}

#[test]
fn bundled_comes_first_then_installed_and_names_match_loosely() {
    let ix = index(vec![
        installed("Avenir Next", &[400, 700], needs("Hello")),
        installed("Montserrat", &[400], needs("Hello")),
    ]);
    assert_eq!(
        ix.resolve_family("montserrat"),
        Some(FontRef::bundled("Montserrat")),
        "the bundled copy wins over an installed one"
    );
    assert_eq!(
        ix.resolve_family("Avenir next").map(|f| f.source),
        Some(FontSource::Installed)
    );
    let rows = search(&ix, "mont", &SearchFilter::default(), 200);
    assert_eq!(
        (rows[0].family.as_str(), rows[0].state),
        ("Montserrat", RowState::Bundled)
    );
    assert!(
        rows.iter()
            .any(|r| r.family == "Montserrat" && r.state == RowState::Installed)
    );
}

#[test]
fn an_empty_query_lists_the_bundled_set_then_this_macs() {
    let ix = index(vec![installed("Zapfino", &[400], needs("Hello"))]);
    let rows = search(&ix, "", &SearchFilter::default(), 200);
    assert_eq!(
        rows.iter()
            .take(12)
            .filter(|r| r.state == RowState::Bundled)
            .count(),
        12
    );
    assert_eq!(rows[12].family, "Zapfino");
}

#[test]
fn hidden_system_faces_never_show() {
    let ix = index(vec![
        installed(".SF NS", &[400], 0),
        installed("LastResort", &[400], 0),
        installed("Georgia", &[400], needs("Hello")),
    ]);
    assert!(
        search(&ix, "", &SearchFilter::default(), 200)
            .iter()
            .all(|r| !r.family.starts_with('.') && r.family != "LastResort")
    );
}

#[test]
fn covers_my_text_reads_the_coverage_computed_at_scan_time() {
    let ix = index(vec![
        installed("Latin Only", &[400], needs("Hello")),
        installed("Wide", &[400], needs("Hello Привет")),
    ]);
    let f = SearchFilter {
        category: None,
        covers: Some("Привет".into()),
        want: None,
        ..Default::default()
    };
    let names: Vec<String> = search(&ix, "", &f, 200)
        .into_iter()
        .filter(|r| r.state == RowState::Installed)
        .map(|r| r.family)
        .collect();
    assert_eq!(names, ["Wide"]);
}

#[test]
fn picking_a_family_snaps_to_a_weight_it_has() {
    let ix = index(vec![]);
    assert_eq!(
        ix.snap_weight(&FontRef::bundled("Montserrat"), 700, false),
        (900, false)
    );
    assert_eq!(
        ix.snap_weight(&FontRef::bundled("Inter"), 800, false),
        (700, false)
    );
}

/// A real-scale measurement: release, `#[ignore]`, the fastest of 5, so a busy shared host can't flake it.
#[test]
#[ignore = "release timing"]
fn searching_two_thousand_names_takes_under_5_ms() {
    let ix = index(
        (0..2300)
            .map(|i| installed(&format!("Family {i}"), &[400], needs("Hello")))
            .collect(),
    );
    let mut best = f64::MAX;
    for _ in 0..5 {
        let start = std::time::Instant::now();
        for q in ["fam", "family 12", "x", "inter"] {
            let _ = search(&ix, q, &SearchFilter::default(), 200);
        }
        best = best.min(start.elapsed().as_secs_f64() * 1e3 / 4.0);
    }
    println!("search: {best:.2} ms per query (fastest of 5)");
    assert!(best < 5.0, "{best:.2} ms");
}

#[test]
fn each_row_carries_the_weight_rust_snaps_to() {
    let ix = index(vec![]);
    let f = SearchFilter {
        want: Some((700, false)),
        ..SearchFilter::default()
    };
    let rows = search(&ix, "", &f, 200);
    let row = |n: &str| rows.iter().find(|r| r.family == n).unwrap().snapped;
    assert_eq!(row("Montserrat"), (900, false), "Montserrat has only Black");
    assert_eq!(row("Inter"), (700, false));
}

/// An installed family resolves to its own nearest face, under its own name (the scan registers it there), and the CJK
/// its coverage bits name is what it draws itself (a picked Hiragino draws its own kanji and kana).
#[test]
fn an_installed_family_resolves_to_its_nearest_face_and_covers_its_own_cjk() {
    use klipolo_fonts::cjk::CjkCover;
    use klipolo_fonts::resolve::resolve;
    let ix = index(vec![
        installed("Avenir Next", &[400, 600, 800], needs("Hello")),
        installed("Hiragino Sans", &[300, 600], needs("今日は")),
    ]);
    let avenir = FontRef {
        family: "Avenir Next".into(),
        source: FontSource::Installed,
    };
    let r = resolve(&ix, &avenir, 700, false);
    assert_eq!(
        (r.face.alias.as_str(), r.face.weight, r.stand_in),
        ("Avenir Next", 800, false)
    );
    let face_id = klipolo_fonts::index::face_id(r.face.file.as_ref().unwrap());
    assert_eq!(
        ix.face_by_id(&face_id),
        Some(&r.face),
        "its bytes can be asked for"
    );
    assert_eq!(ix.cjk_cover(&r.face), CjkCover::NONE);
    let hiragino = FontRef {
        family: "Hiragino Sans".into(),
        source: FontSource::Installed,
    };
    let r = resolve(&ix, &hiragino, 400, false);
    assert_eq!(
        ix.cjk_cover(&r.face),
        CjkCover {
            han: true,
            kana: true,
            hangul: false
        }
    );
    // The same name from another computer, not installed here, is the stand-in.
    let gone = FontRef {
        family: "Futura".into(),
        source: FontSource::Installed,
    };
    assert!(resolve(&ix, &gone, 400, false).stand_in);
    // Search never offers Adobe NotDef, nor does a name lookup.
    assert_eq!(ix.resolve_family("Adobe NotDef"), None);
    assert!(search(&ix, "adobe", &SearchFilter::default(), 200).is_empty());
}

/// A real-size measurement (release, `#[ignore]`, fastest of 5): a snapshot of 1,900 families and
/// 7,600 files decodes once, and a search over it and 2,300 installed families stays within the menu's 5 ms. The
/// browse (an empty query, every row built for the windowed menu) is timed too.
#[test]
#[ignore = "release timing"]
fn a_real_size_catalogue_decodes_once_and_searches_within_5_ms() {
    use std::io::Write;
    let families: Vec<String> = (0..1900)
        .map(|i| {
            let files: Vec<String> = [(400, false), (700, false), (400, true), (700, true)]
                .iter()
                .map(|(w, it)| format!(r#"[{w},{it},"family{i}/v{i}/AbCdEfGhIjKlMnOpQrStUvWx{w}{it}.ttf",123456]"#))
                .collect();
            format!(
                r#"{{"f":"Family {i}","slug":"family{i}","c":"sans","p":{i},"t":{i},"v":"v{i}","s":["cyrillic","latin","latin-ext"],"l":"ofl","files":[{}],"menu":"family{i}/v{i}/menu.ttf"}}"#,
                files.join(",")
            )
        })
        .collect();
    let json = format!(
        r#"{{"date":"2026-10-22","sample":false,"families":[{}]}}"#,
        families.join(",")
    );
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(json.as_bytes()).unwrap();
    let gz = gz.finish().unwrap();
    let mut decode = f64::MAX;
    let mut catalogue = None;
    for _ in 0..5 {
        let start = std::time::Instant::now();
        catalogue = Some(klipolo_fonts::google::Catalogue::from_gz(&gz).unwrap());
        decode = decode.min(start.elapsed().as_secs_f64() * 1e3);
    }
    let installed: Vec<InstalledFamily> = (0..2300)
        .map(|i| InstalledFamily {
            family: format!("Installed {i}"),
            alias: format!("Installed {i}"),
            faces: vec![],
            coverage: 0,
        })
        .collect();
    let ix = FontIndex::shipped(klipolo_fonts::bundled::bundled_dir())
        .with_installed(installed)
        .with_catalogue(std::sync::Arc::new(catalogue.unwrap()));
    let (mut query, mut browse) = (f64::MAX, f64::MAX);
    for _ in 0..5 {
        let start = std::time::Instant::now();
        for q in ["fam", "family 12", "x", "inter"] {
            let _ = search(&ix, q, &SearchFilter::default(), 200);
        }
        query = query.min(start.elapsed().as_secs_f64() * 1e3 / 4.0);
        let start = std::time::Instant::now();
        let rows = search(&ix, "", &SearchFilter::default(), usize::MAX);
        browse = browse.min(start.elapsed().as_secs_f64() * 1e3);
        assert_eq!(rows.len(), 12 + 1900 + 2300);
    }
    // What a browse sends the webview (`fonts_search`'s JSON), once per menu open or filter change.
    let browsed =
        serde_json::to_vec(&search(&ix, "", &SearchFilter::default(), usize::MAX)).unwrap();
    println!(
        "catalogue: {} B raw, {} B gz, decoded in {decode:.1} ms; search {query:.2} ms a query, browse {browse:.2} ms \
         and {} B of JSON (fastest of 5)",
        json.len(),
        gz.len(),
        browsed.len()
    );
    assert!(query < 5.0, "{query:.2} ms");
}
