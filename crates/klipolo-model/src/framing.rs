//! A main clip's framing when it has none of its own for a format. Here once, so the
//! engine, chat's tools and the UI (through the app) read the same answer.

use crate::cursor_style::Follow;
use crate::project::{AssetKind, FormatKind, Project, SourceRole};
use crate::recording::RecordingSource;
use crate::timeline::{CameraMode, Clip, ClipSource, Framing, FramingMode};

/// How far the footage's shape may differ from its area's (a share of the area's width ÷ height) and Fill still count as
/// cropping nothing: rounding in a file's size, not a crop anyone sees.
pub const SHAPE_TOLERANCE: f64 = 0.01;

/// Landscape → Fit for recordings with a screen; Portrait and Square → Fill + follow (closer 1) when the
/// clip's screen recording has a cursor, else Fit; camera-only recordings and imported videos → Fill from the centre.
/// Game takes never follow: games keep no separate cursor.
pub fn default_framing(p: &Project, clip: &Clip, format: FormatKind) -> Framing {
    // A recording whose main footage is a screen or a game (not a camera alone).
    let screen = match &clip.source {
        ClipSource::Recording { id } => p.recording(id).is_some_and(|r| r.screen().is_some()),
        ClipSource::Asset { .. } => false,
    };
    let (mode, follow) = match screen {
        false => (FramingMode::Fill, None),
        true if format == FormatKind::Landscape => (FramingMode::Fit, None),
        true if can_follow(p, clip) => (FramingMode::Fill, Some(Follow { closer: 1.0 })),
        true => (FramingMode::Fit, None),
    };
    Framing {
        mode,
        focus_x: 0.5,
        focus_y: 0.5,
        follow,
    }
}

/// The clip's own framing for `format`, else [`default_framing`].
pub fn effective_framing(p: &Project, clip: &Clip, format: FormatKind) -> Framing {
    clip.framing
        .get(&format)
        .copied()
        .unwrap_or_else(|| default_framing(p, clip, format))
}

/// Whether `clip` can follow the cursor in a vertical or square version: it plays a screen take (its main
/// source a screen, not a game) that kept a cursor track. The one rule for the default framing, chat's
/// `followCursor` and the UI's switch (through the app's `clip_framing`).
pub fn can_follow(p: &Project, clip: &Clip) -> bool {
    let ClipSource::Recording { id } = &clip.source else {
        return false;
    };
    p.recording(id).is_some_and(|r| {
        r.cursor.is_some() && r.screen().is_some_and(|s| s.role == SourceRole::Screen)
    })
}

/// Whether Fill crops `clip`'s footage in `format`: what the clip's crop leaves of it has another shape than the area it
/// fills (the frame less the background's padding, or a split's top pane while the recording's camera is split below
/// it), by more than `SHAPE_TOLERANCE`. Footage of no known size crops nothing. The engine frames by the same areas.
pub fn fill_crops(p: &Project, clip: &Clip, format: FormatKind) -> bool {
    let video = |source: Option<&RecordingSource>| {
        let run = source?.runs.first()?;
        p.asset(&run.asset_id)
            .filter(|a| a.kind == AssetKind::Video)
    };
    let (footage, split) = match &clip.source {
        ClipSource::Asset { id } => (p.asset(id).filter(|a| a.kind == AssetKind::Video), false),
        ClipSource::Recording { id } => {
            let Some(rec) = p.recording(id) else {
                return false;
            };
            match video(rec.screen()) {
                Some(screen) => {
                    let split = video(rec.camera()).is_some()
                        && (clip.camera.as_ref())
                            .is_some_and(|c| c.mode_for(format) == CameraMode::SplitBottom);
                    (Some(screen), split)
                }
                // A camera-only take: the camera is the footage, in the whole area.
                None => (video(rec.primary_source()), false),
            }
        }
    };
    let Some(footage) = footage.filter(|a| a.width > 0 && a.height > 0) else {
        return false;
    };
    let (w, h) = format.size();
    let (w, h) = (f64::from(w), f64::from(h));
    let inset = p.background.padding.clamp(0.0, 0.4) * w.min(h);
    let (aw, ah) = (w - 2.0 * inset, h - 2.0 * inset);
    let ah = if split { ah / 2.0 } else { ah };
    let area = aw / ah;
    let shape = clip
        .crop
        .unwrap_or_default()
        .aspect_of(f64::from(footage.width) / f64::from(footage.height));
    (shape / area - 1.0).abs() > SHAPE_TOLERANCE
}
