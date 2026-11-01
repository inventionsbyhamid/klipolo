//! What the library and the chat put on the timeline besides music (`media::{sound, image, text}`): sound effects at a
//! moment, pictures, text presets and title templates, and how they behave through cuts and format changes.
mod common;
use std::collections::BTreeMap;

use common::*;
use klipolo_model::layout::{resolve_rect, BoxN, Layout};
use klipolo_model::media::image::{
    add_image_ops, add_image_ops_at, background_picture_ops, corner_rect, image_rect, Spot,
    IMAGE_MARGIN, IMAGE_SIZE, IMAGE_US,
};
use klipolo_model::media::music::{add_music_ops, is_music, MusicOpts};
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::media::text::{
    add_template_ops, add_text_ops, preset_rect, TextPreset, TitleTemplate, PORTRAIT_CLEAR, TEXT_US,
};
use klipolo_model::ops::{ItemPatch, Op};
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::project::{Asset, AssetKind, BackgroundKind, Project, Settings};
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

/// A picture behind the video: the project's background shows it, keeping its padding and colours; a file the project
/// doesn't have yet comes in with it, and one it has is used as it is. A sound isn't a picture.
#[test]
fn a_picture_becomes_the_background_with_its_file() {
    let mut p = fixture();
    p.background.padding = 0.1;
    let logo = p.asset("logo").unwrap().clone();
    let ops = background_picture_ops(&p, &logo).unwrap();
    assert!(
        matches!(ops.as_slice(), [Op::SetBackground { .. }]),
        "{ops:?}"
    );
    let shown = apply_ops(&p, &ops).unwrap();
    assert_eq!(shown.background.kind, BackgroundKind::Image);
    assert_eq!(shown.background.asset_id.as_deref(), Some("logo"));
    assert_eq!(shown.background.padding, 0.1);
    assert!(validate(&shown).is_ok());

    let wall = Asset {
        id: "wall".into(),
        path: "/Users/me/Pictures/wall.jpg".into(),
        name: "wall.jpg".into(),
        ..logo
    };
    let ops = background_picture_ops(&p, &wall).unwrap();
    assert!(
        matches!(
            ops.as_slice(),
            [Op::AddAssets { .. }, Op::SetBackground { .. }]
        ),
        "{ops:?}"
    );
    let shown = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        shown.asset("wall").map(|a| a.path.as_str()),
        Some("/Users/me/Pictures/wall.jpg")
    );
    assert_eq!(shown.background.asset_id.as_deref(), Some("wall"));
    assert!(validate(&shown).is_ok());
    assert!(background_picture_ops(&p, &whoosh()).is_err());
}

