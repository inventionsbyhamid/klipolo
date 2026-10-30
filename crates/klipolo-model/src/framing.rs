//! A main clip's framing when it has none of its own for a format. Here once, so the
//! engine, chat's tools and the UI (through the app) read the same answer.

use crate::cursor_style::Follow;
use crate::project::{FormatKind, Project, SourceRole};
use crate::timeline::{Clip, ClipSource, Framing, FramingMode};

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
