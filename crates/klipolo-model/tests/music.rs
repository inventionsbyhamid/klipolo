//! Background music that fits the video (`media::music`): a short song tiles back to back and a long one is cut at the
//! video's end, fading out there, on an audio track of its own; after an edit that moves the video's end, music that
//! reached the old end is fitted to the new one in the same undo step.
mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::media::music::{
    add_music_ops, apply_with_music_refit, is_music, refit_music_ops, with_music_refit, MusicOpts,
    MUSIC_FADE_IN_US, MUSIC_FADE_OUT_US,
};
use klipolo_model::ops::{ClipPatch, TrackTarget};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::query::video_duration;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind};
use klipolo_model::{apply_ops, validate, Op, Project, RangeUs, Transaction};

/// A plain video `len` seconds long (one clip of a 10 minute take), with an 8 s loop and a 2 minute song.
fn video(len: f64) -> Project {
    let mut p = fixture();
    p.assets.push(file("take", AssetKind::Video, 600.0));
    p.assets.push(file("loop", AssetKind::Audio, 8.0));
    p.assets.push(file("song", AssetKind::Audio, 120.0));
    p.timeline.main = vec![take(0.0, len)];
    p
}

fn file(id: &str, kind: AssetKind, seconds: f64) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs(seconds),
        width: if kind == AssetKind::Video { 1920 } else { 0 },
        height: if kind == AssetKind::Video { 1080 } else { 0 },
        fps: if kind == AssetKind::Video { 30.0 } else { 0.0 },
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }
}

/// `take` from `from` to `to` seconds.
fn take(from: f64, to: f64) -> Clip {
    let mut c = Clip::new(
        ClipSource::Asset { id: "take".into() },
        secs(from),
        secs(to),
    );
    c.id = format!("take-{from}");
    c
}

fn asset<'a>(p: &'a Project, id: &str) -> &'a Asset {
    p.asset(id).unwrap()
}

/// `p` with `asset` added as music (`opts`), as one batch.
fn with_music(p: &Project, asset: &str, opts: MusicOpts) -> Project {
    let ops = add_music_ops(p, self::asset(p, asset), opts).unwrap();
    apply_ops(p, &ops).unwrap_or_else(|e| panic!("refused: {e}"))
}

fn tx(ops: Vec<Op>) -> Transaction {
    Transaction {
        label: "edit".into(),
        ops,
        origin: None,
        ..Default::default()
    }
}

fn cut(from: f64, to: f64) -> Op {
    Op::DeleteRanges {
        ranges: vec![RangeUs {
            start: secs(from),
            end: secs(to),
        }],
    }
}

fn speed(clip: &str, speed: f64) -> Op {
    Op::UpdateClip {
        id: clip.into(),
        patch: ClipPatch {
            speed: Some(speed),
            ..ClipPatch::default()
        },
    }
}

/// A sound layer of the 8 s loop, `start` to `start + len` seconds, ducking `duck` (None: a sound effect).
fn sound_layer(id: &str, start: f64, len: f64, duck: Option<f64>) -> Item {
    Item {
        content: ItemContent::Audio {
            asset_id: "loop".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck,
        },
        ..still_item(id, start, len)
    }
}

/// The free music layers (the beds), by start.
fn beds(p: &Project) -> Vec<&Item> {
    music(p)
        .into_iter()
        .filter(|i| i.attach.is_none())
        .collect()
}

/// The music layers, by start.
fn music(p: &Project) -> Vec<&Item> {
    let mut items: Vec<&Item> = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .filter(|i| is_music(i))
        .collect();
    items.sort_by_key(|i| i.start_us);
    items
}

/// Start and end of each layer, in seconds.
fn spans(items: &[&Item]) -> Vec<(f64, f64)> {
    items
        .iter()
        .map(|i| (to_secs(i.start_us), to_secs(i.end_us())))
        .collect()
}

fn to_secs(us: Us) -> f64 {
    us as f64 / 1e6
}

struct Sound {
    asset: String,
    source_in: Us,
    volume: f64,
    fade_in: Us,
    fade_out: Us,
    duck: Option<f64>,
}

fn sound(item: &Item) -> Sound {
    match &item.content {
        ItemContent::Audio {
            asset_id,
            source_in_us,
            volume,
            fade_in_us,
            fade_out_us,
            duck,
            ..
        } => Sound {
            asset: asset_id.clone(),
            source_in: *source_in_us,
            volume: *volume,
            fade_in: *fade_in_us,
            fade_out: *fade_out_us,
            duck: *duck,
        },
        other => panic!("not a sound layer: {other:?}"),
    }
}

