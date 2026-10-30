//! An installed family's widths and duplicate faces, as the menu and the resolver see them. The faces
//! are this Mac's (the review read their OS/2 tables): Futura and Helvetica Neue hold condensed faces among their
//! normal ones; Impact is condensed only; Avenir has two 400s (Book and Roman).

use klipolo_fonts::index::{FontIndex, InstalledFace, InstalledFamily};
use klipolo_fonts::installed::{ScannedFace, group_faces};
use klipolo_fonts::resolve::resolve;
use klipolo_model::font::{FontRef, FontSource};

/// `family`'s face at `(weight, italic, width class)`, the n-th in its file.
fn face(family: &str, n: u32, weight: u16, italic: bool, stretch: u16) -> ScannedFace {
    ScannedFace {
        family: family.into(),
        face: InstalledFace {
            weight,
            italic,
            stretch,
            path: format!("/System/Library/Fonts/{family}.ttc").into(),
            index: n,
            variable: false,
            bytes: 1000,
        },
        coverage: 0,
    }
}

fn mac() -> Vec<InstalledFamily> {
    group_faces(vec![
        face("Futura", 0, 500, false, 5),
        face("Futura", 1, 500, true, 5),
        face("Futura", 2, 700, false, 5),
        face("Futura", 3, 500, false, 3),
        face("Futura", 4, 800, false, 3),
        face("Helvetica Neue", 0, 400, false, 5),
        face("Helvetica Neue", 1, 700, false, 5),
        face("Helvetica Neue", 2, 700, false, 3),
        face("Helvetica Neue", 3, 900, false, 3),
        face("Impact", 0, 400, false, 3),
        face("Avenir", 0, 400, false, 5),
        face("Avenir", 1, 400, false, 5),
        face("Avenir", 2, 800, false, 5),
    ])
}

fn installed(family: &str) -> FontRef {
    FontRef {
        family: family.into(),
        source: FontSource::Installed,
    }
}

#[test]
fn a_familys_other_widths_are_their_own_entries_and_each_weight_is_listed_once() {
    let families = mac();
    let names: Vec<&str> = families.iter().map(|f| f.family.as_str()).collect();
    assert_eq!(
        names,
        [
            "Futura",
            "Futura Condensed",
            "Helvetica Neue",
            "Helvetica Neue Condensed",
            "Impact",
            "Avenir"
        ]
    );
    let weights = |name: &str| -> Vec<(u16, bool, u16)> {
        let f = families.iter().find(|f| f.family == name).unwrap();
        f.faces
            .iter()
            .map(|x| (x.weight, x.italic, x.stretch))
            .collect()
    };
    assert_eq!(
        weights("Futura"),
        [(500, false, 5), (500, true, 5), (700, false, 5)]
    );
    assert_eq!(
        weights("Futura Condensed"),
        [(500, false, 3), (800, false, 3)]
    );
    // Book and Roman are both 400: listed once, the first (cosmic-text's lowest-id pick).
    assert_eq!(weights("Avenir"), [(400, false, 5), (800, false, 5)]);
    assert_eq!(
        families
            .iter()
            .find(|f| f.family == "Avenir")
            .unwrap()
            .faces[0]
            .index,
        0
    );
    // A width split off a family is registered under the family's own name.
    let condensed = families
        .iter()
        .find(|f| f.family == "Futura Condensed")
        .unwrap();
    assert_eq!(condensed.alias, "Futura");
}

#[test]
fn heavy_futura_and_helvetica_neue_resolve_to_real_normal_faces() {
    let ix = FontIndex::shipped(None).with_installed(mac());
    // A bold-look title switched to Futura snaps to its Bold, never the condensed ExtraBold.
    for asked in [800, 900] {
        let r = resolve(&ix, &installed("Futura"), asked, false);
        assert_eq!(
            (
                r.face.alias.as_str(),
                r.face.weight,
                r.face.stretch,
                r.stand_in
            ),
            ("Futura", 700, 5, false)
        );
    }
    let r = resolve(&ix, &installed("Helvetica Neue"), 900, false);
    assert_eq!((r.face.weight, r.face.stretch, r.stand_in), (700, 5, false));
    // The condensed entry draws its own condensed faces, registered under "Futura".
    let r = resolve(&ix, &installed("Futura Condensed"), 900, false);
    assert_eq!(
        (r.face.alias.as_str(), r.face.weight, r.face.stretch),
        ("Futura", 800, 3)
    );
    assert_eq!(
        ix.weights(&installed("Futura")),
        [(500, false), (500, true), (700, false)]
    );
}

#[test]
fn a_condensed_only_family_keeps_its_name_and_its_width() {
    let ix = FontIndex::shipped(None).with_installed(mac());
    let r = resolve(&ix, &installed("Impact"), 400, false);
    assert_eq!(
        (r.face.alias.as_str(), r.face.stretch, r.stand_in),
        ("Impact", 3, false)
    );
}

/// A face over the cap is known as such from the scan's record, so nothing reads it.
#[test]
fn a_face_over_the_cap_is_marked_too_large() {
    let mut big = face("Huge Emoji", 0, 400, false, 5);
    big.face.bytes = 180 * 1024 * 1024;
    let ix = FontIndex::shipped(None)
        .with_installed(group_faces(vec![big, face("Small", 0, 400, false, 5)]));
    let r = resolve(&ix, &installed("Huge Emoji"), 400, false);
    assert!(ix.too_large(&r.face));
    let r = resolve(&ix, &installed("Small"), 400, false);
    assert!(!ix.too_large(&r.face));
}
