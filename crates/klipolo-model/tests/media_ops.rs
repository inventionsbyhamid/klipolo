//! What the library and the chat put on the timeline besides music (`media::{sound, image, text}`): sound effects at a
//! moment, pictures, text presets and title templates, and how they behave through cuts and format changes.
mod common;
use std::collections::BTreeMap;

use common::*;
use klipolo_model::layout::{resolve_rect, BoxN, Layout};
use klipolo_model::media::image::{add_image_ops, image_rect, IMAGE_SIZE, IMAGE_US};
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::media::text::{
    add_template_ops, add_text_ops, preset_rect, TextPreset, TitleTemplate, PORTRAIT_CLEAR, TEXT_US,
};
use klipolo_model::ops::{ItemPatch, Op};
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::project::{Asset, AssetKind, Project, Settings};
use klipolo_model::query::video_duration;
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::{Item, ItemContent, TrackKind};
use klipolo_model::{apply_ops, validate};

fn whoosh() -> Asset {
    Asset {
        id: "whoosh".into(),
        kind: AssetKind::Audio,
        path: "/library/sounds/whoosh.wav".into(),
        name: "Whoosh".into(),
        duration_us: secs(0.9),
        width: 0,
        height: 0,
        fps: 0.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }
}

fn layers(p: &Project) -> Vec<&Item> {
    p.timeline.tracks.iter().flat_map(|t| &t.items).collect()
}

fn layer<'a>(p: &'a Project, id: &str) -> &'a Item {
    layers(p).into_iter().find(|i| i.id == id).unwrap()
}

fn added(ops: &[Op]) -> String {
    ops.iter()
        .find_map(|op| match op {
            Op::AddItem { item, .. } => Some(item.id.clone()),
            _ => None,
        })
        .unwrap()
}

fn inside(r: BoxN, area: BoxN) -> bool {
    const E: f64 = 1e-9;
    r.x >= area.x - E
        && r.y >= area.y - E
        && r.x + r.w <= area.x + area.w + E
        && r.y + r.h <= area.y + area.h + E
}

#[test]
fn two_overlapping_titles_get_two_tracks() {
    let p = fixture();
    let first = add_text_ops(&p, TextPreset::Title, "Intro", secs(2.0)).unwrap();
    let p = apply_ops(&p, &first).unwrap();
    let second = add_text_ops(&p, TextPreset::LowerThird, "Rosa", secs(3.0)).unwrap();
    let p = apply_ops(&p, &second).unwrap();
    let overlays: Vec<_> = p
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Overlay && !t.items.is_empty())
        .collect();
    assert_eq!(overlays.len(), 2, "both visible, each on its own track");
    // Deleting one leaves the other where it was.
    let (a, b) = (added(&first), added(&second));
    let before = layer(&p, &b).clone();
    let p = apply_ops(&p, &[Op::RemoveItem { id: a }]).unwrap();
    assert_eq!(layer(&p, &b), &before);
}

#[test]
fn attached_sound_inside_a_cut_is_removed_and_outside_moves() {
    let p = fixture();
    let inside_cut = add_sound_ops(&p, &whoosh(), secs(5.0)).unwrap();
    let p = apply_ops(&p, &inside_cut).unwrap();
    let after_cut = add_sound_ops(&p, &whoosh(), secs(15.0)).unwrap();
    let p = apply_ops(&p, &after_cut).unwrap();
    let (gone, moved) = (added(&inside_cut), added(&after_cut));
    assert!(
        layer(&p, &gone).attach.is_some(),
        "attached to the clip under it"
    );
    // The library's file came in with the first sound only.
    assert_eq!(p.assets.iter().filter(|a| a.id == "whoosh").count(), 1);
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: secs(4.0),
                end: secs(7.0),
            }],
        }],
    )
    .unwrap();
    assert!(
        layers(&p).iter().all(|i| i.id != gone),
        "inside the cut: removed"
    );
    assert_eq!(
        layer(&p, &moved).start_us,
        secs(12.0),
        "after it: keeps its moment"
    );
}

#[test]
fn a_sound_effect_is_the_whole_file_at_full_volume_cut_at_the_video_end() {
    let p = fixture();
    let end = video_duration(&p.timeline);
    let ops = add_sound_ops(&p, &whoosh(), end - secs(0.5)).unwrap();
    let p = apply_ops(&p, &ops).unwrap();
    let sound = layer(&p, &added(&ops));
    assert_eq!(sound.duration_us, secs(0.5), "cut at the video's end");
    let ItemContent::Audio {
        volume,
        fade_in_us,
        fade_out_us,
        duck,
        ..
    } = &sound.content
    else {
        panic!("a sound layer");
    };
    assert_eq!(
        (*volume, *fade_in_us, *fade_out_us, *duck),
        (1.0, 0, 0, None)
    );
    let silent = Asset {
        has_audio: false,
        ..whoosh()
    };
    assert!(add_sound_ops(&p, &silent, 0).is_err());
}