fn track_of<'a>(p: &'a Project, id: &str) -> &'a Track {
    p.timeline
        .tracks
        .iter()
        .find(|t| t.items.iter().any(|i| i.id == id))
        .unwrap_or_else(|| panic!("no layer {id}"))
}

/// Every music layer is free, on one audio track that holds nothing else, at 30 % and ducking 12 dB; each piece after
/// the first plays the song from its start; the first fades in and the last fades out, nothing else fades.
fn assert_one_bed(p: &Project, asset: &str) {
    let m = music(p);
    assert!(!m.is_empty(), "no music");
    let track = track_of(p, &m[0].id);
    assert_eq!(track.kind, TrackKind::Audio);
    assert_eq!(
        track.items.len(),
        m.len(),
        "the music's track holds only it"
    );
    let last = m.len() - 1;
    for (n, item) in m.iter().enumerate() {
        let s = sound(item);
        assert_eq!(track_of(p, &item.id).id, track.id, "{}", item.id);
        assert_eq!(item.attach, None, "free, so cuts never chop it");
        assert_eq!(s.asset, asset);
        assert_eq!(s.volume, 0.3);
        assert_eq!(s.duck, Some(-12.0));
        if n > 0 {
            assert_eq!(s.source_in, 0, "piece {n} starts the song again");
            assert_eq!(item.start_us, m[n - 1].end_us(), "back to back");
            assert_eq!(s.fade_in, 0, "piece {n} doesn't fade in");
        }
        if n < last {
            assert_eq!(s.fade_out, 0, "piece {n} doesn't fade out");
        }
    }
}

fn fade_in(item: &Item) -> f64 {
    to_secs(sound(item).fade_in)
}

fn fade_out(item: &Item) -> f64 {
    to_secs(sound(item).fade_out)
}

#[test]
fn short_song_tiles_to_the_video_end() {
    let p = with_music(&video(30.0), "loop", MusicOpts::default());
    validate(&p).unwrap();
    let m = music(&p);
    assert_eq!(
        spans(&m),
        [(0.0, 8.0), (8.0, 16.0), (16.0, 24.0), (24.0, 30.0)],
        "8 s pieces back to back, the last cut at the end"
    );
    assert_one_bed(&p, "loop");
    assert_eq!(sound(m[0]).source_in, 0);
    assert_eq!(fade_in(m[0]), 0.5);
    assert_eq!(
        fade_out(m[3]),
        2.0,
        "fades out over 2 s ending at the video end"
    );
    assert_eq!(m[3].end_us(), video_duration(&p.timeline));
}

#[test]
fn long_song_is_cut_with_fade_out_at_the_end() {
    let p = with_music(&video(30.0), "song", MusicOpts::default());
    let m = music(&p);
    assert_eq!(spans(&m), [(0.0, 30.0)], "one piece, cut at the video end");
    assert_one_bed(&p, "song");
    assert_eq!(sound(m[0]).source_in, 0);
    assert_eq!((fade_in(m[0]), fade_out(m[0])), (0.5, 2.0));

    // From a moment to a moment, as the chat may ask: the song starts there and fades out where it is told to end.
    let opts = MusicOpts {
        volume: 0.5,
        duck_db: Some(-6.0),
        start_us: secs(4.0),
        end_us: Some(secs(20.0)),
    };
    let p = with_music(&video(30.0), "song", opts);
    let m = music(&p);
    assert_eq!(spans(&m), [(4.0, 20.0)]);
    let s = sound(m[0]);
    assert_eq!((s.volume, s.duck), (0.5, Some(-6.0)));
    assert_eq!((fade_in(m[0]), fade_out(m[0])), (0.5, 2.0));

    // An end past the video's is cut at the video's: nothing plays after the picture stops.
    let opts = MusicOpts {
        end_us: Some(secs(90.0)),
        ..MusicOpts::default()
    };
    let p = with_music(&video(30.0), "song", opts);
    assert_eq!(spans(&music(&p)), [(0.0, 30.0)]);
}

