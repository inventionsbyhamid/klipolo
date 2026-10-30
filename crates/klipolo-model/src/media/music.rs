//! Background music that fits the video. A song shorter than the video plays again and again, in pieces back to back
//! (the last cut at the video's end); a longer one is cut there. It fades in at its start and out at its end, sits on
//! an audio track of its own, stays free (cuts never chop it) and ducks under speech. The engine has no looping: the
//! pieces are plain sound layers of the same file, and each seam gets the mixer's 3 ms edge ramps.
//!
//! **What counts as music** ([`is_music`]): a sound layer with `duck` set. Music added here always sets it, to 0 dB
//! when ducking is off (`ItemContent::Audio::duck` at 0 lowers nothing), so turning ducking off keeps a song music;
//! `None` is a sound effect. The timeline's Music and Sounds lanes split on it, and the refit acts on it.
//!
//! **Refit** ([`refit_music_ops`]): free music doesn't move when the main track does (`ripple_items`), so after an edit
//! that moves the video's end, each run of music pieces that reached the old end is fitted to the new one: cut shorter
//! (pieces past the end go) or played on (the last piece to the file's end, then new pieces), with its fade-out moved
//! to the new end. [`with_music_refit`] folds that into the edit's own batch, so one undo takes back both.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::apply::{apply_ops, id_taken};
use crate::error::{invalid, ModelError, Result};
use crate::ids::{new_id, Id};
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::code;
use crate::ops::{ItemPatch, Op, TrackTarget};
use crate::project::{Asset, Project};
use crate::query::video_duration;
use crate::time::{Us, MIN_DURATION_US, OVERLAP_TOLERANCE_US};
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

/// The ops that fit the music of `after` to its video's end, when an edit from `before` moved it: every run of free
/// music pieces (one file, back to back, on an unlocked track) that reached `before`'s end is cut at the new end or
/// played on to it, its fade-out ending there; a run that starts at or past the new end goes. Music that stopped
/// earlier, or is attached, or on a locked track, is left as it is. Empty when the end didn't move, and when the video
/// is now empty (every clip deleted): the music waits as it is for the footage that comes next, and fits that.
pub fn refit_music_ops(before: &Project, after: &Project) -> Vec<Op> {
    let (old_end, new_end) = (
        video_duration(&before.timeline),
        video_duration(&after.timeline),
    );
    if old_end == new_end || new_end == 0 {
        return Vec::new();
    }
    let mut ops = Vec::new();
    let mut ids = NewIds {
        p: after,
        used: HashSet::new(),
        from: HashMap::new(),
    };
    for track in after
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && !t.locked)
    {
        for run in runs(track) {
            let bed = &track.items[run.clone()];
            let (first, last) = (&bed[0], &bed[bed.len() - 1]);
            let reached = first.start_us <= old_end
                && last.end_us().saturating_add(MIN_DURATION_US) >= old_end;
            if !reached {
                continue;
            }
            // Up to the new end, or the next layer on the track when one stands before it.
            let end = track
                .items
                .get(run.end)
                .map_or(new_end, |next| next.start_us.min(new_end));
            if let Some(file) = played(last).and_then(|(id, _)| after.asset(id)) {
                refit(bed, &track.id, file.duration_us, end, &mut ids, &mut ops);
            }
        }
    }
    ops
}

/// `ops` with the music's refit after them ([`refit_music_ops`] against their result), as one batch for one undo step.
/// An edit that is refused is refused as before. A refit the batch would refuse (a bug) is left out so the edit still
/// goes through, and isn't reported here: the app's history uses [`apply_with_music_refit`], which says so.
pub fn with_music_refit(before: &Project, ops: Vec<Op>) -> Result<Vec<Op>> {
    apply_with_music_refit(before, ops).map(|done| done.ops)
}

/// What [`apply_with_music_refit`] made.
#[derive(Clone, Debug, PartialEq)]
pub struct Refitted {
    /// The edit's ops, then the music's refit when it needs one.
    pub ops: Vec<Op>,
    /// What `apply_ops` makes of `ops` from the project before, for `Document::commit_tx`.
    pub project: Project,
    /// Why the refit was left out: the batch refused it, which is a bug to log (and a panic in debug builds). None
    /// when it went in or nothing needed one.
    pub refused: Option<ModelError>,
}