#[test]
fn an_image_is_medium_centred_for_5_s_and_attached() {
    let p = fixture();
    let logo = p.asset("logo").unwrap().clone();
    let ops = add_image_ops(&p, &logo, secs(3.0)).unwrap();
    let p = apply_ops(&p, &ops).unwrap();
    let image = layer(&p, &added(&ops));
    assert_eq!(image.duration_us, IMAGE_US);
    assert!(image.attach.is_some());
    let r = image.layout.rect;
    assert!((r.w - IMAGE_SIZE).abs() < 1e-9);
    // Square in pixels, centred.
    assert!((r.w * 1920.0 - r.h * 1080.0).abs() < 1e-6);
    assert!((r.x + r.w / 2.0 - 0.5).abs() < 1e-9 && (r.y + r.h / 2.0 - 0.5).abs() < 1e-9);
    // A tall picture keeps the margins.
    let tall = image_rect(500, 4000, Landscape);
    assert!(tall.y >= 0.04 - 1e-9 && tall.h <= 0.92 + 1e-9);
    assert!(add_image_ops(&p, &whoosh(), 0).is_err());
}

#[test]
fn text_stays_inside_safe_area_when_switching_to_portrait() {
    let mut p = fixture();
    p.settings.formats = vec![Landscape];
    for preset in [
        TextPreset::Title,
        TextPreset::LowerThird,
        TextPreset::Plain,
        TextPreset::Caption,
    ] {
        let ops = add_text_ops(&p, preset, "A day at Rosa's studio", secs(1.0)).unwrap();
        let p = apply_ops(&p, &ops).unwrap();
        let id = added(&ops);
        // 9:16 added after the title was placed in 16:9.
        let both = Settings {
            formats: vec![Landscape, Portrait],
            ..p.settings.clone()
        };
        let p = apply_ops(&p, &[Op::SetSettings { settings: both }]).unwrap();
        let layout = &layer(&p, &id).layout;
        let portrait = resolve_rect(layout, Landscape, Portrait);
        assert!(inside(portrait, PORTRAIT_CLEAR), "{preset:?}: {portrait:?}");
        // As tall in pixels as in 16:9: drawn at its size.
        let landscape = resolve_rect(layout, Landscape, Landscape);
        assert!(
            (portrait.h * 1920.0 - landscape.h * 1080.0).abs() < 1e-6,
            "{preset:?}"
        );
        // The primary switched to 9:16: still there.
        let switched = Settings {
            formats: vec![Landscape, Portrait],
            primary_format: Portrait,
            ..p.settings.clone()
        };
        let p = apply_ops(&p, &[Op::SetSettings { settings: switched }]).unwrap();
        let layout = &layer(&p, &id).layout;
        assert!(
            inside(resolve_rect(layout, Portrait, Portrait), PORTRAIT_CLEAR),
            "{preset:?}"
        );
        // Every preset is inside the frame less its margin in 16:9 and 1:1.
        for format in [Landscape, Square] {
            assert!(
                inside(preset_rect(preset, format), BoxN::FULL),
                "{preset:?} {format:?}"
            );
        }
    }
}

#[test]
fn a_text_preset_is_inter_bold_white_with_a_shadow_fading_in_and_out_for_3_s() {
    let p = fixture();
    let ops = add_text_ops(&p, TextPreset::LowerThird, "Rosa", secs(1.0)).unwrap();
    let p = apply_ops(&p, &ops).unwrap();
    let title = layer(&p, &added(&ops));
    assert_eq!(title.duration_us, TEXT_US);
    assert!(title.attach.is_some());
    let ItemContent::Text { text, style } = &title.content else {
        panic!("a text layer");
    };
    assert_eq!(text, "Rosa");
    assert_eq!(
        (
            style.font.family.as_str(),
            style.weight,
            style.color.as_str(),
            style.shadow
        ),
        ("Inter", 700, "#FFFFFF", true)
    );
    assert_eq!(
        (style.size, style.background.as_deref()),
        (56.0, Some("#000000CC"))
    );
    let motion = title.layout.motion.as_ref().unwrap();
    assert_eq!(motion.enter.as_ref().unwrap().duration_us, 300_000);
    assert_eq!(motion.exit.as_ref().unwrap().duration_us, 300_000);
    assert!(add_text_ops(&p, TextPreset::Title, "  ", 0).is_err());
}