#[test]
fn a_picture_in_a_corner_keeps_to_each_formats_own_corner() {
    // A 2:1 logo at a fifth of the width in the top right, placed in each primary in turn.
    let mut p = fixture();
    let logo = Asset {
        width: 800,
        height: 400,
        ..p.asset("logo").unwrap().clone()
    };
    p.assets.retain(|a| a.id != "logo");
    p.assets.push(logo.clone());
    for primary in [Landscape, Portrait, Square] {
        p.settings = Settings {
            formats: vec![Landscape, Portrait, Square],
            primary_format: primary,
            ..p.settings.clone()
        };
        let ops = add_image_ops_at(&p, &logo, secs(1.0), 0.2, Spot::TopRight).unwrap();
        let after = apply_ops(&p, &ops).unwrap();
        let layout = &layer(&after, &added(&ops)).layout;
        for format in [Landscape, Portrait, Square] {
            let r = resolve_rect(layout, primary, format);
            assert_eq!(
                r,
                corner_rect(800, 400, 0.2, Spot::TopRight, format),
                "{primary:?} {format:?}"
            );
            let (cw, ch) = match format {
                Landscape => (1920.0, 1080.0),
                Portrait => (1080.0, 1920.0),
                Square => (1080.0, 1080.0),
            };
            let area = if format == Portrait {
                PORTRAIT_CLEAR
            } else {
                BoxN::FULL
            };
            let margin = IMAGE_MARGIN * 1080.0;
            // A fifth of this format's width, 2:1, the margin in from the area's top and right.
            assert!((r.w * cw - 0.2 * cw).abs() < 1e-6, "{format:?}: {r:?}");
            assert!(
                (r.w * cw - 2.0 * r.h * ch).abs() < 1e-6,
                "{format:?}: {r:?}"
            );
            assert!(
                ((area.x + area.w - r.x - r.w) * cw - margin).abs() < 1e-6,
                "{format:?}: {r:?}"
            );
            assert!(
                ((r.y - area.y) * ch - margin).abs() < 1e-6,
                "{format:?}: {r:?}"
            );
            assert!(inside(r, area), "{format:?}: {r:?}");
        }
    }
    // The middle is still as the library places it: only the primary's rect, the others derived.
    let centre = add_image_ops_at(&p, &logo, 0, IMAGE_SIZE, Spot::Center).unwrap();
    let after = apply_ops(&p, &centre).unwrap();
    let layout = &layer(&after, &added(&centre)).layout;
    assert!(layout.overrides.is_empty());
    assert_eq!(layout.rect, image_rect(800, 400, p.settings.primary_format));
    // A picture too tall for its corner is as tall as the area less the margins.
    let tall = corner_rect(500, 4000, 0.35, Spot::BottomLeft, Portrait);
    assert!(inside(tall, PORTRAIT_CLEAR), "{tall:?}");
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

/// A sound effect never goes on a track that holds music, even where the music leaves room (before it starts): the
/// Music lane keeps only music.
#[test]
fn a_sound_effect_skips_a_track_that_holds_music() {
    let p = fixture();
    let song = Asset {
        id: "song".into(),
        name: "Song".into(),
        duration_us: secs(120.0),
        ..whoosh()
    };
    let opts = MusicOpts {
        start_us: secs(10.0),
        ..MusicOpts::default()
    };
    let p = apply_ops(&p, &add_music_ops(&p, &song, opts).unwrap()).unwrap();
    let ops = add_sound_ops(&p, &whoosh(), secs(2.0)).unwrap();
    let p = apply_ops(&p, &ops).unwrap();
    let id = added(&ops);
    let track = p
        .timeline
        .tracks
        .iter()
        .find(|t| t.items.iter().any(|i| i.id == id))
        .unwrap();
    assert!(!track.items.iter().any(is_music), "{}", track.name);
}

/// A sound let go at or past the video's end would play over nothing: refused with no room. With no video at all, it
/// plays whole wherever it is put.
#[test]
fn a_sound_at_or_past_the_video_end_is_refused() {
    let p = fixture();
    let end = video_duration(&p.timeline);
    for at in [end, end + secs(3.0)] {
        let e = add_sound_ops(&p, &whoosh(), at).unwrap_err();
        assert_eq!(e.coded().code, "model.sound.noRoom", "at {at}");
    }
    let mut empty = fixture();
    empty.timeline.main.clear();
    let ops = add_sound_ops(&empty, &whoosh(), secs(4.0)).unwrap();
    let empty = apply_ops(&empty, &ops).unwrap();
    assert_eq!(layer(&empty, &added(&ops)).duration_us, secs(0.9));
}

/// The tool that made a file is saved with it and read back as it was; a file nobody made has no `madeWith` written.
#[test]
fn made_with_survives_a_save_and_load() {
    let mut p = fixture();
    p.assets.push(Asset {
        id: "teapot".into(),
        kind: AssetKind::Image,
        path: "/m/Rosa Media/Muse Image/teapot.png".into(),
        name: "teapot.png".into(),
        duration_us: 0,
        width: 512,
        height: 512,
        has_audio: false,
        made_with: Some("Muse Image".into()),
        ..whoosh()
    });
    let json = serde_json::to_string(&p).unwrap();
    let loaded = klipolo_model::load_project(&json).unwrap();
    assert_eq!(loaded.assets, p.assets);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let written: Vec<_> = value["assets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a.get("madeWith").is_some())
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(written, ["teapot"]);
}

/// Two sounds let go on a sound track from the library or Finder go on that track one after the other, each where the
/// "+" would put it: attached to the clip under it, so a later cut over the first removes it and moves the second.
#[test]
fn dropped_sounds_go_one_after_another_and_follow_cuts() {
    use klipolo_model::media::place::{place_ops, Onto};
    let mut p = fixture();
    let mut track = klipolo_model::timeline::Track::new(TrackKind::Audio, "Sounds");
    track.id = "sfx".into();
    p.timeline.tracks.push(track);
    let pop = Asset {
        id: "pop".into(),
        name: "Pop".into(),
        duration_us: secs(0.4),
        ..whoosh()
    };
    let placed = place_ops(&p, &[whoosh(), pop], secs(5.0), &Onto::Track("sfx".into())).unwrap();
    assert!(placed.unplaced.is_empty());
    let ops = placed.ops;
    let p = apply_ops(&p, &ops).unwrap();
    let on: Vec<(&str, i64)> = p.timeline.tracks[0]
        .items
        .iter()
        .map(|i| (i.id.as_str(), i.start_us))
        .collect();
    assert_eq!(on.len(), 2, "both on the track let go on");
    assert_eq!(on[0].1, secs(5.0));
    assert_eq!(on[1].1, secs(5.9), "the second where the first ends");
    assert!(p.timeline.tracks[0]
        .items
        .iter()
        .all(|i| i.attach.is_some()));
    // A cut over all of the first (4.9 s to 5.9 s): it goes, the second moves back by the cut.
    let (first, second) = (on[0].0.to_owned(), on[1].0.to_owned());
    let cut = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: secs(4.9),
                end: secs(5.9),
            }],
        }],
    )
    .unwrap();
    let left: Vec<&Item> = layers(&cut);
    assert!(
        left.iter().all(|i| i.id != first),
        "inside the cut: removed"
    );
    assert_eq!(layer(&cut, &second).start_us, secs(4.9));
}

