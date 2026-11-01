use klipolo_fonts::google::Catalogue;

fn fixture() -> Catalogue {
    Catalogue::from_json(include_bytes!("../catalogue/google-fonts.fixture.json")).unwrap()
}

#[test]
fn the_fixture_parses_with_variants_sizes_and_licences() {
    let c = fixture();
    let m = c.find("montserrat").unwrap();
    assert_eq!(m.licence, "ofl");
    assert!(m.files.iter().any(|f| f.weight == 900 && !f.italic));
    assert!(
        m.files.iter().any(|f| f.weight == 400 && f.italic),
        "\"italic\" is 400 italic"
    );
    assert!(c.family_bytes(m) > 1_000_000);
    assert!(
        m.files.iter().all(|f| !f.url_tail.starts_with("http")),
        "tails only; the host is fixed in code"
    );
}

/// The catalogue is a value in the index: no global, no test override.
fn index() -> klipolo_fonts::index::FontIndex {
    klipolo_fonts::index::FontIndex::shipped(klipolo_fonts::bundled::bundled_dir())
        .with_catalogue(std::sync::Arc::new(fixture()))
}

#[test]
fn google_families_join_search_after_the_bundled_and_resolve_by_name() {
    let ix = index();
    let rows = klipolo_fonts::search::search(&ix, "pop", &Default::default(), 200);
    let poppins = rows.iter().find(|r| r.family == "Poppins").unwrap();
    assert_eq!(poppins.state, klipolo_fonts::search::RowState::Download);
    assert!(poppins.size_bytes.unwrap() > 0);
    assert!(matches!(
        ix.resolve_family("poppins").unwrap().source,
        klipolo_model::font::FontSource::Google { .. }
    ));
    // A bundled family keeps its bundled source even though Google has it too.
    assert_eq!(
        ix.resolve_family("montserrat").unwrap().source,
        klipolo_model::font::FontSource::Bundled
    );
}

#[test]
fn covers_my_text_uses_the_catalogues_subsets() {
    // Abril Fatface (latin, latin-ext only) is a Google row, not bundled: Cyrillic text leaves it out.
    let f = klipolo_fonts::search::SearchFilter {
        category: None,
        covers: Some("Привет".into()),
        want: None,
        ..Default::default()
    };
    let google: Vec<String> = klipolo_fonts::search::search(&index(), "", &f, 200)
        .into_iter()
        .filter(|r| matches!(r.source, klipolo_model::font::FontSource::Google { .. }))
        .map(|r| r.family)
        .collect();
    assert!(!google.contains(&"Abril Fatface".to_owned()), "{google:?}");
    assert!(
        google.contains(&"Roboto".to_owned()),
        "Roboto has Cyrillic: {google:?}"
    );
}

#[test]
fn the_fixture_is_marked_as_a_sample() {
    assert!(fixture().sample);
}

// More catalogue cases: ordering, picks, downloads and the snapshot's rules.

use klipolo_fonts::index::{Downloaded, DownloadedFace, FontIndex, InstalledFamily};
use klipolo_fonts::search::{RowState, SearchFilter, search};
use klipolo_model::font::{FontRef, FontSource};

fn google(family: &str, version: &str) -> FontRef {
    FontRef {
        family: family.into(),
        source: FontSource::Google {
            version: version.into(),
        },
    }
}

/// The empty query's order, a tie at one rank: the bundled set, then this computer's, then Google's families not
/// downloaded, by popularity (none Klipolo bundles). A tie breaks bundled, downloaded Google, installed, then Google
/// not downloaded (the menu's sections show the same order). A query ranks by match first, then the same order.
#[test]
fn an_empty_query_lists_bundled_then_this_macs_then_google_by_popularity() {
    let ix = index().with_installed(vec![InstalledFamily {
        family: "Avenir Next".into(),
        alias: "Avenir Next".into(),
        faces: vec![],
        coverage: 0,
    }]);
    let rows = search(&ix, "", &SearchFilter::default(), usize::MAX);
    let states: Vec<RowState> = rows.iter().map(|r| r.state).collect();
    let first_google = states
        .iter()
        .position(|s| *s == RowState::Download)
        .unwrap();
    assert_eq!(
        rows[12].family, "Avenir Next",
        "the twelve bundled families, then this computer's"
    );
    assert_eq!(first_google, 13);
    assert_eq!(rows[13].family, "Roboto", "then Google's most popular");
    let names: Vec<&str> = rows.iter().map(|r| r.family.as_str()).collect();
    for bundled in ["Montserrat", "Inter", "Playfair Display", "TikTok Sans"] {
        assert_eq!(
            names.iter().filter(|n| **n == bundled).count(),
            1,
            "{bundled} only as the bundled row"
        );
    }
    // 34 in the fixture, 12 of them bundled.
    assert_eq!(
        states.iter().filter(|s| **s == RowState::Download).count(),
        22
    );
    // "noto" matches four Google families at the same rank: most popular first.
    let noto: Vec<String> = search(&ix, "noto sans", &SearchFilter::default(), 200)
        .into_iter()
        .map(|r| r.family)
        .take(4)
        .collect();
    assert_eq!(
        noto,
        [
            "Noto Sans JP",
            "Noto Sans KR",
            "Noto Sans SC",
            "Noto Sans TC"
        ]
    );
}

