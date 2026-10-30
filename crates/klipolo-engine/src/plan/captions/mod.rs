//! The caption layer: the page on screen at this moment, over everything else,
//! drawn by the caption renderer (`render::captions`) from its style's data. A frame finds its page by binary search
//! (from the style's lead before it), works out what each word looks like now (`frame`, allocation-free) and says
//! where the block goes (`place::Anchor`); the resolved style, its colours and font are made once per change of the
//! caption settings (`CaptionLook`, kept by later contexts). A style this build doesn't have draws in Clean, and
//! the frame names its id (`FramePlan::missing`, said as `frame.captionStyleMissing`).

pub mod colors;
pub mod frame;
mod look;
mod motion;
mod moves;
pub mod place;

use std::sync::Arc;

use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::captions::{Captions, Pages, page_at};
use klipolo_model::contrib::ContribKind;
use klipolo_model::font::FontRef;
use klipolo_model::project::FormatKind;
use klipolo_model::time::Us;

pub use colors::StyleColors;
pub use frame::CaptionFrame;
pub use look::CaptionLook;
pub(crate) use moves::may_move_captions;
pub use moves::pages_may_differ;
pub use place::Anchor;

use super::PlanContext;
use super::clip::split_seam;
use super::effects::note_missing;
use super::layers::MissingContrib;

/// One frame's captions: the page on screen, how each of its words looks now, and where the block goes. The drawer
/// lays the page out (once per page: its atlas is kept), resolves the face (the font system and the index are its,
/// and works out the block's centre from the laid-out page's real size.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptionLayer {
    pub captions: Arc<Captions>,
    pub format: FormatKind,
    /// `captions.pages[format].pages[page]`.
    pub page: u32,
    pub style: Arc<CaptionStyleDef>,
    /// `CaptionLook::key`: what the style's word masks are drawn from (the drawer's atlas key).
    pub style_key: u64,
    pub colors: StyleColors,
    /// The font asked for (the person's caption font or the style's family), at the style's weight and style.
    pub font: Arc<FontRef>,
    pub frame: CaptionFrame,
    pub anchor: Anchor,
    /// The widest a line may be, as a share of the frame's width (`place::max_width_share`).
    pub max_width: f64,
    /// The text language its Chinese, Japanese and Korean follow.
    pub language: Arc<str>,
}

impl CaptionLayer {
    /// The format's pages.
    pub fn pages(&self) -> Option<&Pages> {
        self.captions.pages.get(&self.format).map(|p| &**p)
    }

    /// What the page shows (its words that aren't hidden, a `\n` where the person started a line): for tests.
    pub fn shown_text(&self) -> String {
        self.pages()
            .and_then(|p| Some(p.shown_text(p.pages.get(self.page as usize)?)))
            .unwrap_or_default()
    }
}

/// The page on screen at `t`: from its first word's start less the style's lead (a word may be revealed that early,
/// and an entrance starts then), but never before the page before it ends; else `page_at`'s. Binary search;
/// allocates nothing.
pub fn page_at_lead(pages: &Pages, t: Us, lead: Us) -> Option<usize> {
    let next = pages
        .pages
        .partition_point(|p| p.start_us <= t.saturating_add(lead));
    if let Some(i) = next.checked_sub(1) {
        let p = &pages.pages[i];
        let after = i.checked_sub(1).map_or(Us::MIN, |k| pages.pages[k].end_us);
        if p.end_us > p.start_us && t >= (p.start_us - lead).max(after) && t < p.end_us {
            return Some(i);
        }
    }
    page_at(pages, t)
}

/// The captions on screen at `at_us` in `format`, if captions are on and a page is up. A style this build doesn't have
/// is named in `missing` while it shows.
pub fn caption_layer(
    ctx: &PlanContext,
    at_us: Us,
    format: FormatKind,
    missing: &mut Vec<MissingContrib>,
) -> Option<CaptionLayer> {
    let p = &*ctx.project;
    if !p.captions.enabled {
        return None;
    }
    let pages = ctx.captions.pages.get(&format)?;
    let look = &*ctx.caption_style;
    let lead = Us::from(look.style.lead_ms) * 1_000;
    let page = page_at_lead(pages, at_us, lead)?;
    if let Some(id) = &look.missing {
        note_missing(missing, ContribKind::CaptionStyle, id.clone());
    }
    // The seam only for a style placed at it (Arcade): a binary search and the clip's camera layout.
    let seam = (look.style.position.get(format).seam == Some(true))
        .then(|| split_seam(ctx, at_us, format))
        .flatten();
    Some(CaptionLayer {
        captions: ctx.captions.clone(),
        format,
        page: page as u32,
        style: look.style.clone(),
        style_key: look.key,
        colors: look.colors,
        font: look.font.clone(),
        frame: frame::caption_frame_with(&look.style, &look.colors, pages, page, at_us),
        anchor: Anchor::of(p, format, seam),
        max_width: place::max_width_share(p, &look.style, format),
        language: ctx.text_language.clone(),
    })
}