/// A new track is made once for what is let go on it; a picture among sounds goes where pictures go; a video is
/// refused, and a sound with no room left comes into the project without a layer.
#[test]
fn a_drop_makes_one_new_track_and_keeps_each_kind_on_its_own() {
    use klipolo_model::media::place::{place_ops, Onto};
    let p = fixture();
    let logo = p.asset("logo").unwrap().clone();
    let ops = place_ops(
        &p,
        &[whoosh(), logo, whoosh()],
        secs(2.0),
        &Onto::New(TrackKind::Audio),
    )
    .unwrap()
    .ops;
    let new_tracks = ops
        .iter()
        .filter(|op| matches!(op, Op::AddTrack { .. }))
        .count();
    assert_eq!(new_tracks, 1);
    let p2 = apply_ops(&p, &ops).unwrap();
    let audio: Vec<_> = p2
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && !t.items.is_empty())
        .collect();
    assert_eq!(audio.len(), 1);
    assert_eq!(audio[0].items.len(), 2);
    assert!(p2
        .timeline
        .tracks
        .iter()
        .any(|t| t.kind == TrackKind::Overlay
            && t.items
                .iter()
                .any(|i| matches!(i.content, ItemContent::Image { .. }))));
    let video = p.asset("broll").unwrap().clone();
    assert!(place_ops(&p, &[video], secs(2.0), &Onto::Auto).is_err());
    let end = video_duration(&p.timeline);
    let late = Asset {
        id: "late".into(),
        ..whoosh()
    };
    let e = place_ops(&p, &[late], end, &Onto::Auto).unwrap_err();
    assert_eq!(e.coded().code, "model.sound.noRoom");
}