#[test]
fn cut_in_the_middle_refits_music_in_the_same_step() {
    for (song, before_cut, after_cut) in [
        (
            "loop",
            vec![(0.0, 8.0), (8.0, 16.0), (16.0, 24.0), (24.0, 30.0)],
            vec![(0.0, 8.0), (8.0, 16.0), (16.0, 24.0), (24.0, 28.0)],
        ),
        ("song", vec![(0.0, 30.0)], vec![(0.0, 28.0)]),
    ] {
        let mut doc = Document::new(video(30.0)).unwrap();
        let p = doc.project().clone();
        doc.apply(&tx(add_music_ops(
            &p,
            asset(&p, song),
            MusicOpts::default(),
        )
        .unwrap()))
            .unwrap();
        let with_bed = doc.project().clone();
        assert_eq!(spans(&music(&with_bed)), before_cut, "{song}");

        // The cut, as the timeline sends it, comes back with the music's refit after it, and the project it makes.
        let ops = with_music_refit(doc.project(), vec![cut(10.0, 12.0)]).unwrap();
        assert_eq!(ops[0], cut(10.0, 12.0), "the edit itself goes first");
        assert!(ops.len() > 1, "{song}: no refit in {ops:?}");
        assert_eq!(
            ops.len(),
            2,
            "{song}: the cut, then one change to the last piece: {ops:?}"
        );
        let done = apply_with_music_refit(doc.project(), vec![cut(10.0, 12.0)]).unwrap();
        assert_eq!(done.ops, ops);
        assert_eq!(done.refused, None);
        assert_eq!(done.project, apply_ops(doc.project(), &ops).unwrap());
        doc.apply(&tx(ops)).unwrap();

        let p = doc.project();
        assert_eq!(video_duration(&p.timeline), secs(28.0));
        let m = music(p);
        assert_eq!(
            spans(&m),
            after_cut,
            "{song}: the music still ends with the video"
        );
        assert_one_bed(p, song);
        assert_eq!(
            fade_out(m[m.len() - 1]),
            2.0,
            "{song}: the fade-out moved to the end"
        );
        assert_eq!(fade_in(m[0]), 0.5, "{song}");

        // One undo takes back the cut and the refit together.
        doc.undo().unwrap();
        assert_eq!(*doc.project(), with_bed, "{song}");
    }
}

#[test]
fn a_cut_past_a_piece_removes_it_and_more_footage_adds_pieces() {
    let p = with_music(&video(30.0), "loop", MusicOpts::default());

    // Cutting 21 s to 30 s leaves 21 s: the third piece is cut short and the fourth goes.
    let ops = with_music_refit(&p, vec![cut(21.0, 30.0)]).unwrap();
    let short = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        spans(&music(&short)),
        [(0.0, 8.0), (8.0, 16.0), (16.0, 21.0)]
    );
    assert_one_bed(&short, "loop");
    assert_eq!(fade_out(music(&short)[2]), 2.0);

    // 19 s more footage at the end: the short piece plays on to the song's end and new pieces follow to the new end.
    let more = vec![Op::InsertClip {
        clip: take(100.0, 119.0),
        index: None,
    }];
    let ops = with_music_refit(&short, more).unwrap();
    let long = apply_ops(&short, &ops).unwrap();
    assert_eq!(video_duration(&long.timeline), secs(40.0));
    let m = music(&long);
    assert_eq!(
        spans(&m),
        [
            (0.0, 8.0),
            (8.0, 16.0),
            (16.0, 24.0),
            (24.0, 32.0),
            (32.0, 40.0)
        ]
    );
    assert_one_bed(&long, "loop");
    assert_eq!(
        fade_out(m[4]),
        2.0,
        "the 2 s fade-out is back, ending at the new end"
    );
    assert_eq!(
        sound(m[2]).source_in,
        0,
        "the third piece kept its place in the song"
    );
    // As one step: one undo takes back the footage and the music's new pieces together.
    let mut doc = Document::new(short.clone()).unwrap();
    doc.apply(&tx(ops)).unwrap();
    assert_eq!(*doc.project(), long);
    doc.undo().unwrap();
    assert_eq!(*doc.project(), short);

    // A long song grows with the video up to the song's own length, then starts again.
    let p = with_music(&video(100.0), "song", MusicOpts::default());
    let more = vec![Op::InsertClip {
        clip: take(200.0, 250.0),
        index: None,
    }];
    let long = apply_ops(&p, &with_music_refit(&p, more).unwrap()).unwrap();
    assert_eq!(spans(&music(&long)), [(0.0, 120.0), (120.0, 150.0)]);
    assert_one_bed(&long, "song");
}

