//! `text.rs`'s unit tests (no GPU): the font shorthand through the face resolver, and whose weight and italic win.

use klipolo_model::scene::instance::Field;
use klipolo_model::scene::{Flag, Node, SceneFont, Val};

use super::*;

/// A text node of the scene JSON `json`, as written.
fn text_node(json: &str) -> Node {
    serde_json::from_str(json).expect("a text node")
}

fn no_params() -> NoBindings {
    NoBindings
}

/// `n.font()`: the node's `font`, which every node here has.
trait Font {
    fn font(&self) -> &SceneFont;
}

impl Font for Node {
    fn font(&self) -> &SceneFont {
        self.props.font.as_ref().expect("a font")
    }
}

#[test]
fn the_font_shorthand_resolves_through_the_face_resolver_and_the_fonts_own_weight_wins() {
    use klipolo_model::font::{FontRef, FontSource};
    let ix = klipolo_fonts::index::FontIndex::shipped(klipolo_fonts::bundled::bundled_dir());
    let n = text_node(
        r#"{"type":"text","text":"Hi","box":[0,0,1,1],"font":"montserrat","weight":300}"#,
    );
    assert_eq!(
        font_ref(n.font(), &n, &ix, &no_params()),
        (FontRef::bundled("Montserrat"), 300, false),
        "the shorthand goes through resolve_family; the node's weight applies when font sets none"
    );
    let n = text_node(
        r#"{"type":"text","text":"Hi","box":[0,0,1,1],"weight":300,"italic":true,
        "font":{"family":"Montserrat","source":{"type":"bundled"},"weight":700}}"#,
    );
    assert_eq!(
        font_ref(n.font(), &n, &ix, &no_params()),
        (FontRef::bundled("Montserrat"), 700, true),
        "font's own weight wins; the node's italic fills the gap"
    );
    let n = text_node(r#"{"type":"text","text":"Hi","box":[0,0,1,1],"font":"No Such Family"}"#);
    assert_eq!(
        font_ref(n.font(), &n, &ix, &no_params()).0,
        FontRef {
            family: "No Such Family".into(),
            source: FontSource::Installed
        }
    );
}

/// A `"$font"` binding takes its setting's `FontRef`; one with no such setting draws the default face.
#[test]
fn a_bound_font_takes_its_setting() {
    use klipolo_model::font::FontRef;
    let ix = klipolo_fonts::index::FontIndex::shipped(klipolo_fonts::bundled::bundled_dir());
    struct Picked;
    impl Bindings for Picked {
        fn number(&self, field: Field, v: &Val) -> Option<f64> {
            NoBindings.number(field, v)
        }
        fn flag(&self, f: &Flag) -> Option<bool> {
            NoBindings.flag(f)
        }
        fn font(&self, s: &str) -> Option<FontRef> {
            (s == "$face").then(|| FontRef::bundled("Oswald"))
        }
    }
    let n = text_node(r#"{"type":"text","text":"Hi","box":[0,0,1,1],"font":"$face","weight":600}"#);
    assert_eq!(
        font_ref(n.font(), &n, &ix, &Picked),
        (FontRef::bundled("Oswald"), 600, false)
    );
    assert_eq!(
        font_ref(n.font(), &n, &ix, &no_params()).0,
        FontRef::bundled(DEFAULT_FAMILY)
    );
    let n = text_node(r#"{"type":"text","text":"Hi","box":[0,0,1,1],"font":"$$face"}"#);
    assert_eq!(
        font_ref(n.font(), &n, &ix, &Picked).0.family,
        "$face",
        "`$$` is a literal name"
    );
}