/// Files that get no layer come into the project all the same and are named, with why; when none gets one, the drop is
/// refused with the first file's own reason (a file with no sound says so, not "no room").
#[test]
fn a_drop_names_the_files_that_got_no_layer_and_refuses_with_the_first_reason() {
    use klipolo_model::media::place::{place_ops, Onto};
    let p = fixture();
    let end = video_duration(&p.timeline);
    // The first fits before the end; the second would start at the end.
    let long = Asset {
        id: "long".into(),
        duration_us: secs(2.0),
        ..whoosh()
    };
    let late = Asset {
        id: "late".into(),
        ..whoosh()
    };
    let placed = place_ops(&p, &[long, late], end - secs(2.0), &Onto::Auto).unwrap();
    assert_eq!(placed.unplaced.len(), 1);
    assert_eq!(placed.unplaced[0].0, 1);
    assert_eq!(placed.unplaced[0].1.coded().code, "model.sound.noRoom");
    let after = apply_ops(&p, &placed.ops).unwrap();
    assert!(
        after.asset("late").is_some(),
        "in the project without a layer"
    );
    // Nothing placed: the first file's reason.
    let silent = Asset {
        id: "silent".into(),
        has_audio: false,
        ..whoosh()
    };
    let e = place_ops(&p, &[silent], secs(1.0), &Onto::Auto).unwrap_err();
    assert_eq!(e.coded().code, "model.sound.noSound");
}

/// A sound let go on a track that holds music goes on a sound track instead, and so does one that would overflow its
/// own track up onto a music track.
#[test]
fn a_dropped_sound_never_lands_on_a_track_that_holds_music() {
    use klipolo_model::media::place::{place_ops, Onto};
    let p = fixture();
    let song = Asset {
        id: "song".into(),
        name: "Song".into(),
        duration_us: secs(120.0),
        ..whoosh()
    };
    let opts = MusicOpts {
        start_us: secs(10.0),
        ..MusicOpts::default()
    };
    let p = apply_ops(&p, &add_music_ops(&p, &song, opts).unwrap()).unwrap();
    let music_track = p
        .timeline
        .tracks
        .iter()
        .find(|t| t.items.iter().any(is_music))
        .unwrap()
        .id
        .clone();
    // Let go on the music track, before the music starts (it has room there).
    let placed = place_ops(
        &p,
        &[whoosh()],
        secs(2.0),
        &Onto::Track(music_track.clone()),
    )
    .unwrap();
    let p2 = apply_ops(&p, &placed.ops).unwrap();
    let on = |p: &Project, asset: &str| {
        p.timeline
            .tracks
            .iter()
            .find(|t| {
                t.items
                    .iter()
                    .any(|i| matches!(&i.content, ItemContent::Audio { asset_id, .. } if asset_id == asset))
            })
            .unwrap()
            .clone()
    };
    assert!(!on(&p2, "whoosh").items.iter().any(is_music));
    // A sound track below the music, already holding a sound at 2 s: the next one there goes up, past the music.
    let mut q = p.clone();
    let mut sfx = klipolo_model::timeline::Track::new(TrackKind::Audio, "Sounds");
    sfx.id = "sfx".into();
    q.timeline.tracks.insert(0, sfx);
    let first = place_ops(&q, &[whoosh()], secs(2.0), &Onto::Track("sfx".into())).unwrap();
    let q = apply_ops(&q, &first.ops).unwrap();
    let pop = Asset {
        id: "pop".into(),
        ..whoosh()
    };
    let second = place_ops(&q, &[pop], secs(2.0), &Onto::Track("sfx".into())).unwrap();
    let q = apply_ops(&q, &second.ops).unwrap();
    let landed = on(&q, "pop");
    assert_ne!(landed.id, music_track);
    assert!(!landed.items.iter().any(is_music));
}
