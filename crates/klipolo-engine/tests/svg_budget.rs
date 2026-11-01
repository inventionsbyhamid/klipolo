//! Hostile SVGs for the rest of usvg's work: markers refused by name; arcs, circles, ellipses and rounded rects weighed
//! by their radius, a percentage by
//! the largest viewport a `<use>` can give it, and every percentage refused once a viewport grows past 100 %;
//! attribute bytes weighed once per copy and once per descendant that inherits them; style sheets by their selectors
//! and combinators; duplicate ids resolved first-wins and `href` before `xlink:href`, as usvg resolves them; gradient
//! chains. A `<use>` that names nothing is ignored, and every refusal names what it found and says what to do. Each
//! refusal comes within a second.

mod svg_support;

use klipolo_engine::make::svg::flatten;
use svg_support::*;

#[test]
fn markers_are_refused_by_name() {
    // usvg converts a marker's children at every vertex of every path using it, markers within markers too.
    let svg = wrap(
        r##"<defs><marker id="m" overflow="visible"><path d="M0 0h1" stroke="black"/></marker></defs>
          <path d="M0 0h1h1h1" fill="none" stroke="black" marker-mid="url(#m)"/>"##,
    );
    let p = refusal(&svg);
    assert_eq!(p.code, "svg.unsupported");
    assert!(p.message.contains("marker"), "{}", p.message);
}

