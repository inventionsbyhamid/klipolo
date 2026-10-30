//! One caption frame: what each word of the page on screen looks like at `t`, from the style's
//! fixed vocabulary: which words show, their colours, scale and offset, the box behind the word being said and its
//! glide, and the block's entrance, tilt, shake and exit. Pure, deterministic (seeded by the page's index, so preview
//! and export agree) and allocation-free: preview and export call it for every frame. The colours are the style's,
//! resolved once (`StyleColors`); the layout and the atlas (`render::captions`) hold everything that doesn't move.

use arrayvec::ArrayVec;
use klipolo_model::captions::style::{BoxOn, CaptionStyleDef, Exit, Per, Reveal, parse_hex};
use klipolo_model::captions::{Keyword, PageWord, Pages};
use klipolo_model::time::Us;

use super::colors::StyleColors;
use super::motion::{enter, mix, move_ms, ms, progress, unit};

/// The most words a page holds (`klipolo_model::captions::pages::MAX_PAGE_WORDS`).
pub const MAX_WORDS: usize = 32;
/// The word being said stays lit this long after it ends, unless the next one starts.
const ACTIVE_HOLD_US: Us = 150_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordDraw {
    pub visible: bool,
    pub fill: [u8; 3],
    /// The fill's own opacity (Karaoke's words still to come).
    pub fill_alpha: f32,
    /// On the style's box (its text colour).
    pub on_box: bool,
    /// Colour, alpha, blur in em.
    pub glow: Option<([u8; 3], f32, f32)>,
    /// Over its laid-out size (an entrance's pop, the word being said).
    pub scale: f32,
    pub dy_em: f32,
    pub opacity: f32,
    /// The person's "Highlight": a box in this colour behind the word.
    pub highlight: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordBoxDraw {
    /// The page word it glides from (the visible word before; the drawer glides only within a line, and grows the box
    /// in on a line's first word), and the one it is on (indices into the page's words).
    pub from: Option<u8>,
    pub to: u8,
    /// 0 → 1 from `from`'s place to `to`'s (only when both are on one line; the drawer checks).
    pub glide: f32,
    /// 0 → 1 horizontal growth (a line's first word).
    pub grow: f32,
    pub color: [u8; 3],
    pub alpha: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaptionFrame {
    /// The block's opacity, scale about its centre, offset in em and turn.
    pub opacity: f32,
    pub scale: f32,
    pub dx_em: f32,
    pub dy_em: f32,
    pub rotation_deg: f32,
    /// One per word of the page, hidden ones included (never visible).
    pub words: ArrayVec<WordDraw, MAX_WORDS>,
    pub word_box: Option<WordBoxDraw>,
    /// The style boxes each line: colour and alpha.
    pub line_boxes: Option<([u8; 3], f32)>,
}

impl CaptionFrame {
    fn blank() -> Self {
        Self {
            opacity: 1.0,
            scale: 1.0,
            dx_em: 0.0,
            dy_em: 0.0,
            rotation_deg: 0.0,
            words: ArrayVec::new(),
            word_box: None,
            line_boxes: None,
        }
    }
}

fn class(k: Keyword) -> usize {
    match k {
        Keyword::Plain => 0,
        Keyword::Gain => 1,
        Keyword::Loss => 2,
    }
}

/// `caption_frame_with` with the style's colours worked out here (tests; the plan keeps them per context).
pub fn caption_frame(style: &CaptionStyleDef, pages: &Pages, index: usize, t: Us) -> CaptionFrame {
    caption_frame_with(style, &StyleColors::of(style), pages, index, t)
}

/// Page `index` of `pages` at timeline time `t` in `style` (its colours `colors`). A page that isn't there draws
/// nothing (no words).
pub fn caption_frame_with(
    style: &CaptionStyleDef,
    colors: &StyleColors,
    pages: &Pages,
    index: usize,
    t: Us,
) -> CaptionFrame {
    let mut f = CaptionFrame::blank();
    let Some(page) = pages.pages.get(index) else {
        return f;
    };
    let lead = ms(style.lead_ms);
    let words = pages
        .words
        .get(page.words.0 as usize..page.words.1 as usize)
        .unwrap_or_default();
    let line_box = style.boxes.as_ref().filter(|b| b.on == BoxOn::Line);
    let enter_ms = move_ms(&style.enter.motion);

    // The block's entrance, tilt, shake and exit.
    if style.enter.per == Per::Page {
        (f.opacity, f.scale, f.dy_em) = enter(
            &style.enter.motion,
            progress(t, page.start_us - lead, enter_ms),
        );
    }
    f.rotation_deg = style.tilt_deg as f32 * unit(page.index);
    if let Some(sh) = &style.shake {
        let from = page.start_us - lead + ms(enter_ms);
        if (!sh.keyword_pages_only || page.keyword) && t >= from && t < from + ms(sh.ms) {
            let q = (t - from) as f32 / (sh.ms as f32 * 1000.0);
            f.dx_em = sh.amp_em as f32 * (q * std::f32::consts::TAU * 2.0).sin();
        }
    }
    if let Exit::Fade {
        ms: out,
        min_gap_ms,
    } = style.exit
    {
        // The next page that shows (one of hidden words only takes no time).
        let next = pages.pages[index + 1..]
            .iter()
            .find(|n| n.end_us > n.start_us)
            .map_or(Us::MAX, |n| n.start_us);
        if out > 0 && next.saturating_sub(page.end_us) >= ms(min_gap_ms) {
            let left = (page.end_us - t) as f64 / (f64::from(out) * 1000.0);
            f.opacity *= left.clamp(0.0, 1.0) as f32;
        }
    }
    if let Some(b) = line_box {
        f.line_boxes = Some((colors.box_color, b.alpha as f32));
    }

    // The word being said: the last that started (less the lead), while it lasts or until the next starts. Hidden
    // words stay in the page but are never shown, lit or counted.
    let started = |w: &PageWord| w.start_us - lead <= t;
    let active = words
        .iter()
        .rposition(|w| !w.hidden && started(w))
        .filter(|&i| {
            t < words[i].end_us + ACTIVE_HOLD_US
                || words[i + 1..]
                    .iter()
                    .find(|n| !n.hidden)
                    .is_some_and(|n| t < n.start_us - lead)
        });
    let reveal_page = style.pages.reveal == Reveal::Page;
    let visible_count = words
        .iter()
        .filter(|w| !w.hidden && (reveal_page || started(w)))
        .count();

    for (k, w) in words.iter().enumerate().take(MAX_WORDS) {
        let i = page.words.0 + k as u32;
        let look = pages.look(w);
        let mut fill = if line_box.is_some() {
            colors.box_text
        } else {
            colors.fill
        };
        let mut glow = None;
        if let (Some(kw), Some(kd)) = (pages.keyword(i), &style.keyword) {
            fill = colors.keyword[class(kw)];
            if let Some(g) = &kd.glow {
                glow = Some((
                    colors.keyword_glow.unwrap_or(fill),
                    g.alpha as f32,
                    g.blur_em as f32,
                ));
            }
        }
        // "Emphasis" in a style without keywords (Boxed, Karaoke, Highlight, Clean) draws in the accent; its ×1.1
        // is in the layout, so the word is drawn sharp and has room.
        if look.emphasis == Some(true) && style.keyword.is_none() {
            fill = colors.accent;
        }
        if let Some(c) = look.color.as_deref() {
            fill = parse_hex(c);
        }
        let mut d = WordDraw {
            visible: !w.hidden && (reveal_page || started(w)),
            fill,
            fill_alpha: 1.0,
            on_box: line_box.is_some(),
            glow,
            scale: 1.0,
            dy_em: 0.0,
            opacity: 1.0,
            highlight: None,
        };
        if look.highlight == Some(true) {
            d.highlight = Some(colors.accent);
            d.fill = colors.accent_text;
        }
        if style.enter.per == Per::Word {
            (d.opacity, d.scale, d.dy_em) = enter(
                &style.enter.motion,
                progress(t, w.start_us - lead, enter_ms),
            );
        }
        let dim = style.active.dim_upcoming.map(|a| a as f32);
        match active {
            Some(a) if a == k => {
                let lit = style
                    .active
                    .color
                    .as_ref()
                    .map_or(1.0, |c| progress(t, w.start_us - lead, c.fade_ms));
                if let Some(c) = colors.active {
                    d.fill = mix(d.fill, c, lit);
                }
                if let Some(s) = style.active.scale.filter(|_| visible_count >= 2) {
                    d.scale *= 1.0 + (s as f32 - 1.0) * lit;
                }
                if let Some(g) = &style.active.glow {
                    d.glow = Some((
                        colors.active_glow.unwrap_or(d.fill),
                        g.alpha as f32 * lit,
                        g.blur_em as f32,
                    ));
                }
            }
            Some(a) if k > a => d.fill_alpha = dim.unwrap_or(1.0),
            None if !started(w) => d.fill_alpha = dim.unwrap_or(1.0),
            _ => {}
        }
        f.words.push(d);
    }

    // The active-word box glides from the word before (the drawer keeps it still across lines).
    if let (Some(b), Some(a)) = (
        style.boxes.as_ref().filter(|b| b.on == BoxOn::ActiveWord),
        active.filter(|&a| a < MAX_WORDS),
    ) {
        let start = words[a].start_us - lead;
        let glide = progress(t, start, b.glide_ms);
        // Named even once the glide is over: the drawer tells a line's first word (grown in) from the others by it.
        let from = words[..a].iter().rposition(|w| !w.hidden).map(|i| i as u8);
        f.word_box = Some(WordBoxDraw {
            from,
            to: a as u8,
            glide,
            grow: progress(t, start, b.grow_ms),
            color: colors.box_color,
            alpha: b.alpha as f32,
        });
        if let Some(w) = f.words.get_mut(a) {
            w.on_box = true;
            w.fill = colors.box_text;
        }
    }
    f
}
