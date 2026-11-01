//! Animated words in the registry (`Registry::scene_for`): a generator layer of `media::text_motion::SOURCE` draws the
//! scene its own settings make, compiled once for those settings and shared after; other sources are the registry's
//! own scenes.

use std::sync::Arc;

use klipolo_engine::registry::Registry;
use klipolo_model::contrib::ContribId;
use klipolo_model::media::text_motion::{self, From, TextMotion, Unit};
use klipolo_model::params::ParamValue;
use klipolo_model::timeline::TextStyle;

fn words(text: &str, unit: Unit) -> std::collections::BTreeMap<String, ParamValue> {
    let motion = TextMotion {
        unit,
        from: From {
            y: Some(40.0),
            opacity: Some(0.0),
            ..From::default()
        },
        unit_secs: None,
        ease: None,
        stagger_secs: None,
        order: None,
        exit: true,
    };
    text_motion::params(text, &TextStyle::default(), &motion)
}

#[test]
fn animated_words_draw_the_scene_their_settings_make_once() {
    let reg = Registry::builtin();
    let source = text_motion::source();
    let params = words("Each word in turn", Unit::Word);
    let first = reg.scene_for(&source, &params).expect("a scene");
    let again = reg.scene_for(&source, &params).expect("a scene");
    assert!(Arc::ptr_eq(&first, &again), "compiled once for the same settings");
    assert!(first.facts.animates);
    assert_eq!(first.nodes.len(), 1);
    let segments = first.segments.expect("in and out");
    assert!(segments.enter > 0.0 && segments.out > 0.0);
    let other = reg
        .scene_for(&source, &words("Each word in turn", Unit::Letter))
        .expect("a scene");
    assert!(!Arc::ptr_eq(&first, &other), "other settings, another scene");
}

#[test]
fn other_sources_are_the_registrys_own_scenes() {
    let reg = Registry::builtin();
    let title = ContribId::builtin("scene.title-card");
    let none = Default::default();
    let mine = reg.scene_for(&title, &none).expect("the title card");
    assert!(Arc::ptr_eq(&mine, reg.scene(&title).unwrap()));
    assert!(reg.scene_for(&ContribId::builtin("scene.nope"), &none).is_none());
}
