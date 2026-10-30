//! What the SVG import tests share: an SVG around a body, a nest of `<use>` that multiplies a shape, and
//! a refusal timed in a thread of its own, so a hostile file that never ends fails its test after two seconds instead
//! of hanging the run.
#![allow(dead_code)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use klipolo_engine::make::svg::flatten;
use klipolo_model::scene::SceneProblem;

pub const RECT: &str = r#"<rect width="1" height="1"/>"#;

pub fn wrap(body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10">{body}</svg>"#
    )
}

/// `levels` groups `l1`…, each using the one before ten times through `href(level)`, over one shape `base` (`l0`).
pub fn nest_defs(base: &str, levels: usize, href: impl Fn(usize) -> String) -> String {
    let mut s = format!(r#"<defs><g id="l0">{base}</g>"#);
    for level in 1..=levels {
        s.push_str(&format!(r#"<g id="l{level}">"#));
        for _ in 0..10 {
            s.push_str(&format!(r#"<use href="{}"/>"#, href(level - 1)));
        }
        s.push_str("</g>");
    }
    s.push_str("</defs>");
    s
}

/// [`nest_defs`] used once: 10^`levels` copies of `base`.
pub fn nest(base: &str, levels: usize, href: impl Fn(usize) -> String) -> String {
    wrap(&format!(
        r##"{}<use href="#l{levels}"/>"##,
        nest_defs(base, levels, href)
    ))
}

fn head(svg: &str) -> &str {
    &svg[..svg.len().min(240)]
}

/// `svg`'s first problem: refused within a second, and saying what to do.
pub fn refusal(svg: &str) -> SceneProblem {
    let (tx, rx) = mpsc::channel();
    let owned = svg.to_owned();
    let started = Instant::now();
    std::thread::spawn(move || {
        let _ = tx.send(flatten(owned.as_bytes()));
    });
    let result = rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|_| panic!("still running after 2 s: {}", head(svg)));
    let took = started.elapsed();
    let e = result
        .err()
        .unwrap_or_else(|| panic!("not refused: {}", head(svg)));
    assert!(took < Duration::from_secs(1), "{took:?}: {:?}", e[0]);
    assert!(
        e[0].message.contains("import again"),
        "no advice: {}",
        e[0].message
    );
    e.into_iter().next().unwrap()
}

/// [`refusal`]'s code.
pub fn refused(svg: &str) -> &'static str {
    refusal(svg).code
}
