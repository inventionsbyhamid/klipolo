//! Background music that fits the video (`media::music`): a short song tiles back to back and a long one is cut at the
//! video's end, fading out there, on an audio track of its own.
mod common;
use common::*;
use klipolo_model::media::music::{add_music_ops, is_music, MusicOpts};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::query::video_duration;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind};
use klipolo_model::{apply_ops, validate, Project};

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
            assert_eq!(
                music(&p).last().unwrap().end_us(),
                secs(len),
                "{song} over {len} s"
            );
        }
    }
    // A 3 s video: fade in 0.5 s, and out over half of it.
    let p = with_music(&video(3.0), "song", MusicOpts::default());
    let m = music(&p);
    assert_eq!((fade_in(m[0]), fade_out(m[0])), (0.5, 1.5));
    // 17 s of an 8 s loop: the last piece is 1 s and fades over its half.
    let p = with_music(&video(17.0), "loop", MusicOpts::default());
    assert_eq!(fade_out(music(&p)[2]), 0.5);
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
    assert!(add_music_ops(&p, &silent, MusicOpts::default()).is_err());
    let late = MusicOpts {
        start_us: secs(30.0),
        ..MusicOpts::default()
    };
    assert!(add_music_ops(&p, asset(&p, "song"), late).is_err());
}
