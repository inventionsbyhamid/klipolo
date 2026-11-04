//! A short's hook holds the captions in 9:16: the hook's words sit in `HOOK_BAND`, just above the captions, and the two
//! stacked crowd the opening frame. So while words placed in that band show, that version draws no captions; they start
//! as the words end. Where the clip is shown whole (Fit, as for a presenter's corner inset), the hook moves up into the
//! band above the picture (`HOOK_ABOVE`), and holds the captions there too. The other versions, and every caption
//! file, keep every word. Nothing is stored for it: move or delete the words and the captions come back.

use crate::framing::effective_framing;
use crate::layout::BoxN;
use crate::media::text_motion;
use crate::project::{FormatKind, Project};
use crate::query::clip_at;
use crate::time::Us;
use crate::timeline::{FramingMode, Item, ItemContent};

/// Where a short's hook sits in 9:16: under the face (which a Fill crop of a talk shows from about 0.08 to 0.46), just
/// above word pop's captions (centred at 0.69), inside the apps' clear area. Two lines of the hook's 130 pixels fit it.
pub const HOOK_BAND: BoxN = BoxN {
    x: 0.06,
    y: 0.465,
    w: 0.81,
    h: 0.155,
};

/// Where a short's hook goes in 9:16 over a 16:9 picture shown whole (Fit): in the blurred band above the picture
/// (which spans 0.342 to 0.658 of the height), as tall as `HOOK_BAND`, inside the apps' clear area. Far from a
/// presenter's corner inset in the picture, and from the captions below it.
pub const HOOK_ABOVE: BoxN = BoxN {
    x: 0.06,
    y: 0.165,
    w: 0.81,
    h: 0.155,
};

/// Whether `item` is words placed in `HOOK_BAND`, or `HOOK_ABOVE`, in 9:16 (its own box where 9:16 is `p`'s main
/// format, else its 9:16 place).
pub fn in_hook_band(p: &Project, item: &Item) -> bool {
    let words = match &item.content {
        ItemContent::Text { .. } => true,
        ItemContent::Generator { source, .. } => source.as_str() == text_motion::SOURCE,
        _ => false,
    };
    let rect = if p.settings.primary_format == FormatKind::Portrait {
        Some(item.layout.rect)
    } else {
        item.layout.overrides.get(&FormatKind::Portrait).copied()
    };
    let near = |a: f64, b: f64| (a - b).abs() < 1e-3;
    let is = |r: BoxN, band: BoxN| {
        near(r.x, band.x) && near(r.y, band.y) && near(r.w, band.w) && near(r.h, band.h)
    };
    words && rect.is_some_and(|r| is(r, HOOK_BAND) || is(r, HOOK_ABOVE))
}

/// Where a short's hook goes in 9:16 at timeline `at_us` of `p`: `HOOK_ABOVE` where the clip there is shown whole (Fit)
/// in 9:16, else `HOOK_BAND`.
pub fn hook_place(p: &Project, at_us: Us) -> BoxN {
    let fit = clip_at(&p.timeline.main, at_us).is_some_and(|(i, _)| {
        effective_framing(p, &p.timeline.main[i], FormatKind::Portrait).mode == FramingMode::Fit
    });
    if fit {
        HOOK_ABOVE
    } else {
        HOOK_BAND
    }
}

/// Whether the 9:16 version's captions wait at timeline `at_us` of `p`: words in the hook's place (`in_hook_band`) show
/// then, whatever the clip's framing.
pub fn hook_holds_captions(p: &Project, at_us: Us) -> bool {
    (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .any(|item| item.start_us <= at_us && at_us < item.end_us() && in_hook_band(p, item))
}