#[test]
fn music_not_touching_the_end_is_left_alone() {
    // Music told to end at 20 s, and a sound effect that runs to the video's end.
    let opts = MusicOpts {
        end_us: Some(secs(20.0)),
        ..MusicOpts::default()
    };
    let mut p = with_music(&video(30.0), "loop", opts);
    let whoosh = Item {
        content: ItemContent::Audio {
            asset_id: "loop".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        ..still_item("whoosh", 25.0, 5.0)
    };
    assert!(
        !is_music(&whoosh),
        "a sound that doesn't duck is a sound effect"
    );
    apply(
        &mut p,
        Op::AddItem {
            track: klipolo_model::ops::TrackTarget::Auto,
            item: whoosh,
            attach: false,
        },
    );
    let edit = vec![cut(2.0, 6.0)];
    let after = apply_ops(&p, &edit).unwrap();
    assert!(refit_music_ops(&p, &after).is_empty());
    assert_eq!(with_music_refit(&p, edit.clone()).unwrap(), edit);
    assert_eq!(
        spans(&music(&after)),
        [(0.0, 8.0), (8.0, 16.0), (16.0, 20.0)]
    );
    assert_eq!(item_by_id(&after, "whoosh").end_us(), secs(30.0));

    // An edit that leaves the video's end where it was changes no music, even music that reaches it.
    let p = with_music(&video(30.0), "loop", MusicOpts::default());
    let rename = vec![Op::Rename {
        name: "Renamed".into(),
    }];
    assert_eq!(with_music_refit(&p, rename.clone()).unwrap(), rename);

    // Music on a locked track stays as it is: a refit would make the edit fail.
    let mut locked = p.clone();
    let id = music(&p)[0].id.clone();
    for t in &mut locked.timeline.tracks {
        if t.items.iter().any(|i| i.id == id) {
            t.locked = true;
        }
    }
    let edit = vec![cut(2.0, 6.0)];
    let after = apply_ops(&locked, &edit).unwrap();
    assert!(refit_music_ops(&locked, &after).is_empty());
    assert_eq!(with_music_refit(&locked, edit.clone()).unwrap(), edit);
    // And an edit that is refused is refused as it was.
    assert!(with_music_refit(&p, vec![Op::RemoveClip { id: "nope".into() }]).is_err());
}

#[test]
fn fades_never_exceed_half_the_length() {
    let check = |p: &Project, what: &str| {
        validate(p).unwrap_or_else(|e| panic!("{what}: {e}"));
        for item in music(p) {
            let s = sound(item);
            assert!(
                s.fade_in * 2 <= item.duration_us && s.fade_out * 2 <= item.duration_us,
                "{what}: {} is {} µs with fades {} and {}",
                item.id,
                item.duration_us,
                s.fade_in,
                s.fade_out
            );
        }
    };
    for len in [
        0.02, 0.3, 0.9, 1.0, 3.0, 4.1, 8.0, 8.5, 9.0, 17.0, 33.0, 130.0,
    ] {
        for song in ["loop", "song"] {
            let p = with_music(&video(len), song, MusicOpts::default());
            check(&p, &format!("{song} over {len} s"));
            // At the end, or under 4 s before it after playing the file to its end (the song doesn't start again
            // for a moment).
            let last = *music(&p).last().unwrap();
            let early = secs(len) - last.end_us();
            let whole = sound(last).source_in + last.duration_us == asset(&p, song).duration_us;
            assert!(
                early == 0 || (whole && early > 0 && early < secs(4.0)),
                "{song} over {len} s ends {early} µs early"
            );
        }
    }
    // A 3 s video: fade in 0.5 s, and out over half of it.
    let p = with_music(&video(3.0), "song", MusicOpts::default());
    let m = music(&p);
    assert_eq!((fade_in(m[0]), fade_out(m[0])), (0.5, 1.5));
    // 4.1 s of a 4.1 s video and a 120 s song: one piece, so nothing to end early on.
    let p = with_music(&video(4.1), "song", MusicOpts::default());
    assert_eq!(spans(&music(&p)), [(0.0, 4.1)]);

    // Cuts that leave a short last piece, or a very short video, keep them inside too, and growing back restores them.
    let p = with_music(&video(30.0), "loop", MusicOpts::default());
    for (from, to) in [(5.0, 10.0), (0.5, 29.0), (0.0, 29.98), (1.0, 23.5)] {
        let short = apply_ops(&p, &with_music_refit(&p, vec![cut(from, to)]).unwrap()).unwrap();
        check(&short, &format!("cut {from} to {to}"));
        let back = vec![Op::InsertClip {
            clip: take(300.0, 300.0 + (to - from)),
            index: None,
        }];
        let grown = apply_ops(&short, &with_music_refit(&short, back).unwrap()).unwrap();
        check(&grown, &format!("cut {from} to {to}, then grown back"));
        let m = music(&grown);
        assert_eq!(m.last().unwrap().end_us(), secs(30.0), "cut {from} to {to}");
        assert_eq!(
            (fade_in(m[0]), fade_out(m[m.len() - 1])),
            (0.5, 2.0),
            "cut {from} to {to}: the fades are back"
        );
    }
}

#[test]
fn music_goes_on_an_audio_track_of_its_own() {
    // A sound effect on the only audio track: the music gets a new track.
    let mut p = video(30.0);
    let mut sfx = Track::new(TrackKind::Audio, "Audio 1");
    sfx.id = "sfx".into();
    sfx.items.push(Item {
        content: ItemContent::Audio {
            asset_id: "loop".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        ..still_item("ding", 26.0, 1.0)
    });
    p.timeline.tracks.push(sfx);
    let with = with_music(&p, "loop", MusicOpts::default());
    let track = track_of(&with, &music(&with)[0].id);
    assert_ne!(track.id, "sfx");
    assert_one_bed(&with, "loop");

    // An empty audio track is used; a locked one isn't.
    let mut p = video(30.0);
    let mut empty = Track::new(TrackKind::Audio, "Audio 1");
    empty.id = "empty".into();
    let mut locked = empty.clone();
    locked.id = "locked".into();
    locked.locked = true;
    p.timeline.tracks = vec![locked, empty];
    let with = with_music(&p, "loop", MusicOpts::default());
    assert_eq!(track_of(&with, &music(&with)[0].id).id, "empty");

    // Ducking off still makes it music (0 dB), so it fits the video as it changes.
    let opts = MusicOpts {
        duck_db: None,
        ..MusicOpts::default()
    };
    let p = with_music(&video(30.0), "song", opts);
    let m = music(&p);
    assert_eq!(m.len(), 1);
    assert_eq!(sound(m[0]).duck, Some(0.0));

    // A file that isn't in the project yet comes in with the music, in the same batch.
    let p = video(30.0);
    let mut fresh = asset(&p, "song").clone();
    fresh.id = "fresh".into();
    fresh.path = "/media/fresh".into();
    let ops = add_music_ops(&p, &fresh, MusicOpts::default()).unwrap();
    let with = apply_ops(&p, &ops).unwrap();
    assert!(with.asset("fresh").is_some());
    assert_eq!(sound(music(&with)[0]).asset, "fresh");

    // A file without sound, or no room before the end, is refused with words of its own.
    let p = video(30.0);
    let mut silent = asset(&p, "take").clone();
    silent.has_audio = false;
    let e = add_music_ops(&p, &silent, MusicOpts::default()).unwrap_err();
    assert_eq!(e.coded().code, "model.music.noSound");
    assert_eq!(e, refused("That file has no sound to play as music."));
    let no_room = "Music has to start before the video ends, and end after it starts.";
    let late = MusicOpts {
        start_us: secs(30.0),
        ..MusicOpts::default()
    };
    let backwards = MusicOpts {
        start_us: secs(10.0),
        end_us: Some(secs(5.0)),
        ..MusicOpts::default()
    };
    for opts in [late, backwards] {
        let e = add_music_ops(&p, asset(&p, "song"), opts).unwrap_err();
        assert_eq!(e.coded().code, "model.music.noRoom", "{opts:?}");
        assert_eq!(e, refused(no_room), "{opts:?}");
    }
}

#[test]
fn music_added_before_any_footage_plays_whole_then_fits_the_video() {
    let mut empty = video(30.0);
    empty.timeline.main.clear();
    let p = with_music(&empty, "song", MusicOpts::default());
    assert_eq!(spans(&music(&p)), [(0.0, 120.0)], "the whole song");

    let first = vec![Op::InsertClip {
        clip: take(0.0, 45.0),
        index: None,
    }];
    let p = apply_ops(&p, &with_music_refit(&p, first).unwrap()).unwrap();
    let m = music(&p);
    assert_eq!(spans(&m), [(0.0, 45.0)]);
    assert_eq!((fade_in(m[0]), fade_out(m[0])), (0.5, 2.0));
    assert_eq!(MUSIC_FADE_IN_US, secs(0.5));
    assert_eq!(MUSIC_FADE_OUT_US, secs(2.0));

    // Deleting every clip to start over keeps the music for the footage that comes next.
    let gone = vec![Op::RemoveClip {
        id: "take-0".into(),
    }];
    assert_eq!(with_music_refit(&p, gone.clone()).unwrap(), gone);

    // Music from 5 s on an empty video fits the first footage as well.
    let late = MusicOpts {
        start_us: secs(5.0),
        ..MusicOpts::default()
    };
    let p = with_music(&empty, "song", late);
    assert_eq!(spans(&music(&p)), [(5.0, 125.0)]);
    let first = vec![Op::InsertClip {
        clip: take(0.0, 45.0),
        index: None,
    }];
    let p = apply_ops(&p, &with_music_refit(&p, first).unwrap()).unwrap();
    let m = music(&p);
    assert_eq!(spans(&m), [(5.0, 45.0)]);
    assert_eq!((fade_in(m[0]), fade_out(m[0])), (0.5, 2.0));
}

#[test]
fn music_a_hair_short_of_the_end_still_follows_it() {
    // 16.005 s of an 8 s loop: two whole pieces, and the last 5 ms (under the shortest layer) left silent.
    let p = with_music(&video(16.005), "loop", MusicOpts::default());
    assert_eq!(spans(&music(&p)), [(0.0, 8.0), (8.0, 16.0)]);
    let short = apply_ops(&p, &with_music_refit(&p, vec![cut(2.0, 4.0)]).unwrap()).unwrap();
    let m = music(&short);
    assert_eq!(spans(&m), [(0.0, 8.0), (8.0, 14.005)]);
    assert_eq!(fade_out(m[1]), 2.0);

    // A song set by hand to end 5 ms before the video does still follows the end when more footage comes.
    let opts = MusicOpts {
        end_us: Some(secs(29.995)),
        ..MusicOpts::default()
    };
    let p = with_music(&video(30.0), "song", opts);
    let more = vec![Op::InsertClip {
        clip: take(100.0, 110.0),
        index: None,
    }];
    let long = apply_ops(&p, &with_music_refit(&p, more).unwrap()).unwrap();
    let m = music(&long);
    assert_eq!(spans(&m), [(0.0, 40.0)]);
    assert_eq!(fade_out(m[0]), 2.0);
}

#[test]
fn music_stops_at_the_next_layer_on_its_track() {
    // A sound placed on the music's own track after the video's end: more footage plays the music up to it, not over
    // it or onto another track.
    let mut p = with_music(&video(30.0), "loop", MusicOpts::default());
    let track = track_of(&p, &music(&p)[0].id).id.clone();
    let tail = Item {
        content: ItemContent::Audio {
            asset_id: "loop".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        ..still_item("tail", 37.0, 1.0)
    };
    apply(
        &mut p,
        Op::AddItem {
            track: klipolo_model::ops::TrackTarget::Track { id: track.clone() },
            item: tail,
            attach: false,
        },
    );
    let more = vec![Op::InsertClip {
        clip: take(100.0, 110.0),
        index: None,
    }];
    let long = apply_ops(&p, &with_music_refit(&p, more).unwrap()).unwrap();
    let m = music(&long);
    assert_eq!(
        spans(&m),
        [
            (0.0, 8.0),
            (8.0, 16.0),
            (16.0, 24.0),
            (24.0, 32.0),
            (32.0, 37.0)
        ]
    );
    assert!(m.iter().all(|i| track_of(&long, &i.id).id == track));
    assert_eq!(fade_out(m[4]), 2.0, "fades out as it stops");
    assert_eq!(item_by_id(&long, "tail").start_us, secs(37.0));
}

#[test]
fn speed_change_refits_music() {
    for (song, fast, back) in [
        ("song", vec![(0.0, 15.0)], vec![(0.0, 30.0)]),
        (
            "loop",
            vec![(0.0, 8.0), (8.0, 15.0)],
            vec![(0.0, 8.0), (8.0, 16.0), (16.0, 24.0), (24.0, 30.0)],
        ),
    ] {
        let p = with_music(&video(30.0), song, MusicOpts::default());
        // Twice as fast: the video is 15 s and the music ends there, fading out over its last 2 s.
        let p = apply_ops(
            &p,
            &with_music_refit(&p, vec![speed("take-0", 2.0)]).unwrap(),
        )
        .unwrap();
        assert_eq!(video_duration(&p.timeline), secs(15.0));
        let m = music(&p);
        assert_eq!(spans(&m), fast, "{song} at 2x");
        assert_one_bed(&p, song);
        assert_eq!(fade_out(m[m.len() - 1]), 2.0, "{song} at 2x");
        // Back to normal speed: 30 s again, the music fills it with its fades in place.
        let p = apply_ops(
            &p,
            &with_music_refit(&p, vec![speed("take-0", 1.0)]).unwrap(),
        )
        .unwrap();
        let m = music(&p);
        assert_eq!(spans(&m), back, "{song} back at 1x");
        assert_one_bed(&p, song);
        assert_eq!(
            (fade_in(m[0]), fade_out(m[m.len() - 1])),
            (0.5, 2.0),
            "{song} back at 1x"
        );
    }
}

#[test]
fn attached_sounds_follow_the_cut_and_music_refits() {
    let mut p = with_music(&video(30.0), "song", MusicOpts::default());
    // Sounds tied to their moments: a whoosh inside the cut, one after it, and a ducked sting from 26 s that runs on
    // 4 s past the end. The sting ducks, so it counts as music, but it is attached: the refit never touches it.
    for item in [
        sound_layer("inside", 10.5, 1.0, None),
        sound_layer("after", 20.0, 1.0, None),
        sound_layer("sting", 26.0, 8.0, Some(-12.0)),
    ] {
        apply(
            &mut p,
            Op::AddItem {
                track: TrackTarget::Auto,
                item,
                attach: true,
            },
        );
    }
    let ops = with_music_refit(&p, vec![cut(10.0, 12.0)]).unwrap();
    let after = apply_ops(&p, &ops).unwrap();
    assert_eq!(video_duration(&after.timeline), secs(28.0));

    // The bed ends with the video, fading out there.
    let bed = beds(&after);
    assert_eq!(spans(&bed), [(0.0, 28.0)]);
    assert_eq!((fade_in(bed[0]), fade_out(bed[0])), (0.5, 2.0));
    // The whoosh inside the cut is gone; the one after it moved back by the cut's 2 s.
    assert!(after
        .timeline
        .tracks
        .iter()
        .all(|t| t.items.iter().all(|i| i.id != "inside")));
    let moved = item_by_id(&after, "after");
    assert_eq!((moved.start_us, moved.end_us()), (secs(18.0), secs(19.0)));
    // The sting moved with its moment and kept its whole length and fades.
    let sting = item_by_id(&after, "sting");
    assert!(sting.attach.is_some());
    assert_eq!((sting.start_us, sting.end_us()), (secs(24.0), secs(32.0)));
    assert_eq!((fade_in(sting), fade_out(sting)), (0.0, 0.0));
    assert!(ops.iter().all(
        |op| !matches!(op, Op::UpdateItem { id, .. } | Op::RemoveItem { id } if id == "sting")
    ));
}

#[test]
fn a_short_remainder_ends_the_music_after_the_last_whole_piece() {
    // 17 s of an 8 s loop: two whole pieces and 1 s left. The song doesn't start again for a moment: it ends after the
    // second piece, fading out over its last 2 s, 1 s before the video does.
    let p = with_music(&video(17.0), "loop", MusicOpts::default());
    let m = music(&p);
    assert_eq!(spans(&m), [(0.0, 8.0), (8.0, 16.0)]);
    assert_one_bed(&p, "loop");
    assert_eq!((fade_in(m[0]), fade_out(m[1])), (0.5, 2.0));

    // A cut that leaves the same remainder ends it the same way, and music ending that way still follows the video.
    let p = with_music(&video(30.0), "loop", MusicOpts::default());
    let short = apply_ops(&p, &with_music_refit(&p, vec![cut(10.0, 23.0)]).unwrap()).unwrap();
    let m = music(&short);
    assert_eq!(spans(&m), [(0.0, 8.0), (8.0, 16.0)]);
    assert_eq!(fade_out(m[1]), 2.0);
    let more = vec![Op::InsertClip {
        clip: take(100.0, 113.0),
        index: None,
    }];
    let long = apply_ops(&short, &with_music_refit(&short, more).unwrap()).unwrap();
    let m = music(&long);
    assert_eq!(
        spans(&m),
        [(0.0, 8.0), (8.0, 16.0), (16.0, 24.0), (24.0, 30.0)]
    );
    assert_one_bed(&long, "loop");
    assert_eq!(fade_out(m[3]), 2.0);

    // 4 s or more is kept: 20 s ends with a 4 s piece fading out at the end.
    let p = with_music(&video(20.0), "loop", MusicOpts::default());
    let m = music(&p);
    assert_eq!(spans(&m), [(0.0, 8.0), (8.0, 16.0), (16.0, 20.0)]);
    assert_eq!(fade_out(m[2]), 2.0);
}

#[test]
fn music_crossing_the_new_end_is_cut_there_with_its_fade_out() {
    // Music told to end at 20 s doesn't reach the 30 s end, but a cut to 15 s ends the video inside it: it is cut there
    // and fades out there, instead of stopping dead past the end.
    let opts = MusicOpts {
        end_us: Some(secs(20.0)),
        ..MusicOpts::default()
    };
    let p = with_music(&video(30.0), "loop", opts);
    assert_eq!(spans(&music(&p)), [(0.0, 8.0), (8.0, 16.0), (16.0, 20.0)]);
    let short = apply_ops(&p, &with_music_refit(&p, vec![cut(10.0, 25.0)]).unwrap()).unwrap();
    assert_eq!(video_duration(&short.timeline), secs(15.0));
    let m = music(&short);
    assert_eq!(spans(&m), [(0.0, 8.0), (8.0, 15.0)]);
    assert_one_bed(&short, "loop");
    assert_eq!(fade_out(m[1]), 2.0);
}

#[test]
fn a_refit_leaves_hand_set_fades_on_pieces_it_does_not_move() {
    // A 5 s fade-in set by hand on the first 8 s piece: more than half of it, which a person may choose.
    let mut p = with_music(&video(30.0), "loop", MusicOpts::default());
    let first = music(&p)[0].clone();
    let mut content = first.content.clone();
    if let ItemContent::Audio { fade_in_us, .. } = &mut content {
        *fade_in_us = secs(5.0);
    }
    apply(
        &mut p,
        Op::UpdateItem {
            id: first.id.clone(),
            patch: content_patch(content),
        },
    );
    // A cut in the middle changes the last piece only: the first keeps its fade-in.
    let ops = with_music_refit(&p, vec![cut(10.0, 12.0)]).unwrap();
    assert_eq!(ops.len(), 2, "the cut, then the last piece: {ops:?}");
    let after = apply_ops(&p, &ops).unwrap();
    let m = music(&after);
    assert_eq!(
        spans(&m),
        [(0.0, 8.0), (8.0, 16.0), (16.0, 24.0), (24.0, 28.0)]
    );
    assert_eq!(fade_in(m[0]), 5.0);
    assert_eq!(fade_out(m[3]), 2.0);
}

#[test]
fn music_past_the_first_footage_on_an_empty_video_is_left_alone() {
    // Two songs on an empty video: one from the start, one placed from 60 s.
    let mut empty = video(30.0);
    empty.timeline.main.clear();
    let p = with_music(&empty, "song", MusicOpts::default());
    let later = MusicOpts {
        start_us: secs(60.0),
        ..MusicOpts::default()
    };
    let p = with_music(&p, "loop", later);
    assert_eq!(spans(&music(&p)), [(0.0, 120.0), (60.0, 68.0)]);
    // 45 s of footage: the first song fits it; the second, which starts after it, stays as it was placed.
    let first = vec![Op::InsertClip {
        clip: take(0.0, 45.0),
        index: None,
    }];
    let after = apply_ops(&p, &with_music_refit(&p, first).unwrap()).unwrap();
    let m = music(&after);
    assert_eq!(spans(&m), [(0.0, 45.0), (60.0, 68.0)]);
    assert_eq!((fade_in(m[1]), fade_out(m[1])), (0.5, 2.0));
}

#[test]
fn new_pieces_get_fresh_ids_when_the_numbers_run_out() {
    // An id `{first piece}-18446744073709551615` (the largest number there is) in the project: new pieces can't be
    // numbered past it, so they get fresh ids instead.
    let mut p = with_music(&video(16.0), "loop", MusicOpts::default());
    let base = music(&p)[0].id.clone();
    let last = format!("{base}-{}", u64::MAX);
    apply(
        &mut p,
        Op::AddTrack {
            id: Some(last.clone()),
            kind: TrackKind::Audio,
            index: None,
        },
    );
    let more = vec![Op::InsertClip {
        clip: take(100.0, 114.0),
        index: None,
    }];
    let done = apply_with_music_refit(&p, more).unwrap();
    assert_eq!(done.refused, None);
    let m = music(&done.project);
    assert_eq!(
        spans(&m),
        [(0.0, 8.0), (8.0, 16.0), (16.0, 24.0), (24.0, 30.0)]
    );
    for piece in &m[2..] {
        assert!(!piece.id.starts_with(&format!("{base}-")), "{}", piece.id);
    }
    assert_ne!(m[2].id, m[3].id);
}
