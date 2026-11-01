//! Separate sound: an imported video clip's own sound becomes a sound layer under it, and the clip is muted, so
//! nothing heard changes.
mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::ops::{op_risk, Op, OpRisk};
use klipolo_model::project::Project;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Item, ItemContent, TrackKind};
use klipolo_model::{apply_ops, Transaction};

fn detach(clip: &str) -> Op {
    Op::DetachSound {
        clip_id: clip.into(),
    }
}

fn layer<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no layer {id}"))
}

/// Each sound layer of the b-roll's file: (start, length, where in the file it starts), by start.
fn broll_sounds(p: &Project) -> Vec<(Us, Us, Us)> {
    let mut out: Vec<_> = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .filter_map(|i| match &i.content {
            ItemContent::Audio {
                asset_id,
                source_in_us,
                ..
            } if asset_id == "broll" => Some((i.start_us, i.duration_us, *source_in_us)),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn separating_an_imported_clip_s_sound_makes_an_attached_sound_layer_and_mutes_the_clip() {
    let mut p = fixture();
    // "b" plays the b-roll from 2 s to 8 s, at 20 s on the timeline.
    p.timeline.main[1].source_in_us = secs(2.0);
    p.timeline.main[1].source_out_us = secs(8.0);
    let p = apply_ops(&p, &[detach("b")]).unwrap();
    let item = layer(&p, "b-sound");
    assert!(matches!(
        &item.content,
        ItemContent::Audio { asset_id, source_in_us, muted: false, tie: None, duck: None, .. }
            if asset_id == "broll" && *source_in_us == secs(2.0)
    ));
    assert_eq!((item.start_us, item.duration_us), (secs(20.0), secs(6.0)));
    let anchor = item.attach.as_ref().expect("attached to its clip");
    assert_eq!(
        (anchor.clip_id.as_str(), anchor.source_us),
        ("b", secs(2.0))
    );
    let track = p
        .timeline
        .tracks
        .iter()
        .find(|t| t.items.iter().any(|i| i.id == "b-sound"))
        .unwrap();
    assert_eq!(track.kind, TrackKind::Audio);
    assert!(p.timeline.main[1].muted, "not heard twice");
    assert_eq!(op_risk(&detach("b")), OpRisk::Additive);
    assert_eq!(detach("b").tag(), "detachSound");
}

#[test]
fn a_separated_sound_plays_as_its_clip_played_it() {
    let mut p = fixture();
    let b = &mut p.timeline.main[1];
    (b.volume, b.fade_in_us, b.fade_out_us) = (1.5, secs(1.0), secs(2.0));
    let p = apply_ops(&p, &[detach("b")]).unwrap();
    assert!(matches!(
        layer(&p, "b-sound").content,
        ItemContent::Audio { volume, fade_in_us, fade_out_us, muted: false, .. }
            if volume == 1.5 && fade_in_us == secs(1.0) && fade_out_us == secs(2.0)
    ));
    // A clip already muted gives a muted layer: still nothing heard.
    let mut quiet = fixture();
    quiet.timeline.main[1].muted = true;
    let quiet = apply_ops(&quiet, &[detach("b")]).unwrap();
    assert!(matches!(
        layer(&quiet, "b-sound").content,
        ItemContent::Audio { muted: true, .. }
    ));
}

#[test]
fn the_separated_sound_follows_its_clip() {
    let p = apply_ops(&fixture(), &[detach("b")]).unwrap();
    // Removing the clip before it moves "b" to 0 s, and its sound with it.
    let p = apply_ops(&p, &[Op::RemoveClip { id: "a".into() }]).unwrap();
    assert_eq!(broll_sounds(&p), [(0, secs(10.0), 0)]);
}

#[test]
fn separating_is_refused_where_it_would_change_what_is_heard() {
    // A recording's sounds are separated one by one (`DetachSource`).
    assert_eq!(
        apply_ops(&fixture(), &[detach("a")]).unwrap_err(),
        refused("Separate a recording's sounds one by one from its menu.")
    );
    let mut silent = fixture();
    silent
        .assets
        .iter_mut()
        .find(|a| a.id == "broll")
        .unwrap()
        .has_audio = false;
    assert_eq!(
        apply_ops(&silent, &[detach("b")]).unwrap_err(),
        refused("This clip's file has no sound.")
    );
    let mut fast = fixture();
    fast.timeline.main[1].speed = 2.0;
    assert_eq!(
        apply_ops(&fast, &[detach("b")]).unwrap_err(),
        refused("Set the clip back to normal speed before separating its sound.")
    );
    let once = apply_ops(&fixture(), &[detach("b")]).unwrap();
    assert_eq!(
        apply_ops(&once, &[detach("b")]).unwrap_err(),
        refused("This clip's sound is already in its own layer.")
    );
    assert!(apply_ops(&fixture(), &[detach("nope")]).is_err());
}

/// Separate, then undo: the layer is gone and the clip plays its own sound again, exactly as before.
#[test]
fn separating_a_sound_then_undo_puts_the_clip_back() {
    let mut doc = Document::new(fixture()).unwrap();
    let before = doc.project().clone();
    doc.apply(&Transaction {
        label: "undo.sound.separate".into(),
        ops: vec![detach("b")],
        origin: None,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(broll_sounds(doc.project()), [(secs(20.0), secs(10.0), 0)]);
    doc.undo();
    assert_eq!(*doc.project(), before);
}
