//! SVG flattened into scene paths, in the render worker only. The SVG is untrusted: nothing it
//! names is opened, nothing in it expands, text is refused before usvg sees it, and every size is capped plainly.

use klipolo_engine::make::svg::{MAX_SVG_BYTES, flatten};
use klipolo_model::contrib::Catalog;
use klipolo_model::scene::path::parse_d;
use klipolo_model::scene::{LineCap, Paint, Scene, check_scene};

#[test]
fn a_logo_becomes_path_nodes_and_nothing_external_is_read() {
    let nodes = flatten(include_bytes!("fixtures/svg/logo.svg")).unwrap();
    assert!(!nodes.is_empty());
    assert!(nodes.iter().all(|n| n.props.d.is_some()));
    // `external-ref.svg` names `file:///etc/hosts` and an `http://` image: refused, never opened.
    // `with-text.svg` has a `<text>` element: refused before usvg sees it.
    let e = flatten(include_bytes!("fixtures/svg/external-ref.svg")).unwrap_err();
    assert!(e.iter().any(|p| p.code == "svg.external"));
    let e = flatten(include_bytes!("fixtures/svg/with-text.svg")).unwrap_err();
    assert!(e.iter().any(|p| p.code == "svg.text"));
    // Entity expansion bomb: refused by size or by the parser's own limits, quickly.
    let started = std::time::Instant::now();
    assert!(flatten(include_bytes!("fixtures/svg/bomb.svg")).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

/// The logo's three shapes keep their look: one view box (the SVG's size) so they fit their box together, the badge's
/// gradient across it in fractions of that box, the ring's stroke and the check's round caps; and the nodes are a
/// scene the checker accepts as stored.
#[test]
fn a_logo_keeps_its_paint_and_fits_one_view_box() {
    let nodes = flatten(include_bytes!("fixtures/svg/logo.svg")).unwrap();
    assert_eq!(nodes.len(), 3);
    assert!(
        nodes
            .iter()
            .all(|n| n.props.view_box == Some([0.0, 0.0, 200.0, 100.0]))
    );
    for n in &nodes {
        parse_d(n.props.d.as_deref().unwrap()).unwrap();
    }
    let Some(Paint::Gradient(g)) = &nodes[0].props.fill else {
        panic!("the badge's fill: {:?}", nodes[0].props.fill);
    };
    // The circle's box runs from x 10 to 90 of 200: its gradient from 0.05 to 0.45 of the view.
    let [x0, y0, x1, y1] = g.linear.unwrap();
    assert!(
        (x0 - 0.05).abs() < 1e-4 && (x1 - 0.45).abs() < 1e-4,
        "{:?}",
        g.linear
    );
    assert!(y0.abs() < 1e-4 && y1.abs() < 1e-4);
    assert_eq!(g.stops.len(), 2);
    assert_eq!(
        nodes[1].props.stroke,
        Some(Paint::Color("#2D6CDFFF".into()))
    );
    assert_eq!(nodes[1].props.fill, Some(Paint::Color("none".into())));
    assert_eq!(nodes[2].props.cap, Some(LineCap::Round));
    let scene = serde_json::json!({ "scene": 1, "duration": 2, "nodes": nodes });
    let stored = Scene::from_stored(scene.to_string().as_bytes()).unwrap();
    assert!(check_scene(&stored, Catalog::builtin()).is_ok());
}

/// Hostile SVGs are refused plainly, each quickly: more than 1 MiB, a nest of `<use>` that would copy
/// itself a million times, elements nested a hundred deep, a path past 4,096 commands, a script, a clip path, svgz.
#[test]
fn hostile_svgs_are_refused_with_their_reason() {
    let code = |svg: &[u8]| {
        let started = std::time::Instant::now();
        let e = flatten(svg).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        e[0].code
    };
    let wrap = |body: &str| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10">{body}</svg>"#
        )
    };
    let mut big = wrap("");
    big.push_str(&" ".repeat(MAX_SVG_BYTES));
    assert_eq!(code(big.as_bytes()), "svg.tooLarge");
    // Each level uses the one before ten times: 10⁶ copies of one rect.
    let mut uses = String::from(r##"<defs><g id="l0"><rect width="1" height="1"/></g>"##);
    for level in 1..=6 {
        uses.push_str(&format!(r#"<g id="l{level}">"#));
        for _ in 0..10 {
            uses.push_str(&format!(r##"<use href="#l{}"/>"##, level - 1));
        }
        uses.push_str("</g>");
    }
    uses.push_str(r##"</defs><use href="#l6"/>"##);
    assert_eq!(code(wrap(&uses).as_bytes()), "svg.tooLarge");
    let deep = format!(
        "{}<rect width=\"1\" height=\"1\"/>{}",
        "<g>".repeat(100),
        "</g>".repeat(100)
    );
    assert_eq!(code(wrap(&deep).as_bytes()), "svg.tooDeep");
    let mut d = String::from("M0 0");
    for i in 0..5000 {
        d.push_str(&format!(" L{} {}", i % 10, (i / 10) % 10));
    }
    let long = wrap(&format!(r#"<path d="{d}" fill="none" stroke="black"/>"#));
    assert_eq!(code(long.as_bytes()), "svg.pathTooLong");
    let script = wrap(r#"<script>alert(1)</script><rect width="5" height="5"/>"#);
    assert_eq!(code(script.as_bytes()), "svg.unsupported");
    let clipped = wrap(
        r##"<clipPath id="c"><rect width="5" height="5"/></clipPath><g clip-path="url(#c)"><rect width="9" height="9"/></g>"##,
    );
    assert_eq!(code(clipped.as_bytes()), "svg.unsupported");
    assert_eq!(code(&[0x1f, 0x8b, 8, 0, 0, 0]), "svg.parse");
    // A path whose stroke only shows: its fill is left clear, its stroke kept.
    let line = flatten(
        wrap(r##"<path d="M1 1 L9 9" fill="none" stroke="#FF0000" stroke-width="2"/>"##).as_bytes(),
    )
    .unwrap();
    assert_eq!(line[0].props.stroke, Some(Paint::Color("#FF0000FF".into())));
}

/// An odd dash list is the scene's to repeat (usvg had already doubled it). (A gradient that flattens to nothing
/// now paints `none`; usvg 0.48.1 drops such a gradient's transform itself, so no SVG reaches that guard.)
#[test]
fn imported_odd_dashes_keep_their_pattern() {
    let svg = |body: &str| {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">{body}</svg>"#)
    };
    let dashed =
        svg(r#"<path d="M0 5 L10 5" fill="none" stroke="black" stroke-dasharray="5 2 1"/>"#);
    let nodes = flatten(dashed.as_bytes()).unwrap();
    assert_eq!(nodes[0].props.dash, Some(vec![5.0, 2.0, 1.0]));
}