#[test]
fn a_title_template_fills_the_frame_in_every_format() {
    let p = fixture();
    let mut params = BTreeMap::new();
    params.insert(
        "title".to_string(),
        klipolo_model::params::ParamValue::Text("Rosa Lopez".into()),
    );
    let ops = add_template_ops(&p, TitleTemplate::LowerThird, params, secs(2.0));
    let p = apply_ops(&p, &ops).unwrap();
    validate(&p).unwrap();
    let card = layer(&p, &added(&ops));
    assert!(matches!(card.content, ItemContent::Generator { .. }));
    for format in [Landscape, Portrait, Square] {
        assert_eq!(
            resolve_rect(&card.layout, Landscape, format),
            BoxN::FULL,
            "{format:?}"
        );
    }
}

/// A layer added with an override for the primary format (`Layout::full_frame` has one for every format) has it taken
/// off as it is added: a later move of its rect shows in the primary.
#[test]
fn a_full_frame_layer_added_keeps_later_rect_edits_in_its_primary() {
    let p = fixture();
    let ops = add_template_ops(&p, TitleTemplate::TitleCard, BTreeMap::new(), 0);
    let p = apply_ops(&p, &ops).unwrap();
    let id = added(&ops);
    let layout = &layer(&p, &id).layout;
    assert!(
        !layout.overrides.contains_key(&Landscape),
        "the primary's override is gone"
    );
    assert_eq!(layout.rect, BoxN::FULL);
    assert_eq!(
        layout.overrides.get(&Portrait),
        Some(&BoxN::FULL),
        "the others keep theirs"
    );
    let smaller = BoxN {
        x: 0.1,
        y: 0.1,
        w: 0.5,
        h: 0.5,
    };
    let moved = Layout {
        rect: smaller,
        ..layout.clone()
    };
    let p = apply_ops(
        &p,
        &[Op::UpdateItem {
            id: id.clone(),
            patch: ItemPatch {
                layout: Some(moved),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(
        resolve_rect(&layer(&p, &id).layout, Landscape, Landscape),
        smaller
    );
    // A primary override that differs from the rect is where the layer was drawn: it becomes the rect.
    let mut item = layer(&p, &id).clone();
    item.id = "copy".into();
    item.layout.overrides.insert(Landscape, smaller);
    item.layout.rect = BoxN::FULL;
    let p = apply_ops(
        &p,
        &[Op::AddItem {
            track: klipolo_model::ops::TrackTarget::Auto,
            item,
            attach: false,
        }],
    )
    .unwrap();
    let copy = &layer(&p, "copy").layout;
    assert_eq!((copy.rect, copy.overrides.get(&Landscape)), (smaller, None));
}

/// A title placed in a 16:9-only project keeps its own 9:16 place through settings changes made before 9:16 is listed:
/// 1:1 added first, then 9:16. Its place for a format that isn't listed stays in the file and comes back with it.
#[test]
fn a_text_presets_portrait_place_survives_formats_added_one_at_a_time() {
    let mut p = fixture();
    p.settings.formats = vec![Landscape];
    let ops = add_text_ops(&p, TextPreset::Title, "A day at Rosa's studio", secs(1.0)).unwrap();
    let mut p = apply_ops(&p, &ops).unwrap();
    let id = added(&ops);
    let placed = layer(&p, &id).layout.clone();
    for formats in [vec![Landscape, Square], vec![Landscape, Square, Portrait]] {
        let settings = Settings {
            formats,
            ..p.settings.clone()
        };
        p = apply_ops(&p, &[Op::SetSettings { settings }]).unwrap();
    }
    let layout = &layer(&p, &id).layout;
    assert_eq!(layout.overrides, placed.overrides);
    let portrait = resolve_rect(layout, Landscape, Portrait);
    assert_eq!(portrait, preset_rect(TextPreset::Title, Portrait));
    assert!(inside(portrait, PORTRAIT_CLEAR), "{portrait:?}");
}

/// A settings change that keeps the formats (the frame rate) leaves every layer's places as they were.
#[test]
fn a_settings_change_that_keeps_the_formats_keeps_every_place() {
    let mut p = fixture();
    p.settings.formats = vec![Landscape, Portrait];
    let ops = add_text_ops(&p, TextPreset::LowerThird, "Rosa Diaz", secs(1.0)).unwrap();
    let p = apply_ops(&p, &ops).unwrap();
    let id = added(&ops);
    let settings = Settings {
        fps: if p.settings.fps == 25 { 30 } else { 25 },
        ..p.settings.clone()
    };
    let next = apply_ops(&p, &[Op::SetSettings { settings }]).unwrap();
    assert_eq!(layer(&next, &id).layout, layer(&p, &id).layout);
}
