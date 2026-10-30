//! Which cursor takes a project needs: every take with a cursor file that the main track plays, because
//! the cursor layer draws it and Auto zooms and chat read its facts, with the smoothings its clips draw it with, the
//! follow-crop's crops they follow it with, and its screen files (whose activity goes into its facts and follow paths).
//!
//! A clip draws with its own style, else the project's (`Project::cursor_for`), as it takes effect (`effective`: as
//! recorded draws the raw path). Nothing is drawn where the cursor is hidden, nor over a take whose frames already show
//! the cursor (`CursorCapture::Burned`), so those clips need no smoothing; the take is still wanted for its facts. A
//! clip follows in each of the project's formats where its effective framing is Fill with `follow`.
//!
//! A take's screen activity is read only where something plans with it: a clip of it under an Auto zoom
//! (the camera's focus), or following (its follow paths): `Want::plans_read_activity`. Other takes' facts and sizes
//! never depend on how far a screen's analysis has got (`Want::activity_screens`): export, which makes the activity of
//! exactly those screens before its first frame, then holds the same takes as the preview at the budget's edge. Every
//! take still lists its screens (`Want::screens`), for chat's facts.

use std::collections::HashMap;

use klipolo_model::cursor_style::{CursorCapture, Smoothing};
use klipolo_model::project::{Project, Recording};
use klipolo_model::time::Us;
use klipolo_model::timeline::{ClipSource, ItemContent, TrackKind, ZoomTarget};

/// The most follow-crop crops a take holds paths for (a take never wants more than it holds, so it is
/// never built again on every publish). Past it, the first ones in timeline order win; a clip following with another
/// crop shows its Fill crop at its focus, in preview and export alike.
pub const MAX_FOLLOWS: usize = 4;

/// A take the project needs (see the module notes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Want {
    pub path: String,
    /// In their order (Light, Medium, Strong); Off never, as it draws the raw path.
    pub smoothings: Vec<Smoothing>,
    /// The follow-crop's crops its clips draw (`plan::framing::follow_crop_key`: w, h and closer in thousandths), in
    /// first-use order, at most `MAX_FOLLOWS`.
    pub crops: Vec<(u16, u16, u16)>,
    /// The take's screen files, each with where it starts in the take (`SourceRun::offset_us`), in order.
    pub screens: Vec<(Us, String)>,
    /// The take's plans read its screens' activity: a clip of it is under an Auto zoom, or follows (see the module
    /// notes). Only then do its facts and follow paths take it (`activity_screens`).
    pub plans_read_activity: bool,
    /// The screen's picture, width over height (0 when unknown): the follow path measures its speed on it.
    pub aspect: f64,
}

impl Want {
    /// The screens whose activity the take's facts and follow paths take: its own when its plans read them
    /// (`plans_read_activity`), else none, so its facts and size don't depend on them.
    pub fn activity_screens(&self) -> &[(Us, String)] {
        match self.plans_read_activity {
            true => &self.screens,
            false => &[],
        }
    }
}