/// [`with_music_refit`], with the project its ops make from `before` (so a caller can record it with
/// `Document::commit_tx` instead of applying the ops again) and a refused refit told, not hidden. Applies once when
/// nothing needs a refit, twice when something does.
pub fn apply_with_music_refit(before: &Project, ops: Vec<Op>) -> Result<Refitted> {
    let after = apply_ops(before, &ops)?;
    let refit = refit_music_ops(before, &after);
    let done = fold(before, ops, after, refit);
    debug_assert!(
        done.refused.is_none(),
        "the music's refit was refused: {:?}",
        done.refused
    );
    Ok(done)
}

/// `refit` after `ops` (which made `after` from `before`) as one batch, or `ops` alone, with why, when the batch is
/// refused.
fn fold(before: &Project, ops: Vec<Op>, after: Project, refit: Vec<Op>) -> Refitted {
    if refit.is_empty() {
        return Refitted {
            ops,
            project: after,
            refused: None,
        };
    }
    let own = ops.len();
    let mut all = ops;
    all.extend(refit);
    match apply_ops(before, &all) {
        Ok(project) => Refitted {
            ops: all,
            project,
            refused: None,
        },
        Err(why) => {
            all.truncate(own);
            Refitted {
                ops: all,
                project: after,
                refused: Some(why),
            }
        }
    }
}

/// The lowest audio track with nothing on it that isn't locked or muted (a muted one would silence the music).
fn empty_audio_track(p: &Project) -> Option<&Track> {
    p.timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio && t.items.is_empty() && !t.locked && !t.muted)
}

/// One piece of music: where it starts on the timeline, how long it plays and from where in the file, and, in a refit,
/// which of the run's layers it is (None: a new one).
struct Piece {
    start: Us,
    len: Us,
    source_in: Us,
    from: Option<usize>,
}

impl Piece {
    /// `template` (a free music layer) as this piece, `id`, with these fades (each kept within half the piece).
    fn item(&self, template: &Item, id: Id, fade_in: Us, fade_out: Us) -> Item {
        let mut item = template.clone();
        item.id = id;
        item.start_us = self.start;
        item.duration_us = self.len;
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
            from: None,
        });
        at = at.saturating_add(len);
        from = 0;
    }
    out
}