/// A Latin family downloads whole; a family over 15 MB (CJK) shows its regular weight's size, as it downloads a weight
/// at a time.
#[test]
fn a_download_is_the_whole_family_up_to_15_mb_else_its_regular_weight() {
    let ix = index();
    let size = |name: &str| {
        search(&ix, name, &SearchFilter::default(), 1)[0]
            .size_bytes
            .unwrap()
    };
    let c = fixture();
    assert_eq!(size("Poppins"), c.family_bytes(c.find("poppins").unwrap()));
    let jp = c.find("noto sans jp").unwrap();
    assert!(c.family_bytes(jp) > 15_000_000);
    let regular = jp
        .files
        .iter()
        .find(|f| f.weight == 400 && !f.italic)
        .unwrap();
    assert_eq!(size("Noto Sans JP"), regular.bytes);
    assert!((5_000_000..6_000_000).contains(&regular.bytes));
}

/// A picked Google family snaps to a weight it has, from its files; a Google family downloaded here at the
/// catalogue's version is listed as downloaded.
#[test]
fn a_google_pick_snaps_to_a_weight_it_has_and_a_download_says_downloaded() {
    let ix = index();
    assert_eq!(
        ix.snap_weight(&google("Abril Fatface", "v24"), 700, false),
        (400, false)
    );
    assert_eq!(
        ix.snap_weight(&google("Poppins", "v23"), 800, true),
        (800, true)
    );
    assert_eq!(
        ix.snap_weight(&google("Poppins", "v1"), 800, true),
        (800, true),
        "another version: as asked"
    );
    let downloaded = ix.with_downloaded(vec![Downloaded {
        family: "Poppins".into(),
        version: "v23".into(),
        faces: vec![DownloadedFace {
            weight: 400,
            italic: false,
            file: klipolo_fonts::resolve::FaceFile {
                path: "/d/poppins/400.ttf".into(),
                index: 0,
            },
        }],
        cover: klipolo_fonts::cjk::CjkCover::NONE,
    }]);
    let row = search(&downloaded, "poppins", &SearchFilter::default(), 1).remove(0);
    assert_eq!((row.state, row.size_bytes), (RowState::Downloaded, None));
}

/// The snapshot is checked as it is read: nothing odd reaches a project or a download URL.
#[test]
fn a_snapshot_that_breaks_a_rule_is_refused() {
    let one = |row: &str| format!(r#"{{"date":"2026-10-22","sample":false,"families":[{row}]}}"#);
    let row = |v: &str, l: &str, tail: &str| {
        format!(
            r#"{{"f":"X Sans","slug":"xsans","c":"sans","p":0,"t":0,"v":"{v}","s":["latin"],"l":"{l}","files":[[400,false,"{tail}",10]],"menu":""}}"#
        )
    };
    assert!(Catalogue::from_json(one(&row("v1", "ofl", "xsans/v1/a.ttf")).as_bytes()).is_ok());
    for bad in [
        row("1", "ofl", "xsans/v1/a.ttf"),
        row("v1", "gpl", "xsans/v1/a.ttf"),
        row("v1", "ofl", "https://evil.example/a.ttf"),
        row("v1", "ofl", "../a.ttf"),
        row("v1", "ofl", "/xsans/a.ttf"),
    ] {
        let e = Catalogue::from_json(one(&bad).as_bytes()).unwrap_err();
        assert!(e.contains("X Sans"), "{e}");
    }
    assert!(
        Catalogue::from_json(br#"{"date":"2026-10-22","families":[]}"#).is_err(),
        "sample is required"
    );
    // A name carrying bidi controls, which could make one name read as another, and one
    // face listed twice are refused too, here and by the builder and scanner (`scripts/font-catalogue-rules.mjs`).
    let file = r#"[[400,false,"x/v1/a.ttf",10]]"#;
    let named = |name: &str, files: &str| {
        one(&format!(
            r#"{{"f":"{name}","slug":"x","c":"sans","p":0,"t":0,"v":"v1","s":["latin"],"l":"ofl","files":{files},"menu":""}}"#
        ))
    };
    assert!(Catalogue::from_json(named("X Sans", file).as_bytes()).is_ok());
    for bad in [
        named(r"X \u202eSans", file),
        named(r"X \u2066Sans", file),
        named(r"X \u200fSans", file),
        // The Arabic letter mark is a bidi control too.
        named(r"X \u061cSans", file),
        named(
            "X Sans",
            r#"[[400,false,"x/v1/a.ttf",10],[400,false,"x/v1/b.ttf",10]]"#,
        ),
    ] {
        assert!(Catalogue::from_json(bad.as_bytes()).is_err(), "{bad}");
    }
}

/// The compiled-in snapshot reads and is either the labelled sample or a real one, never anything else.
#[test]
fn the_shipped_snapshot_parses() {
    let shipped = Catalogue::from_gz(include_bytes!("../catalogue/google-fonts.json.gz")).unwrap();
    assert!(
        shipped.sample || shipped.families.len() >= 1_000,
        "{}",
        shipped.families.len()
    );
    let c = klipolo_fonts::google::catalogue();
    assert_eq!(c.info().families as usize, c.families.len());
    assert!(c.find("poppins").is_some());
    let index = FontIndex::shipped(None).with_catalogue(klipolo_fonts::google::shipped());
    assert!(
        std::sync::Arc::ptr_eq(
            index.catalogue().unwrap(),
            &klipolo_fonts::google::shipped()
        ),
        "one copy"
    );
}
