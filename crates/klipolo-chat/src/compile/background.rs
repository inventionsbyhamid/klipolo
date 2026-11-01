//! `setBackground`: one `SetBackground` with the project's background as the working copy has it, changed by what the
//! call gives; a value left null stays. A gradient given one colour fades it into itself until a second comes. A
//! background that comes out as it was adds no op. Its label always names a `kind`: the tool's word, or `same` when the
//! call left it null (only the colours or the inset changed). Whether a colour is one the project takes is the model's
//! rule: a call it refuses goes back to the model with the reason.
//!
//! A background only shows where a clip leaves the frame uncovered. So a call that names one that can be seen (blurred
//! video, a colour, a gradient or a picture; not black) sets every main clip to Fit in each of the video's formats
//! where it fills the frame and Fill crops it (`framing::fill_crops`), as the clip card's Fit does (`clips::framed`),
//! in the same undo step; with padding there is no need, as the padding shows the background round every clip. The
//! reply says, per format (the main one first, the others named: "In the vertical version"), how many clips were set
//! to Fit, and how many of them stopped following the cursor.

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::fill_focus::FillFocus;
use klipolo_model::framing::{effective_framing, fill_crops};
use klipolo_model::ops::{ClipPatch, Op};
use klipolo_model::project::{BackgroundKind, FormatKind};
use klipolo_model::timeline::FramingMode;

use super::clips::framed;
use super::{Edit, label};
use crate::tools::BackgroundChoice;

/// The edit for `setBackground` on `work`, with the reply's note on the clips it set to Fit (None when it set none).
pub(super) fn set_background(
    work: &Project,
    kind: Option<BackgroundChoice>,
    (color, color_end): (Option<&str>, Option<&str>),
    padding: Option<f64>,
) -> (Edit, Option<String>) {
    let mut background = work.background.clone();
    if let Some(kind) = kind {
        background.kind = kind.kind();
    }
    if let Some(color) = color {
        background.color = color.to_owned();
    }
    if let Some(end) = color_end {
        background.color_end = Some(end.to_owned());
    }
    if background.kind == BackgroundKind::Gradient && background.color_end.is_none() {
        background.color_end = Some(background.color.clone());
    }
    if let Some(padding) = padding {
        background.padding = padding;
    }
    let mut params = vec![("kind", ErrorParam::from(kind.map_or("same", |k| k.key())))];
    if let Some(padding) = padding {
        params.push(("padding", ErrorParam::Number(padding)));
    }
    let shows =
        kind.is_some() && background.kind != BackgroundKind::None && background.padding <= 0.0;
    let mut ops = Vec::new();
    if background != work.background {
        ops.push(Op::SetBackground { background });
    }
    let primary = work.settings.primary_format;
    // The main format first, then the others in the video's order.
    let formats: Vec<FormatKind> = std::iter::once(primary)
        .chain(
            work.settings
                .formats
                .iter()
                .copied()
                .filter(|&f| f != primary),
        )
        .collect();
    let mut fitted: Vec<Fitted> = formats
        .iter()
        .map(|&format| Fitted::of(format, primary))
        .collect();
    if shows {
        for clip in &work.timeline.main {
            let mut fit = Vec::new();
            for (k, &format) in formats.iter().enumerate() {
                let now = effective_framing(work, clip, format);
                if now.mode != FramingMode::Fill || !fill_crops(work, clip, format) {
                    continue;
                }
                fit.push(format);
                fitted[k].clips += 1;
                fitted[k].followed += usize::from(now.follow.is_some());
                fitted[k].speaking += usize::from(matches!(
                    clip.fill_focus.get(&format),
                    Some(FillFocus::Speaker { .. })
                ));
            }
            // One change per clip, its formats together.
            let Some(framing) = framed(work, clip, FramingMode::Fit, &fit) else {
                continue;
            };
            ops.push(Op::UpdateClip {
                id: clip.id.clone(),
                patch: ClipPatch {
                    framing: Some(framing),
                    ..ClipPatch::default()
                },
            });
        }
    }
    let edit = Edit {
        ops,
        label: label("setBackground", params),
    };
    let said: Vec<String> = fitted.into_iter().filter_map(Fitted::said).collect();
    (edit, (!said.is_empty()).then(|| said.join(" ")))
}

/// The clips a background set to Fit in one format, and how many of them followed the cursor, or the speaker, there
/// before; `version` names the format when it isn't the main one ("vertical").
#[derive(Clone, Copy, Debug)]
struct Fitted {
    version: Option<&'static str>,
    clips: usize,
    followed: usize,
    speaking: usize,
}

impl Fitted {
    fn of(format: FormatKind, primary: FormatKind) -> Self {
        let version = (format != primary).then_some(match format {
            FormatKind::Landscape => "landscape",
            FormatKind::Portrait => "vertical",
            FormatKind::Square => "square",
        });
        Self {
            version,
            clips: 0,
            followed: 0,
            speaking: 0,
        }
    }

    /// "A clip is set to Fit so the background shows round it.", "In the vertical version, 3 clips are set to Fit so
    /// the background shows round them. One of them no longer follows the cursor." A clip that followed the speaker is
    /// named the same way ("It no longer follows the speaker.").
    fn said(self) -> Option<String> {
        let set = match self.clips {
            0 => return None,
            1 => "a clip is set to Fit so the background shows round it.".to_owned(),
            n => format!("{n} clips are set to Fit so the background shows round them."),
        };
        let set = match self.version {
            Some(version) => format!("In the {version} version, {set}"),
            None => capitalised(&set),
        };
        let mut said = set;
        for (k, what) in [(self.followed, "cursor"), (self.speaking, "speaker")] {
            let lost = match (self.clips, k) {
                (_, 0) => continue,
                (1, _) => format!("It no longer follows the {what}."),
                (n, k) if k == n => format!("They no longer follow the {what}."),
                (_, 1) => format!("One of them no longer follows the {what}."),
                (_, k) => format!("{k} of them no longer follow the {what}."),
            };
            said.push(' ');
            said.push_str(&lost);
        }
        Some(said)
    }
}

/// `text` with its first letter a capital.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}