/// The runs of music pieces on `track` a refit treats as one song: free music of one file without clock drift, each
/// piece starting where the one before ends. Indexes into `track.items`, in order.
fn runs(track: &Track) -> Vec<Range<usize>> {
    let piece = |item: &Item| {
        is_music(item)
            && item.attach.is_none()
            && matches!(item.content, ItemContent::Audio { drift_ppm, .. } if drift_ppm == 0.0)
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    for (i, item) in track.items.iter().enumerate() {
        if !piece(item) {
            continue;
        }
        let follows = out.last().is_some_and(|run| {
            let before = &track.items[run.end - 1];
            run.end == i
                && played(before).map(|(id, _)| id) == played(item).map(|(id, _)| id)
                && item.start_us.abs_diff(before.end_us()) <= OVERLAP_TOLERANCE_US.unsigned_abs()
        });
        match out.last_mut() {
            Some(run) if follows => run.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out
}

/// Fits one run of music, `bed` on track `track`, of a file `file_len` long, to end at `end` ([`plan`]): the run's
/// pieces past the plan go, those it keeps change only where their length or fades do, and new ones follow on the
/// track. The fade-in stays on the first piece and the fade-out moves to the last.
fn refit(bed: &[Item], track: &Id, file_len: Us, end: Us, ids: &mut NewIds, ops: &mut Vec<Op>) {
    let (first, last) = (&bed[0], &bed[bed.len() - 1]);
    let want_in = meant(fades(first).0, first.duration_us, MUSIC_FADE_IN_US);
    let want_out = meant(fades(last).1, last.duration_us, MUSIC_FADE_OUT_US);
    let plan = plan(bed, file_len, end);
    let kept = plan.iter().filter(|piece| piece.from.is_some()).count();
    ops.extend(
        bed[kept..]
            .iter()
            .map(|i| Op::RemoveItem { id: i.id.clone() }),
    );
    let final_piece = plan.len().saturating_sub(1);
    for (k, piece) in plan.iter().enumerate() {
        let own = piece.from.map_or((0, 0), |i| fades(&bed[i]));
        let fade_in = if k == 0 { want_in } else { own.0 };
        // The old last piece's fade-out goes with it to the new last one; others keep theirs (none, as added).
        let fade_out = if k == final_piece {
            want_out
        } else if piece.from == Some(bed.len() - 1) {
            0
        } else {
            own.1
        };
        match piece.from {
            Some(i) => {
                let layer = &bed[i];
                if piece.len != layer.duration_us
                    || fades(layer) != fit(fade_in, fade_out, piece.len)
                {
                    update(layer, piece.len, fade_in, fade_out, ops);
                }
            }
            None => ops.push(Op::AddItem {
                track: TrackTarget::Track { id: track.clone() },
                item: piece.item(last, ids.next(&first.id), fade_in, fade_out),
                attach: false,
            }),
        }
    }
}

/// Where a run of music's pieces go to end at `end`: the first ones as they are, then either the one `end` falls in,
/// cut there, or the last played on to its file's end and new pieces from the file's start after it. Empty when the
/// run starts at `end` or past it.
fn plan(bed: &[Item], file_len: Us, end: Us) -> Vec<Piece> {
    let stays = |i: usize, len: Us| Piece {
        start: bed[i].start_us,
        len,
        source_in: source_in(&bed[i]),
        from: Some(i),
    };
    let last = &bed[bed.len() - 1];
    let (k, len) = if end < last.end_us() {
        // Shorter: the pieces from the new end on go, and the one it falls in is cut there.
        let Some(k) = bed
            .iter()
            .rposition(|i| i.start_us.saturating_add(MIN_DURATION_US) <= end)
        else {
            return Vec::new();
        };
        let len = end
            .saturating_sub(bed[k].start_us)
            .min(room(&bed[k], file_len));
        (k, len)
    } else {
        // Longer: the last piece plays on to the file's end.
        let len = end
            .saturating_sub(last.start_us)
            .min(room(last, file_len))
            .max(last.duration_us);
        (bed.len() - 1, len)
    };
    let mut plan: Vec<Piece> = (0..k).map(|i| stays(i, bed[i].duration_us)).collect();
    plan.push(stays(k, len));
    let at = bed[k].start_us.saturating_add(len);
    plan.extend(pieces(at..end, file_len, 0));
    plan
}

/// Sets `piece` to `len` with these fades (kept within half of it).
fn update(piece: &Item, len: Us, fade_in: Us, fade_out: Us, ops: &mut Vec<Op>) {
    let mut content = piece.content.clone();
    set_sound(&mut content, None, fade_in, fade_out, len);
    ops.push(Op::UpdateItem {
        id: piece.id.clone(),
        patch: ItemPatch {
            duration_us: Some(len),
            content: Some(content),
            ..ItemPatch::default()
        },
    });
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

fn fades(item: &Item) -> (Us, Us) {
    match item.content {
        ItemContent::Audio {
            fade_in_us,
            fade_out_us,
            ..
        } => (fade_in_us, fade_out_us),
        _ => (0, 0),
    }
}

fn played(item: &Item) -> Option<(&Id, Us)> {
    item.content.played()
}

fn source_in(item: &Item) -> Us {
    played(item).map_or(0, |(_, from)| from)
}

/// How long `item` can play on from its start: to the end of its file, `file_len` long.
fn room(item: &Item, file_len: Us) -> Us {
    file_len.saturating_sub(source_in(item))
}

/// The fade a piece was meant to have: its own, unless it is as long as half the piece allows, when it was likely cut
/// to fit (a short last piece) and the music's default may come back now that there's room.
fn meant(fade: Us, len: Us, default: Us) -> Us {
    if fade.saturating_mul(2) >= len {
        fade.max(default)
    } else {
        fade
    }
}

/// Ids for new pieces: `{first piece}-n`, the smallest n from 2 not in the project nor given out already. Each base's
/// search goes on from where it stopped, so a refit that adds many pieces stays linear.
struct NewIds<'a> {
    p: &'a Project,
    used: HashSet<Id>,
    from: HashMap<Id, u64>,
}

impl NewIds<'_> {
    fn next(&mut self, base: &str) -> Id {
        let n = self.from.entry(base.to_owned()).or_insert(2);
        loop {
            let id = format!("{base}-{n}");
            *n = n.saturating_add(1);
            if !self.used.contains(&id) && !id_taken(self.p, &id) {
                self.used.insert(id.clone());
                return id;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_refit_is_left_out_and_told() {
        let before = Project::new("Refit");
        let ops = vec![Op::Rename {
            name: "Renamed".into(),
        }];
        let after = apply_ops(&before, &ops).unwrap();
        let bad = vec![Op::RemoveItem { id: "nope".into() }];
        let done = fold(&before, ops.clone(), after.clone(), bad);
        assert_eq!(done.ops, ops, "the edit alone");
        assert_eq!(done.project, after);
        assert_eq!(
            done.refused,
            Some(ModelError::NotFound {
                kind: "layer",
                id: "nope".into()
            })
        );
    }
}
