//! Background music that fits the video. A song shorter than the video plays again and again, in pieces back to back
//! (the last cut at the video's end); a longer one is cut there. It fades in at its start and out at its end, sits on
//! an audio track of its own, stays free (cuts never chop it) and ducks under speech. The engine has no looping: the
//! pieces are plain sound layers of the same file, and each seam gets the mixer's 3 ms edge ramps.
//!
//! **What counts as music** ([`is_music`]): a sound layer with `duck` set. Music added here always sets it, to 0 dB
//! when ducking is off (`ItemContent::Audio::duck` at 0 lowers nothing), so turning ducking off keeps a song music;
//! `None` is a sound effect. The timeline's Music and Sounds lanes split on it.

use std::ops::Range;

use crate::error::{invalid, Result};
use crate::ids::{new_id, Id};
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::code;
use crate::ops::{Op, TrackTarget};
use crate::project::{Asset, Project};
use crate::query::video_duration;
use crate::time::{Us, MIN_DURATION_US};
use crate::timeline::{Item, ItemContent, Track, TrackKind};

/// 30 %: under a voice without hiding it.
pub const MUSIC_VOLUME: f64 = 0.3;
/// Ducking while someone speaks (`ItemContent::Audio::duck`).
pub const MUSIC_DUCK_DB: f64 = -12.0;
pub const MUSIC_FADE_IN_US: Us = 500_000;
/// Ends at the video's end.
pub const MUSIC_FADE_OUT_US: Us = 2_000_000;

/// How music is added. [`Default`] is a background bed: 30 %, ducking 12 dB, from the start to the video's end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicOpts {
    /// 0 to 2 (`model.layer.volume`).
    pub volume: f64,
    /// Ducking under speech in dB, -30 to 0; None turns it off (the layer gets 0 dB, so it is still music).
    pub duck_db: Option<f64>,
    pub start_us: Us,
    /// None: the video's end (the song's own end when the video is empty). A later end is cut at the video's.
    pub end_us: Option<Us>,
}

impl Default for MusicOpts {
    fn default() -> Self {
        Self {
            volume: MUSIC_VOLUME,
            duck_db: Some(MUSIC_DUCK_DB),
            start_us: 0,
            end_us: None,
        }
    }
}

/// Whether `item` is music: a sound layer with ducking set, 0 dB included (see the module's notes).
pub fn is_music(item: &Item) -> bool {
    matches!(item.content, ItemContent::Audio { duck: Some(_), .. })
}

/// The ops that add `asset` as music: its pieces from `opts.start_us` to the end, free, on the lowest empty audio track
/// that is unlocked and unmuted, else on a new one. The pieces name that track rather than letting `TrackTarget::Auto`
/// pick one each, which could part them where a track has room for some and not others. The file comes in with them
/// when the project doesn't have it yet (the library's case). Refused when the file has no sound, or there's no room
/// (the start at or past the end).
pub fn add_music_ops(p: &Project, asset: &Asset, opts: MusicOpts) -> Result<Vec<Op>> {
    if !asset.has_audio || asset.duration_us < MIN_DURATION_US {
        return Err(invalid(code!("model.music.noSound")));
    }
    let start = opts.start_us;
    let end = match (opts.end_us, video_duration(&p.timeline)) {
        (None, 0) => start.saturating_add(asset.duration_us),
        (None, video) => video,
        (Some(end), 0) => end,
        (Some(end), video) => end.min(video),
    };
    if end.saturating_sub(start) < MIN_DURATION_US {
        return Err(invalid(code!("model.music.noRoom")));
    }
    let mut ops = Vec::new();
    if p.asset(&asset.id).is_none() {
        ops.push(Op::AddAssets {
            assets: vec![asset.clone()],
        });
    }
    let base = new_id();
    let track = match empty_audio_track(p) {
        Some(track) => track.id.clone(),
        None => {
            let id = format!("{base}-track");
            ops.push(Op::AddTrack {
                id: Some(id.clone()),
                kind: TrackKind::Audio,
                index: None,
            });
            id
        }
    };
    let pieces = pieces(start..end, asset.duration_us, 0);
    let last = pieces.len().saturating_sub(1);
    let content = ItemContent::Audio {
        asset_id: asset.id.clone(),
        source_in_us: 0,
        volume: opts.volume,
        muted: false,
        fade_in_us: 0,
        fade_out_us: 0,
        effect: None,
        drift_ppm: 0.0,
        centre_from: None,
        tie: None,
        duck: Some(opts.duck_db.unwrap_or(0.0)),
    };
    let template = Item {
        id: base.clone(),
        start_us: start,
        duration_us: 0,
        attach: None,
        // Sound has no picture: the whole canvas, as every sound layer's box.
        layout: Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        ),
        content,
        effects: Vec::new(),
    };
    for (n, piece) in pieces.into_iter().enumerate() {
        let id = if n == 0 {
            base.clone()
        } else {
            format!("{base}-{}", n + 1)
        };
        let fade_in = if n == 0 { MUSIC_FADE_IN_US } else { 0 };
        let fade_out = if n == last { MUSIC_FADE_OUT_US } else { 0 };
        ops.push(Op::AddItem {
            track: TrackTarget::Track { id: track.clone() },
            item: piece.item(&template, id, fade_in, fade_out),
            attach: false,
        });
    }
    Ok(ops)
}

