//! The tools a short is made with, and the refs the context gives the words and pictures on screen (`t1`, `i1`). The
//! project is made here: `talk.mov`, 30 s, played whole, landscape.

use std::collections::HashMap;

use klipolo_chat::{Library, Refs, View, build_context};
use klipolo_model::media::text::{TextPreset, add_text_ops};
use klipolo_model::project::Asset;
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, TrackKind};
use klipolo_model::{Project, apply_ops};
use serde_json::json;

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn asset(id: &str, kind: &str, name: &str, length: f64, size: (u32, u32), audio: bool) -> Asset {
    serde_json::from_value(json!({
        "id": id,
        "kind": kind,
        "path": format!("/media/{name}"),
        "name": name,
        "durationUs": secs(length),
        "width": size.0,
        "height": size.1,
        "fps": if kind == "video" { 30.0 } else { 0.0 },
        "hasAudio": audio,
    }))
    .unwrap()
}

fn heart() -> Asset {
    asset("pack-heart", "image", "heart.png", 5.0, (512, 512), false)
}

/// "talk.mov" played whole.
fn project() -> Project {
    let mut p = Project::new("Trip");
    p.assets = vec![asset("talk", "video", "talk.mov", 30.0, (1920, 1080), true)];
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(30.0));
    clip.id = "t".into();
    p.timeline.main = vec![clip];
    p
}

/// Every layer on the tracks of `kind`, by start.
fn layers(p: &Project, kind: TrackKind) -> Vec<Item> {
    let mut out: Vec<Item> = (p.timeline.tracks.iter())
        .filter(|t| t.kind == kind)
        .flat_map(|t| t.items.iter().cloned())
        .collect();
    out.sort_by_key(|i| i.start_us);
    out
}

/// `p` with a title reading "Day one" from 0.5 s and the Heart sticker from 10.0 s, as the canvas and the library place
/// them.
fn with_words_and_picture() -> Project {
    let p = project();
    let mut ops = add_text_ops(&p, TextPreset::Title, "Day one", secs(0.5)).unwrap();
    ops.extend(klipolo_model::media::image::add_image_ops(&p, &heart(), secs(10.0)).unwrap());
    apply_ops(&p, &ops).unwrap()
}

#[test]
fn the_context_names_words_and_pictures_on_screen() {
    let p = with_words_and_picture();
    let ctx = build_context(
        &p,
        &Speech::default(),
        &HashMap::new(),
        &HashMap::new(),
        &View::default(),
        &Library::default(),
    );
    let at = ctx.text.find("Layers:\n").expect("a Layers list");
    let lines: Vec<&str> = ctx.text[at..].lines().skip(1).take(2).collect();
    assert_eq!(
        lines,
        [
            "t1 0.5-3.5 \"Day one\" (words, enter fade, exit fade)",
            "i1 10.0-15.0 heart.png (picture)",
        ]
    );
    let refs = Refs::of(&p);
    let text = layers(&p, TrackKind::Overlay)[0].id.clone();
    let picture = layers(&p, TrackKind::Overlay)[1].id.clone();
    assert_eq!(refs.text("t1"), Some(text.as_str()));
    assert_eq!(refs.picture("i1"), Some(picture.as_str()));
    assert_eq!((refs.text("t2"), refs.picture("i01")), (None, None));
}

#[test]
fn a_layer_ref_key_changes_when_a_word_or_picture_ref_would_name_another() {
    let p = with_words_and_picture();
    let key = klipolo_chat::layers_key(&p);
    let mut fewer = p.clone();
    let first = layers(&p, TrackKind::Overlay)[0].id.clone();
    (fewer.timeline.tracks.iter_mut()).for_each(|t| t.items.retain(|i| i.id != first));
    assert_ne!(klipolo_chat::layers_key(&fewer), key);
}
