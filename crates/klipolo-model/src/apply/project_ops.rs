use std::sync::Arc;

use crate::captions::looks::{self, Look};
use crate::contrib::ContribId;
use crate::cursor_style::CursorStyle;
use crate::error::{invalid, not_found, Result};
use crate::layout::{resolve_rect, Layout};
use crate::messages::code;
use crate::ops::{CursorTarget, TrackPatch};
use crate::project::{
    Asset, AudioSettings, Background, CaptionSettings, ColorLut, FormatKind, Project, Recording,
    Settings,
};
use crate::speech::{fix_text_blank, fix_text_ok, without_tags, WordFix};
use crate::time::{RangeUs, Us};
use crate::timeline::{ClipSource, ItemContent, Track, TrackKind};
use crate::validate::MAX_WORD_LOOKS;

use super::places::Places;
use super::{default_track_name, fresh_id, id_taken, LOCKED};

pub(super) fn rename(p: &mut Project, name: &str) -> Result<()> {
    p.name = name.trim().to_string();
    Ok(())
}

/// Replace the settings. When the primary format changes, every layout is re-expressed in the new
/// primary's coordinates and the old primary keeps its exact rect as an override.
/// A layer's overrides for formats that are no longer listed stay in the file, as the camera's modes and a clip's
/// framing do: a format listed again finds its layers where they were placed (a title's 9:16 place, a template that
/// fills the frame). Caption places for formats no longer listed are dropped. The caption style never changes with the
/// formats.
pub(super) fn set_settings(p: &mut Project, settings: &Settings) -> Result<()> {
    let (old, new) = (p.settings.primary_format, settings.primary_format);
    let formats = settings.formats.as_slice();
    for track in &mut p.timeline.tracks {
        for item in &mut track.items {
            rebase_layout(&mut item.layout, old, new);
        }
    }
    p.captions.place.retain(|f, _| formats.contains(f));
    for camera in p.timeline.main.iter_mut().filter_map(|c| c.camera.as_mut()) {
        rebase_layout(&mut camera.bubble, old, new);
    }
    p.settings = settings.clone();
    Ok(())
}

/// Re-expresses `layout` from the `old` primary in the `new` one: the new primary's place becomes the rect and the old
/// rect the old primary's override. Overrides for other formats, listed or not, are left as they are.
fn rebase_layout(layout: &mut Layout, old: FormatKind, new: FormatKind) {
    if old != new {
        let old_rect = layout.rect;
        layout.rect = resolve_rect(layout, old, new);
        layout.overrides.remove(&new);
        layout.overrides.insert(old, old_rect);
    }
}

pub(super) fn set_captions(p: &mut Project, captions: &CaptionSettings) -> Result<()> {
    let mut next = captions.clone();
    // The first enable decides the style once, from the formats then; a later format change never does. One
    // place for the switch, `setCaptions`, MCP and the captions panel. A look picked while captions are off pins
    // the style it was picked for too, so adding 9:16 never leaves it on a style without that look.
    if next.style.is_none() && (next.enabled || next.variant.is_some()) {
        next.style = Some(ContribId::from(crate::captions::styles::default_style(
            &p.settings.formats,
        )));
    }
    p.captions = next;
    Ok(())
}

/// Replaces the fixes of `asset_id` overlapping `[start, end)` with one reading `text`, or removes them (`None`). The
/// list stays sorted by asset and start, so no two fixes of one file overlap. O(fixes).
pub(super) fn fix_words(
    p: &mut Project,
    asset_id: &str,
    start: Us,
    end: Us,
    text: Option<&str>,
) -> Result<()> {
    if p.asset(asset_id).is_none() {
        return Err(not_found("media", asset_id));
    }
    super::check_time(start)?;
    super::check_time(end)?;
    if start >= end {
        return Err(invalid(code!("model.fix.range")));
    }
    // Tags draw nothing (a text of them only is blank), and a correction is stored without them.
    if text.is_some_and(fix_text_blank) {
        return Err(invalid(code!("model.fix.blank")));
    }
    let text = text.map(without_tags);
    let text = text.as_deref();
    if text.is_some_and(|t| !fix_text_ok(t)) {
        return Err(invalid(code!("model.fix.text")));
    }
    // The list is shared with the project the batch started from: this copies it once.
    let fixes = Arc::make_mut(&mut p.word_fixes);
    fixes.retain(|f| f.asset_id != asset_id || f.end_us <= start || f.start_us >= end);
    if let Some(text) = text {
        let fix = WordFix {
            asset_id: asset_id.into(),
            start_us: start,
            end_us: end,
            text: text.into(),
        };
        let at = fixes.partition_point(|f| (f.asset_id.as_str(), f.start_us) < (asset_id, start));
        fixes.insert(at, fix);
    }
    Ok(())
}