/// Every take with a cursor file that the main track plays, in timeline order (each once, where it first plays), with
/// the smoothings its clips' styles use, the crops they follow with, and its screens when their activity is read.
/// O(clips × (formats + log Auto zooms) + recordings + runs + zoom items).
pub fn wanted(p: &Project) -> Vec<Want> {
    let cursors: HashMap<&str, _> = p
        .recordings
        .iter()
        .filter_map(|r| Some((r.id.as_str(), (r, r.cursor.as_ref()?))))
        .collect();
    let mut out: Vec<Want> = Vec::new();
    let mut at: HashMap<&str, usize> = HashMap::new();
    let autos = auto_spans(p);
    // Per take: something reads its activity.
    let mut read: Vec<bool> = Vec::new();
    let starts = klipolo_model::query::clip_starts(&p.timeline.main);
    for (clip, &start) in p.timeline.main.iter().zip(&starts) {
        let span = (start, start.saturating_add(clip.duration_us()));
        let ClipSource::Recording { id } = &clip.source else {
            continue;
        };
        let Some(&(rec, cursor)) = cursors.get(id.as_str()) else {
            continue;
        };
        let i = *at.entry(cursor.path.as_str()).or_insert_with(|| {
            out.push(take(p, rec, &cursor.path));
            read.push(false);
            out.len() - 1
        });
        read[i] |= under(&autos, span);
        let want = &mut out[i];
        let style = p.cursor_for(clip).effective();
        let drawn = !style.hidden && cursor.capture != CursorCapture::Burned;
        let smoothing = style.smoothing;
        if drawn && smoothing != Smoothing::Off && !want.smoothings.contains(&smoothing) {
            want.smoothings.push(smoothing);
        }
        for &format in &p.settings.formats {
            if let Some(crop) = crate::plan::follow_crop_for(p, clip, rec, format)
                && !want.crops.contains(&crop)
                && want.crops.len() < MAX_FOLLOWS
            {
                want.crops.push(crop);
            }
        }
    }
    for (want, read) in out.iter_mut().zip(read) {
        want.smoothings.sort_by_key(|&s| s as u8);
        want.plans_read_activity = read || !want.crops.is_empty();
    }
    out
}

/// The spans of `p`'s Auto zooms on its visible zoom tracks, merged where they meet: sorted and apart. O(items log
/// items).
fn auto_spans(p: &Project) -> Vec<(Us, Us)> {
    let mut spans: Vec<(Us, Us)> = (p.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Zoom && !t.hidden)
        .flat_map(|t| &t.items)
        .filter(|i| {
            matches!(
                &i.content,
                ItemContent::Zoom {
                    target: ZoomTarget::Auto { .. },
                    ..
                }
            )
        })
        .map(|i| (i.start_us, i.end_us()))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(Us, Us)> = Vec::with_capacity(spans.len());
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

/// A clip over `[a, b)` meets one of `spans` (`auto_spans`), touching included, as a camera run counts the clips at its
/// ends (`plan::zoom::camera`). O(log spans).
fn under(spans: &[(Us, Us)], (a, b): (Us, Us)) -> bool {
    let i = spans.partition_point(|&(_, end)| end < a);
    spans.get(i).is_some_and(|&(start, _)| start <= b)
}

/// The cursor files of the takes `wanted` lists, in its order (timeline order, each once), without what they draw:
/// what a plan context finds its takes by on every edit. O(clips + recordings).
pub fn take_paths(p: &Project) -> Vec<&str> {
    let cursors: HashMap<&str, &str> = (p.recordings.iter())
        .filter_map(|r| Some((r.id.as_str(), r.cursor_path()?)))
        .collect();
    let mut seen = std::collections::HashSet::new();
    (p.timeline.main.iter())
        .filter_map(|clip| match &clip.source {
            ClipSource::Recording { id } => cursors.get(id.as_str()).copied(),
            ClipSource::Asset { .. } => None,
        })
        .filter(|path| seen.insert(*path))
        .collect()
}

/// `rec`'s take with nothing drawn yet: its screen files (the primary's runs) and their picture's shape.
fn take(p: &Project, rec: &Recording, path: &str) -> Want {
    let runs = rec.screen().map_or(&[][..], |s| &s.runs);
    let screens = (runs.iter())
        .filter_map(|r| Some((r.offset_us, p.asset(&r.asset_id)?.path.clone())))
        .collect();
    let aspect = (runs.first().and_then(|r| p.asset(&r.asset_id)))
        .filter(|a| a.width > 0 && a.height > 0)
        .map_or(0.0, |a| f64::from(a.width) / f64::from(a.height));
    Want {
        path: path.to_owned(),
        screens,
        aspect,
        ..Want::default()
    }
}

/// Some take of `p` has frames without the cursor somewhere (`CursorCapture::Separate` or `BurnedFrom`), so the cursor
/// layer may draw over it: a take landing then changes the frame. O(recordings).
pub fn draws_any(p: &Project) -> bool {
    p.recordings
        .iter()
        .filter_map(|r| r.cursor.as_ref())
        .any(|c| c.capture != CursorCapture::Burned)
}