#[test]
fn arcs_and_round_shapes_are_weighed_by_their_radius() {
    let radius = |svg: &str| {
        let p = refusal(&wrap(svg));
        assert!(
            p.code == "svg.tooLarge" && p.message.contains("radius"),
            "{}: {}",
            p.code,
            p.message
        );
    };
    // One arc of radius 4·10²⁶: kurbo makes about 40,000 cubics of it, which svgtypes drains from a Vec's front.
    radius(r#"<path d="M0 0A4e26 4e26 0 1 1 1 0" fill="none" stroke="black"/>"#);
    // An arc whose radii are too small to reach its end: scaled up as kurbo scales them (one radius 10⁻⁵ of the other).
    radius(r#"<path d="M0 0A1e-4 1e9 0 0 1 4e6 0" fill="none" stroke="black"/>"#);
    // A thousand copies of a circle of radius 4·10²⁰ (4,000 cubics each), a rounded rect, an ellipse.
    let circles = format!(
        r##"<defs><g id="c"><circle r="4e20"/></g></defs>{}"##,
        r##"<use href="#c"/>"##.repeat(1000)
    );
    radius(&circles);
    radius(r#"<rect width="8e20" height="8e20" rx="4e20"/>"#);
    radius(r#"<ellipse rx="1" ry="4e20"/>"#);
    // A radius in em: the font size it is relative to can be anything.
    radius(r#"<g font-size="1e30"><circle r="2em"/></g>"#);
    // An icon's rounded corners and circles import, in percent of the view too.
    let icon = r#"<path d="M2 0h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H2a2 2 0 0 1-2-2V2a2 2 0 0 1 2-2z"/>
      <circle cx="5" cy="5" r="20%" fill="white"/><rect x="4" y="4" width="2" height="2" rx="0.5"/>"#;
    assert_eq!(flatten(wrap(icon).as_bytes()).unwrap().len(), 3);
}

/// A `<use>` sizes the `<symbol>` (or `<svg>`) it shows, and a percentage inside is of that size,
/// so a file of 200 bytes can ask usvg for an ellipse or a rounded rect of millions of cubics.
#[test]
fn a_use_that_sizes_a_symbol_weighs_the_percent_radii_inside() {
    for inside in [
        r#"<ellipse rx="50%" ry="50%"/>"#,
        r#"<rect width="100%" height="100%" rx="50%"/>"#,
    ] {
        let p = refusal(&wrap(&format!(
            r##"<symbol id="s">{inside}</symbol><use href="#s" width="3e38" height="3e38"/>"##
        )));
        assert!(
            p.code == "svg.tooLarge" && p.message.contains("radius"),
            "{inside}: {}: {}",
            p.code,
            p.message
        );
    }
}

/// `hops` symbols in a `<use>` chain, each nesting `depth` `<svg>`s at 200 % before showing the next at 200 %, the
/// last an `rx="50%"` ellipse: every level doubles the viewport the radius is a share of.
fn chained(hops: usize, depth: usize) -> String {
    let open = r#"<svg width="200%" height="200%">"#.repeat(depth);
    let close = "</svg>".repeat(depth);
    let mut body = String::new();
    for i in 0..hops {
        let inner = if i + 1 < hops {
            format!(r##"<use href="#s{}" width="200%" height="200%"/>"##, i + 1)
        } else {
            r#"<ellipse rx="50%" ry="50%"/>"#.to_owned()
        };
        body.push_str(&format!(
            r#"<symbol id="s{i}">{open}{inner}{close}</symbol>"#
        ));
    }
    body.push_str(r##"<use href="#s0" width="200%" height="200%"/>"##);
    wrap(&body)
}

/// A viewport sized past 100 % (an `<svg>`, `<symbol>` or `<use>`) compounds through nesting and
/// through every hop of a `<use>` chain, until usvg's f32 viewport saturates and one `rx="50%"` ellipse becomes
/// millions of cubics. So once any viewport grows past 100 %, every percentage length in the file is refused, plainly
/// and naming it.
#[test]
fn a_percentage_in_a_file_whose_viewport_grows_is_refused() {
    let named = |svg: &str, says: &str| {
        let p = refusal(svg);
        assert!(
            p.code == "svg.tooLarge"
                && p.message.contains(says)
                && p.message.ends_with("then import again."),
            "{says}: {}: {}",
            p.code,
            p.message
        );
    };
    // A crafted file: about 7 KB, a 32-deep chain, 4 nested <svg> at 200 % in each symbol (usvg made its one
    // ellipse 1.6 million segments before this was refused).
    let file = chained(32, 4);
    assert!(file.len() < 12 << 10, "{} bytes", file.len());
    named(&file, r#"rx="50%""#);
    named(&chained(16, 2), r#"rx="50%""#);
    // One <use> 10³⁴ times its viewport.
    named(
        &wrap(
            r##"<symbol id="s"><ellipse rx="50%" ry="50%"/></symbol><use href="#s" width="1e36%" height="1e36%"/>"##,
        ),
        r#"rx="50%""#,
    );
    // Any percentage length, not only a radius; in a style attribute too.
    let grown = |inside: &str| {
        wrap(&format!(
            r#"<svg width="150%" height="150%" overflow="visible">{inside}</svg>"#
        ))
    };
    named(
        &grown(r#"<rect x="10%" width="1" height="1"/>"#),
        r#"x="10%""#,
    );
    named(
        &grown(r#"<path d="M0 0h5" stroke="black" style="stroke-width:5%"/>"#),
        "stroke-width:5%",
    );
}

/// The other side: a common sprite, symbols shown by `<use>` at 100 % or at pixel sizes (and a nested `<svg>` at
/// 100 %) with percentage radii and positions inside, still imports. (Overflow visible: a symbol usvg clips is
/// refused on its own, by the converter.)
#[test]
fn a_sprite_at_100_percent_with_percent_radii_imports() {
    let sprite = wrap(
        r##"<defs><symbol id="pill" viewBox="0 0 10 10" overflow="visible">
          <rect width="100%" height="100%" rx="50%"/></symbol>
          <symbol id="dot" viewBox="0 0 10 10" overflow="visible"><circle cx="50%" cy="50%" r="25%"/></symbol></defs>
          <svg width="100%" height="100%" overflow="visible"><use href="#pill" width="100%" height="100%"/></svg>
          <use href="#dot" width="100%" height="100%"/><use href="#dot" x="2" width="4" height="4"/>"##,
    );
    assert_eq!(flatten(sprite.as_bytes()).unwrap().len(), 3);
}

#[test]
fn attribute_bytes_are_weighed_per_copy_and_per_inheriting_descendant() {
    // A thousand copies of a path under a group whose 100 KB dash list every copy reads again (and keeps).
    let svg = wrap(&format!(
        r##"<defs><path id="p" d="M0 0h1" fill="none" stroke="black"/><g id="a">{}</g><g id="b">{}</g></defs>
          <g stroke-dasharray="{}">{}</g>"##,
        r##"<use href="#p"/>"##.repeat(10),
        r##"<use href="#a"/>"##.repeat(10),
        "1 ".repeat(50_000),
        r##"<use href="#b"/>"##.repeat(10)
    ));
    let p = refusal(&svg);
    assert_eq!(p.code, "svg.tooLarge");
    assert!(p.message.contains("attributes"), "{}", p.message);
}

#[test]
fn style_sheets_are_weighed_by_selectors_and_combinators() {
    let sheet = |svg: &str| {
        let p = refusal(&wrap(svg));
        assert!(
            p.code == "svg.tooLarge" && p.message.contains("style sheet"),
            "{}: {}",
            p.code,
            p.message
        );
    };
    // 30,000 selectors in one comma list (one `{`): simplecss makes a rule of each, and usvg tests each on every element.
    sheet(&format!(
        "<style>{}*{{fill:red}}</style>{}",
        "*,".repeat(29_999),
        RECT.repeat(150)
    ));
    // Two selectors of six descendant combinators over 40 nested groups: matching backtracks through every ancestor.
    sheet(&format!(
        "<style>x g g g g g g{{fill:red}} y g g g g g g{{fill:blue}}</style>{}{RECT}{}",
        "<g>".repeat(40),
        "</g>".repeat(40)
    ));
    // An ordinary sheet imports.
    let modest = r#"<style>.a{fill:red} g > .b, .c{stroke:blue;stroke-width:2}</style>
      <g><rect class="a b" width="1" height="1"/><rect class="c" x="2" width="1" height="1"/></g>"#;
    assert_eq!(flatten(wrap(modest).as_bytes()).unwrap().len(), 2);
}

#[test]
fn duplicate_ids_and_both_hrefs_resolve_as_usvg_resolves_them() {
    // The first `#x` holds 200 rects, the last one: 150 uses copy the first (30,000 elements to usvg, which keeps the
    // first of a duplicated id).
    let svg = wrap(&format!(
        r##"<defs><g id="x">{}</g><g id="x">{RECT}</g></defs>{}"##,
        RECT.repeat(200),
        r##"<use href="#x"/>"##.repeat(150)
    ));
    assert_eq!(refused(&svg), "svg.tooLarge");
    // A <use> with both hrefs takes the unprefixed one (usvg 0.48.1), in either order: 120 copies of a rect, not of
    // 200 rects (24,000 elements).
    let heavy = format!(r#"<defs><g id="heavy">{}</g></defs>"#, RECT.repeat(200));
    for u in [
        r##"<use xlink:href="#heavy" href="#light"/>"##,
        r##"<use href="#light" xlink:href="#heavy"/>"##,
    ] {
        let uses = u.repeat(120);
        let svg = wrap(&format!(
            r#"{heavy}<rect id="light" width="1" height="1"/>{uses}"#
        ));
        assert_eq!(flatten(svg.as_bytes()).unwrap().len(), 121, "{u}");
    }
}

#[test]
fn gradient_chains_are_weighed() {
    // A → B → C → B: usvg follows a gradient's href until it finds stops, stopping only back at A or at itself.
    let cycle = r##"<defs><linearGradient id="a" href="#b"/><linearGradient id="b" href="#c"/>
      <linearGradient id="c" href="#b"/></defs><rect width="5" height="5" fill="url(#a)"/>"##;
    assert_eq!(refused(&wrap(cycle)), "svg.reference");
    let mut chain = String::from("<defs>");
    for i in 0..40 {
        chain.push_str(&format!(
            r##"<linearGradient id="g{i}" href="#g{}"/>"##,
            i + 1
        ));
    }
    chain.push_str(
        r##"<linearGradient id="g40"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
          </linearGradient></defs><rect width="5" height="5" fill="url(#g0)"/>"##,
    );
    assert_eq!(refused(&wrap(&chain)), "svg.tooDeep");
}

/// A `<use>` that names nothing here (or names another file) is ignored, as usvg ignores it, and
/// nothing is opened.
#[test]
fn a_use_that_names_nothing_is_ignored() {
    let svg = wrap(&format!(r##"<use href="#nothing"/>{RECT}"##));
    assert_eq!(flatten(svg.as_bytes()).unwrap().len(), 1);
    let svg = wrap(r##"<use href="other.svg#a"/><use href="#"/><path d="M0 0h5v5z"/>"##);
    assert_eq!(flatten(svg.as_bytes()).unwrap().len(), 1);
}

/// Each refusal names the first thing it found and says what to do.
#[test]
fn every_refusal_names_what_it_found_and_says_what_to_do() {
    for (el, says, not) in [
        ("pattern", "pattern", "mask"),
        ("clipPath", "clip path", "mask"),
        ("mask", "mask", "filter"),
        ("filter", "filter", "mask"),
    ] {
        let p = refusal(&wrap(&format!(r#"<defs><{el} id="x"/></defs>{RECT}"#)));
        assert!(
            p.message.contains(says) && !p.message.contains(not),
            "{el}: {}",
            p.message
        );
    }
    let first = refusal(&wrap(&format!(
        r#"<defs><mask id="m"/><filter id="f"/></defs>{RECT}"#
    )));
    assert!(
        first.message.contains("mask") && !first.message.contains("filter"),
        "{}",
        first.message
    );
    let script = refusal(&wrap(&format!("<script>x()</script>{RECT}")));
    assert!(script.message.contains("script"), "{}", script.message);
    let foreign = refusal(&wrap(&format!("<foreignObject/>{RECT}")));
    assert!(foreign.message.contains("foreign"), "{}", foreign.message);
    // A nested <svg> (usvg clips it to its own viewport) is named as such, not as a clip path the file doesn't have.
    let nested = refusal(&wrap(
        r#"<svg width="5" height="5"><rect width="9" height="9"/></svg>"#,
    ));
    assert!(
        nested.message.contains("<svg> or <symbol>"),
        "{}",
        nested.message
    );
    // A filter written as a function, with no <filter> element.
    let blur = refusal(&wrap(r#"<rect width="5" height="5" filter="blur(2)"/>"#));
    assert!(blur.message.contains("filter"), "{}", blur.message);
    let picture = refusal(&wrap(
        r#"<image href="data:image/png;base64,iVBORw0KGgo=" width="1" height="1"/>"#,
    ));
    assert!(picture.message.contains("picture"), "{}", picture.message);
    let link = refusal(&wrap(
        r#"<image href="https://example.com/a.png" width="1" height="1"/>"#,
    ));
    assert!(link.message.contains("never opens"), "{}", link.message);
}
