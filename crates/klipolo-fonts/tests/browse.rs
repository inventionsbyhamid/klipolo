//! The font menu's browse, windowed: section counts, then only the rows drawn; the pinned sections list a font only
//! while it is that very font; Google Fonts off lists only what's downloaded.

use std::sync::Arc;

use klipolo_fonts::browse::{BrowseAsk, Section, browse, browse_rows, row_for};
use klipolo_fonts::google::Catalogue;
use klipolo_fonts::index::{Downloaded, FontIndex, InstalledFamily};
use klipolo_fonts::search::{RowState, SearchFilter, search};
use klipolo_model::font::{FontRef, FontSource};

fn index() -> FontIndex {
    let fixture =
        Catalogue::from_json(include_bytes!("../catalogue/google-fonts.fixture.json")).unwrap();
    let here = |name: &str| InstalledFamily {
        family: name.into(),
        alias: name.into(),
        faces: vec![],
        coverage: 0,
    };
    FontIndex::shipped(klipolo_fonts::bundled::bundled_dir())
        .with_catalogue(Arc::new(fixture))
        .with_installed(
            (0..300)
                .map(|i| here(&format!("Mac Family {i:03}")))
                .collect(),
        )
        .with_downloaded(vec![Downloaded {
            family: "Lato".into(),
            version: "v25".into(),
            faces: vec![],
            cover: klipolo_fonts::cjk::CjkCover::NONE,
        }])
}

fn google(family: &str, version: &str) -> FontRef {
    FontRef {
        family: family.into(),
        source: FontSource::Google {
            version: version.into(),
        },
    }
}

#[test]
fn a_browse_says_its_sections_counts_and_sends_only_the_rows_asked_for() {
    let ix = index();
    let google_rows = search(&ix, "", &SearchFilter::default(), usize::MAX)
        .iter()
        .filter(|r| matches!(r.source, FontSource::Google { .. }))
        .count() as u32;
    let ask = BrowseAsk {
        project: vec![
            FontRef::bundled("Anton"),
            google("Lato", "v25"),
            // Another version than the one listed: not "In this project" (the missing-font check says it's missing).
            google("Poppins", "v1"),
        ],
        recent: vec![FontRef::bundled("Inter")],
        value: Some(FontRef::bundled("Inter")),
        ..BrowseAsk::default()
    };
    let b = browse(&ix, &ask);
    // The 12 bundled families come first, then the project's and the recent picks, this computer's and Google's.
    assert_eq!(
        b.sections,
        [
            (Section::Bundled, 12),
            (Section::Project, 2),
            (Section::Recent, 1),
            (Section::Installed, 300),
            (Section::Google, google_rows),
        ]
    );
    assert_eq!(
        b.value,
        Some(0),
        "the chosen font's first row: the bundled Inter"
    );
    let rows = browse_rows(&ix, &ask, 11, 15);
    assert_eq!(
        rows.iter().map(|r| r.family.as_str()).collect::<Vec<_>>(),
        ["JetBrains Mono", "Anton", "Lato", "Inter"]
    );
    assert_eq!(rows[2].state, RowState::Downloaded);
    let last = 2 + 1 + 12 + 300 + google_rows;
    let tail = browse_rows(&ix, &ask, last - 1, last + 50);
    assert_eq!(tail.len(), 1, "never past the end");
    assert_eq!(
        row_for(&ix, &google("Lato", "v25"), (400, false))
            .unwrap()
            .state,
        RowState::Downloaded
    );
}

#[test]
fn with_google_fonts_off_only_whats_downloaded_is_listed() {
    let ix = index();
    let off = SearchFilter {
        google_off: true,
        ..SearchFilter::default()
    };
    let ask = BrowseAsk {
        filter: off.clone(),
        project: vec![google(
            "Poppins",
            ix.catalogue()
                .unwrap()
                .find("poppins")
                .unwrap()
                .version
                .as_str(),
        )],
        ..BrowseAsk::default()
    };
    let b = browse(&ix, &ask);
    assert!(
        !b.sections.iter().any(|(s, _)| *s == Section::Project),
        "a download row isn't pinned"
    );
    assert_eq!(
        b.sections.last(),
        Some(&(Section::Google, 1)),
        "Lato, downloaded"
    );
    let typed = search(&ix, "lato", &off, 200);
    assert!(typed.iter().all(|r| r.state != RowState::Download));
    assert!(search(&ix, "poppins", &off, 200).is_empty());
}

/// A family downloaded a weight at a time (over 15 MB) offers the weights it can still gain, each
/// with its own size, in the menu's Weight menu; one downloaded whole offers none.
#[test]
fn a_family_downloaded_a_weight_at_a_time_offers_its_other_weights() {
    let face = |weight: u16| klipolo_fonts::index::DownloadedFace {
        weight,
        italic: false,
        file: klipolo_fonts::resolve::FaceFile {
            path: klipolo_fonts::bundled::bundled_dir()
                .unwrap()
                .join("Inter/Inter-Regular.ttf"),
            index: 0,
        },
    };
    let downloaded = |family: &str, version: &str| Downloaded {
        family: family.into(),
        version: version.into(),
        faces: vec![face(400)],
        cover: klipolo_fonts::cjk::CjkCover::NONE,
    };
    let index = index().with_downloaded(vec![
        downloaded("Noto Sans JP", "v54"),
        downloaded("Poppins", "v23"),
    ]);
    let jp = row_for(&index, &google("Noto Sans JP", "v54"), (400, false)).unwrap();
    assert_eq!(jp.faces, [(400, false)]);
    assert_eq!(jp.more_faces.len(), 8, "the other eight weights");
    let bold = jp.more_faces.iter().find(|f| f.weight == 700).unwrap();
    assert!(!bold.italic && bold.bytes > 5_000_000);
    let poppins = row_for(&index, &google("Poppins", "v23"), (400, false)).unwrap();
    assert!(
        poppins.more_faces.is_empty(),
        "a whole family: nothing more to fetch"
    );
}