/// `look` over each of `ranges` of `asset_id`'s own time, replacing the looks there; an empty look only clears.
/// One pass over the file's looks (`looks::set_word_looks`): O(log looks + window +
/// ranges × log ranges), and one copy of the shared list per batch. At least one range, each valid, and no more
/// ranges than a project holds looks.
pub(super) fn set_word_look(
    p: &mut Project,
    asset_id: &str,
    ranges: &[RangeUs],
    look: &Look,
) -> Result<()> {
    if p.asset(asset_id).is_none() {
        return Err(invalid(code!("model.wordLook.asset")));
    }
    if ranges.is_empty() || !ranges.iter().all(|r| looks::range_ok(r.start, r.end)) {
        return Err(invalid(code!("model.wordLook.range")));
    }
    if ranges.len() > MAX_WORD_LOOKS {
        return Err(invalid(code!("model.wordLook.tooMany")));
    }
    looks::check_look(look)?;
    let ranges = looks::sorted_ranges(ranges);
    looks::set_word_looks(Arc::make_mut(&mut p.word_looks), asset_id, &ranges, look);
    Ok(())
}

pub(super) fn set_background(p: &mut Project, background: &Background) -> Result<()> {
    p.background = background.clone();
    Ok(())
}

pub(super) fn set_audio(p: &mut Project, audio: &AudioSettings) -> Result<()> {
    p.audio = audio.clone();
    Ok(())
}

/// The project's cursor style (it needs one), or each clip's own (`None` clears it, so the project's draws). Every
/// clip is found before any changes, so an unknown id changes nothing. O(ids) through the batch's index.
pub(super) fn set_cursor_style(
    p: &mut Project,
    places: &mut Places,
    target: &CursorTarget,
    style: Option<&CursorStyle>,
) -> Result<()> {
    if let Some(style) = style {
        style.check()?;
    }
    match target {
        CursorTarget::Project => {
            p.cursor = style
                .copied()
                .ok_or_else(|| invalid(code!("model.cursor.projectNeedsStyle")))?;
        }
        CursorTarget::Clips { ids } => {
            let at = ids
                .iter()
                .map(|id| {
                    places
                        .clip(&p.timeline.main, id)
                        .ok_or_else(|| not_found("clip", id))
                })
                .collect::<Result<Vec<_>>>()?;
            for i in at {
                p.timeline.main[i].cursor = style.copied();
            }
        }
    }
    Ok(())
}

pub(super) fn add_assets(p: &mut Project, assets: &[Asset]) -> Result<()> {
    p.assets.extend(assets.iter().cloned());
    Ok(())
}

pub(super) fn add_recording(p: &mut Project, recording: &Recording) -> Result<()> {
    p.recordings.push(recording.clone());
    Ok(())
}

fn used_on_timeline(p: &Project, id: &str) -> bool {
    let in_main = p
        .timeline
        .main
        .iter()
        .any(|c| matches!(&c.source, ClipSource::Asset { id: a } if a == id));
    let in_items =
        p.timeline
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .any(|item| match &item.content {
                ItemContent::Video { asset_id, .. }
                | ItemContent::Image { asset_id }
                | ItemContent::Audio { asset_id, .. } => asset_id == id,
                _ => false,
            });
    in_main || in_items
}

