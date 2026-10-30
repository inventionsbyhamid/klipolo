//! A caption style's colours, resolved once per change of the caption settings (`CaptionLook`) so a frame reads
//! numbers, never paints: `#RRGGBB`, `accent` and `contrast` are worked out here, and nothing allocates.

use klipolo_model::captions::style::{BoxOn, CaptionStyleDef, contrast, parse_hex};

/// Every colour a caption frame or its drawer uses, as RGB bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleColors {
    /// The words' fill.
    pub fill: [u8; 3],
    /// A word on the style's box (its `text`, else black or white against the box).
    pub box_text: [u8; 3],
    /// A keyword's fill per class (`Keyword::Plain`, `Gain`, `Loss`).
    pub keyword: [[u8; 3]; 3],
    /// A keyword's glow; None: the keyword's own fill.
    pub keyword_glow: Option<[u8; 3]>,
    /// The word being said: its colour, and its glow's (None: the word's own fill).
    pub active: Option<[u8; 3]>,
    pub active_glow: Option<[u8; 3]>,
    /// The style's box (line or active word).
    pub box_color: [u8; 3],
    /// The accent (the person's colour, or the style's), and black or white on it (a word the person highlighted).
    pub accent: [u8; 3],
    pub accent_text: [u8; 3],
    pub outline: [u8; 3],
    pub shadow: [u8; 3],
}

impl StyleColors {
    pub fn of(style: &CaptionStyleDef) -> Self {
        let accent = parse_hex(&style.accent);
        let box_color = style
            .boxes
            .as_ref()
            .map_or([0, 0, 0], |b| style.rgb(&b.color, false));
        let box_text = style
            .boxes
            .as_ref()
            .and_then(|b| b.text.as_ref())
            .map_or(contrast(box_color), |p| style.rgb(p, true));
        let on_line = style.boxes.as_ref().is_some_and(|b| b.on == BoxOn::Line);
        let keyword = style.keyword.as_ref().map_or([[255; 3]; 3], |k| {
            [
                style.rgb(&k.color, on_line),
                parse_hex(&k.gain),
                parse_hex(&k.loss),
            ]
        });
        let keyword_glow = style
            .keyword
            .as_ref()
            .and_then(|k| k.glow.as_ref())
            .and_then(|g| g.color.as_ref())
            .map(|c| style.rgb(c, false));
        let active = style
            .active
            .color
            .as_ref()
            .map(|c| style.rgb(&c.color, false));
        let active_glow = style
            .active
            .glow
            .as_ref()
            .and_then(|g| g.color.as_ref())
            .map(|c| style.rgb(c, false));
        Self {
            fill: style.rgb(&style.fill, false),
            box_text,
            keyword,
            keyword_glow,
            active,
            active_glow,
            box_color,
            accent,
            accent_text: contrast(accent),
            outline: style
                .outline
                .as_ref()
                .map_or([0, 0, 0], |o| style.rgb(&o.color, false)),
            shadow: style
                .shadow
                .as_ref()
                .map_or([0, 0, 0], |s| style.rgb(&s.color, false)),
        }
    }
}
