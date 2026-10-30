//! Hostile SVGs refused before usvg does their work: every `<use>` read as usvg
//! reads it (spaces round the `#id`), path data counted in every copy, servers that usvg clones per use (patterns,
//! clip paths, masks, filters) refused by name, `<style>` rules capped, cycles and long chains of `<use>` refused, and
//! what comes out sized to fit a stored scene. Each is a plain refusal within a second that says what to do: no worker
//! killed, no strike.

mod svg_support;

use klipolo_engine::make::svg::flatten;
use svg_support::*;

#[test]
fn what_usvg_would_multiply_is_refused_before_it_runs() {
    // An href with spaces round its `#id` is a reference all the same (svgtypes reads it so): 10⁵ copies.
    assert_eq!(
        refused(&nest(RECT, 5, |l| format!(" #l{l}"))),
        "svg.tooLarge"
    );
    assert_eq!(
        refused(&nest(RECT, 5, |l| format!("#l{l} "))),
        "svg.tooLarge"
    );
    // A 4,000-command path used 1,000 times: its data counts once per copy.
    let path = format!(
        r#"<path d="M0 0{}" fill="none" stroke="black"/>"#,
        "h1".repeat(4000)
    );
    assert_eq!(
        refused(&nest(&path, 3, |l| format!("#l{l}"))),
        "svg.tooLarge"
    );
    // Paint servers, clips, masks and filters are refused by name, used or not.
    for el in ["pattern", "clipPath", "mask", "filter"] {
        let svg = wrap(&format!(r#"<defs><{el} id="x"/></defs>{RECT}"#));
        assert_eq!(refused(&svg), "svg.unsupported", "{el}");
    }
    // A thousand style rules.
    let style = format!("<style>{}</style>{RECT}", ".a{fill:red}".repeat(1000));
    assert_eq!(refused(&wrap(&style)), "svg.tooLarge");
    // A <use> that leads back to itself through another (one that names nothing is ignored: `svg_budget.rs`).
    let cycle = r##"<defs><g id="a"><g><use href="#b"/></g></g><g id="b"><use href="#a"/></g></defs><use href="#a"/>"##;
    assert_eq!(refused(&wrap(cycle)), "svg.reference");
    // Inkscape's flowed text, which usvg would drop silently.
    let flowed = r#"<flowRoot><flowPara>Hello</flowPara></flowRoot>"#;
    assert_eq!(refused(&wrap(flowed)), "svg.text");
    // A chain of 40 <use>, each of the one before.
    let mut chain = format!(r#"<defs><g id="c0">{RECT}</g>"#);
    for i in 1..=40 {
        chain.push_str(&format!(r##"<g id="c{i}"><use href="#c{}"/></g>"##, i - 1));
    }
    chain.push_str(r##"</defs><use href="#c40"/>"##);
    assert_eq!(refused(&wrap(&chain)), "svg.tooDeep");
}

/// What comes out must store: a view or a coordinate past the scene's reach is refused, so is more path data than a
/// stored scene holds, and numbers are written to the view's precision.
#[test]
fn what_comes_out_fits_a_stored_scene() {
    let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1e30" height="10"><rect width="5" height="5"/></svg>"#;
    assert_eq!(refused(huge), "svg.tooLarge");
    let far = wrap(r#"<path d="M0 0 L1e9 5" fill="none" stroke="black"/>"#);
    assert_eq!(refused(&far), "svg.tooLarge");
    // A hundred 4,000-command paths: 800 KB in, megabytes of path data out.
    let mut many = String::new();
    for i in 0..100 {
        many.push_str(&format!(
            r#"<path d="M0 {i}{}" fill="none" stroke="black"/>"#,
            "h1".repeat(4000)
        ));
    }
    assert_eq!(refused(&wrap(&many)), "svg.tooLarge");
    let nodes = flatten(include_bytes!("fixtures/svg/logo.svg")).unwrap();
    for n in &nodes {
        let d = n.props.d.as_deref().unwrap();
        for number in d.split(|c: char| c == ' ' || c.is_ascii_alphabetic()) {
            let decimals = number.split_once('.').map_or(0, |(_, f)| f.len());
            assert!(decimals <= 2, "{number} in {d}");
        }
    }
}

/// The review's own five, at full size: each refused by the pre-pass, plainly and quickly (they would otherwise drive
/// usvg past the worker's memory cap or its deadline).
#[test]
fn the_reviews_hostile_svgs_are_refused_plainly() {
    assert_eq!(
        refused(&nest(RECT, 6, |l| format!(" #l{l}"))),
        "svg.tooLarge"
    );
    let long = format!(r#"<path d="M0 0{}"/>"#, "h1".repeat(490_000));
    let code = refused(&nest(&long, 3, |l| format!("#l{l}")));
    assert!(
        code == "svg.tooLarge" || code == "svg.pathTooLong",
        "{code}"
    );
    let pattern = r##"<defs><pattern id="p" width="1" height="1"><rect width="1" height="1"/></pattern></defs>
      <rect width="5" height="5" fill="url(#p)"/>"##;
    assert_eq!(refused(&wrap(pattern)), "svg.unsupported");
    let rules = format!("<style>{}</style>{RECT}", "*{}".repeat(300_000));
    assert_eq!(refused(&wrap(&rules)), "svg.tooLarge");
    let heavy = RECT.repeat(50);
    let three = format!(
        r##"<defs><g id="a">{heavy}<use href="#b"/></g><g id="b"><use href="#c"/></g><g id="c"><use href="#a"/></g></defs><use href="#a"/>"##
    );
    assert_eq!(refused(&wrap(&three)), "svg.reference");
    // A chain of 5,000 written from its far end: refused as too deep, never walked to the bottom.
    let mut far = String::from("<defs>");
    for i in (1..=5000).rev() {
        far.push_str(&format!(r##"<g id="r{i}"><use href="#r{}"/></g>"##, i - 1));
    }
    far.push_str(&format!(
        r##"<g id="r0">{RECT}</g></defs><use href="#r5000"/>"##
    ));
    assert_eq!(refused(&wrap(&far)), "svg.tooDeep");
}