/// The lowest audio track with nothing on it that isn't locked or muted (a muted one would silence the music).
fn empty_audio_track(p: &Project) -> Option<&Track> {
    p.timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio && t.items.is_empty() && !t.locked && !t.muted)
}

/// One piece of music: where it starts on the timeline, how long it plays and from where in the file.
struct Piece {
    start: Us,
    len: Us,
    source_in: Us,
}

impl Piece {
    /// `template` as this piece, `id`, with these fades (each kept within half the piece).
    fn item(&self, template: &Item, id: Id, fade_in: Us, fade_out: Us) -> Item {
        let mut item = template.clone();
        item.id = id;
        item.start_us = self.start;
        item.duration_us = self.len;
        item.attach = None;
        set_sound(
            &mut item.content,
            Some(self.source_in),
            fade_in,
            fade_out,
            self.len,
        );
        item
    }
}

/// The pieces that play a file `file_len` long over `span`, the first from `source_in` in the file and every one after
/// it from the file's start, back to back; the last is cut at the span's end. A remainder under 10 ms (the shortest
/// layer) is left silent.
fn pieces(span: Range<Us>, file_len: Us, source_in: Us) -> Vec<Piece> {
    let mut out = Vec::new();
    let (mut at, mut from) = (span.start, source_in);
    while span.end.saturating_sub(at) >= MIN_DURATION_US {
        let room = file_len.saturating_sub(from);
        if room < MIN_DURATION_US {
            if from == 0 {
                break;
            }
            from = 0;
            continue;
        }
        let len = room.min(span.end.saturating_sub(at));
        out.push(Piece {
            start: at,
            len,
            source_in: from,
        });
        at = at.saturating_add(len);
        from = 0;
    }
    out
}

/// A sound's file start (when given) and its fades, each within half of `len`, so the two never overlap
/// (`model.audio.fades`).
fn set_sound(content: &mut ItemContent, from: Option<Us>, fade_in: Us, fade_out: Us, len: Us) {
    if let ItemContent::Audio {
        source_in_us,
        fade_in_us,
        fade_out_us,
        ..
    } = content
    {
        if let Some(from) = from {
            *source_in_us = from;
        }
        (*fade_in_us, *fade_out_us) = fit(fade_in, fade_out, len);
    }
}

fn fit(fade_in: Us, fade_out: Us, len: Us) -> (Us, Us) {
    let half = len.max(0) / 2;
    (fade_in.clamp(0, half), fade_out.clamp(0, half))
}
