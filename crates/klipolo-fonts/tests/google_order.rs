//! Google's families among the others: which family a name resolves to, and a typed query's cap, which never lets one
//! source crowd another out.

use klipolo_fonts::google::Catalogue;
use klipolo_fonts::index::{Downloaded, FontIndex, InstalledFamily};
use klipolo_fonts::search::{RowState, SearchFilter, search};
use klipolo_model::font::{FontRef, FontSource};

/// The bundled set and the fixture catalogue.
fn index() -> FontIndex {
    let fixture =
        Catalogue::from_json(include_bytes!("../catalogue/google-fonts.fixture.json")).unwrap();
    FontIndex::shipped(klipolo_fonts::bundled::bundled_dir())
        .with_catalogue(std::sync::Arc::new(fixture))
}

fn google(family: &str, version: &str) -> FontRef {
    FontRef {
        family: family.into(),
        source: FontSource::Google {
            version: version.into(),
        },
    }
}

/// A name never resolves to a Google family not downloaded while one of that name is here. Bundled, then a downloaded
/// Google family, then this computer's, then Google's catalogue.
#[test]
fn a_name_resolves_bundled_then_downloaded_then_installed_then_the_catalogue() {
    let here = |name: &str| InstalledFamily {
        family: name.into(),
        alias: name.into(),
        faces: vec![],
        coverage: 0,
    };
    let ix = index()
        .with_installed(vec![here("Roboto"), here("Lato"), here("Montserrat")])
        .with_downloaded(vec![Downloaded {
            family: "Lato".into(),
            version: "v25".into(),
            faces: vec![],
            cover: klipolo_fonts::cjk::CjkCover::NONE,
        }]);
    assert_eq!(
        ix.resolve_family("montserrat"),
        Some(FontRef::bundled("Montserrat"))
    );
    assert_eq!(
        ix.resolve_family("LATO"),
        Some(google("Lato", "v25")),
        "a download before this Mac's"
    );
    assert_eq!(
        ix.resolve_family("roboto").map(|f| f.source),
        Some(FontSource::Installed),
        "this Mac's before a Google family not downloaded"
    );
    assert_eq!(
        ix.resolve_family("poppins"),
        Some(google("Poppins", "v23")),
        "the catalogue last"
    );
}

/// A typed query caps each source separately, so 250 Google families matching "sans" never crowd this
/// computer's out of the answer (and the menu's "On this Mac" section out of the list).
#[test]
fn a_typed_query_caps_each_source_so_this_macs_families_stay() {
    let rows: Vec<String> = (0..250)
        .map(|i| {
            format!(
                r#"{{"f":"Family {i} Sans","slug":"family{i}sans","c":"sans","p":{i},"t":{i},"v":"v1","s":["latin"],"l":"ofl","files":[[400,false,"family{i}sans/v1/a.ttf",1000]],"menu":""}}"#
            )
        })
        .collect();
    let json = format!(
        r#"{{"date":"2026-10-23","sample":true,"families":[{}]}}"#,
        rows.join(",")
    );
    let ix = FontIndex::shipped(klipolo_fonts::bundled::bundled_dir())
        .with_catalogue(std::sync::Arc::new(
            Catalogue::from_json(json.as_bytes()).unwrap(),
        ))
        .with_installed(vec![InstalledFamily {
            family: "Gill Sans".into(),
            alias: "Gill Sans".into(),
            faces: vec![],
            coverage: 0,
        }]);
    let found = search(&ix, "sans", &SearchFilter::default(), 200);
    assert!(
        found.iter().any(|r| r.family == "Gill Sans"),
        "this Mac's family stays"
    );
    let google = found
        .iter()
        .filter(|r| r.state == RowState::Download)
        .count();
    assert_eq!(google, 200, "Google's own cap");
}

/// A rank tie breaks as a name resolves: bundled, then a downloaded Google family, then this computer's, then a Google
/// family not downloaded. So "roboto" typed in the menu leads with this Mac's Roboto, as chat's `styleText "Roboto"`
/// stores it, and a downloaded Lato leads this Mac's; a better rank still comes first.
#[test]
fn a_rank_tie_breaks_bundled_downloaded_installed_then_google() {
    let here = |name: &str| InstalledFamily {
        family: name.into(),
        alias: name.into(),
        faces: vec![],
        coverage: 0,
    };
    let ix = index()
        .with_installed(vec![here("Roboto"), here("Lato"), here("Palatino")])
        .with_downloaded(vec![downloaded("Lato", "v25")]);
    let first = |q: &str| {
        let row = search(&ix, q, &SearchFilter::default(), 200).remove(0);
        (row.family, row.state)
    };
    assert_eq!(first("roboto"), ("Roboto".into(), RowState::Installed));
    assert_eq!(first("lato"), ("Lato".into(), RowState::Downloaded));
    let ix = index().with_installed(vec![here("Palatino")]);
    let lato = search(&ix, "lato", &SearchFilter::default(), 200).remove(0);
    assert_eq!(
        (lato.family.as_str(), lato.state),
        ("Lato", RowState::Download),
        "rank first"
    );
}

/// A family downloaded only at versions older than the catalogue's resolves to the newest of them,
/// which draws, never the catalogue's, which isn't downloaded.
#[test]
fn a_family_downloaded_only_at_older_versions_resolves_to_the_newest_download() {
    let ix = index().with_downloaded(vec![
        downloaded("Lato", "v20"),
        downloaded("Lato", "v23"),
        downloaded("Lato", "v21"),
    ]);
    assert_eq!(ix.resolve_family("lato"), Some(google("Lato", "v23")));
}

fn downloaded(family: &str, version: &str) -> Downloaded {
    Downloaded {
        family: family.into(),
        version: version.into(),
        faces: vec![],
        cover: klipolo_fonts::cjk::CjkCover::NONE,
    }
}

/// The older-download row: a family downloaded only at a version older than the catalogue's is listed as that download,
/// which a pick stores and which draws, never as "Download" of the newer one; at equal rank it ties as a downloaded
/// family.
#[test]
fn a_family_downloaded_only_at_an_older_version_is_listed_as_that_download() {
    let ix = index().with_downloaded(vec![downloaded("Lato", "v20"), downloaded("Lato", "v23")]);
    let row = search(&ix, "lato", &SearchFilter::default(), 200).remove(0);
    assert_eq!(
        (row.family.as_str(), row.state, row.size_bytes),
        ("Lato", RowState::Downloaded, None)
    );
    assert_eq!(row.source, google("Lato", "v23").source);
    assert_eq!(
        ix.resolve_family("Lato"),
        Some(google("Lato", "v23")),
        "as a name resolves"
    );
}