fn used_by_recording(p: &Project, id: &str) -> bool {
    p.recordings.iter().any(|r| r.asset_ids().any(|a| a == id))
}

pub(super) fn remove_asset(p: &mut Project, id: &str) -> Result<()> {
    if p.asset(id).is_none() {
        return Err(not_found("media", id));
    }
    if used_on_timeline(p, id) {
        return Err(invalid(code!("model.media.inUse")));
    }
    if used_by_recording(p, id) {
        return Err(invalid(code!("model.media.inRecording")));
    }
    if p.background.asset_id.as_deref() == Some(id) {
        return Err(invalid(code!("model.media.isBackground")));
    }
    p.assets.retain(|a| a.id != id);
    // Copied only when the file has fixes or looks to drop.
    if p.word_fixes.iter().any(|f| f.asset_id == id) {
        Arc::make_mut(&mut p.word_fixes).retain(|f| f.asset_id != id);
    }
    if p.word_looks.iter().any(|l| l.asset_id == id) {
        Arc::make_mut(&mut p.word_looks).retain(|l| l.asset_id != id);
    }
    Ok(())
}

pub(super) fn add_lut(p: &mut Project, lut: &ColorLut) -> Result<()> {
    p.luts.push(lut.clone());
    Ok(())
}

pub(super) fn remove_lut(p: &mut Project, id: &str) -> Result<()> {
    let before = p.luts.len();
    p.luts.retain(|l| l.id != id);
    if p.luts.len() == before {
        return Err(not_found("lut", id));
    }
    for clip in &mut p.timeline.main {
        if clip.look.lut_id.as_deref() == Some(id) {
            clip.look.lut_id = None;
        }
    }
    Ok(())
}

fn track_index(p: &Project, id: &str) -> Result<usize> {
    p.timeline
        .tracks
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| not_found("track", id))
}

/// Insert a new empty track at `index` (clamped to the end; `None` appends). Without an `id`, the
/// track gets `{kind}-track` (numbered if taken), so repeating the op repeats the id.
pub(super) fn add_track(
    p: &mut Project,
    id: Option<&str>,
    kind: TrackKind,
    index: Option<usize>,
) -> Result<()> {
    let mut track = Track::new(kind, default_track_name(kind, &p.timeline.tracks));
    track.id = match id {
        Some(id) => id.to_string(),
        None => {
            let kind_name = match kind {
                TrackKind::Overlay => "overlay",
                TrackKind::Audio => "audio",
                TrackKind::Zoom => "zoom",
            };
            fresh_id(format!("{kind_name}-track"), |id| id_taken(p, id))
        }
    };
    let len = p.timeline.tracks.len();
    let at = index.unwrap_or(len).min(len);
    p.timeline.tracks.insert(at, track);
    Ok(())
}

pub(super) fn remove_track(p: &mut Project, id: &str) -> Result<()> {
    let i = track_index(p, id)?;
    if p.timeline.tracks[i].locked {
        return Err(invalid(LOCKED));
    }
    p.timeline.tracks.remove(i);
    Ok(())
}

/// Move a track to `index` (clamped to the top).
pub(super) fn move_track(p: &mut Project, id: &str, index: usize) -> Result<()> {
    let i = track_index(p, id)?;
    let track = p.timeline.tracks.remove(i);
    let at = index.min(p.timeline.tracks.len());
    p.timeline.tracks.insert(at, track);
    Ok(())
}

pub(super) fn update_track(p: &mut Project, id: &str, patch: &TrackPatch) -> Result<()> {
    let i = track_index(p, id)?;
    let track = &mut p.timeline.tracks[i];
    if let Some(name) = &patch.name {
        track.name = name.trim().to_string();
    }
    if let Some(v) = patch.hidden {
        track.hidden = v;
    }
    if let Some(v) = patch.muted {
        track.muted = v;
    }
    if let Some(v) = patch.locked {
        track.locked = v;
    }
    if let Some(v) = patch.solo {
        track.solo = v;
    }
    Ok(())
}
