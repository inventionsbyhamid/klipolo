//! A caption style's colours and looks: paints, `#RRGGBB` and the black-or-white contrast, the
//! named looks (variants), and the style as drawn with a look and the person's colour over it (`style::*` re-exports
//! the types).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::style::{BoxDef, CaptionStyleDef, OutlineDef, ShadowDef};

/// `#RRGGBB`, `accent` (the style's accent, or the person's), or `contrast` (black or white against the box behind).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "CaptionPaint")]
pub struct Paint(pub String);

/// A named look of the style (Clean's Plate and Outlined, Boxed's Dark and Tint): its fields replace the style's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionVariantDef")]
pub struct VariantDef {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub accent: Option<String>,
    #[serde(default)]
    pub fill: Option<Paint>,
    #[serde(default)]
    pub outline: Option<OutlineDef>,
    #[serde(default)]
    pub boxes: Option<BoxDef>,
    #[serde(default)]
    pub shadow: Option<ShadowDef>,
}

pub fn hex_ok(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn parse_hex(s: &str) -> [u8; 3] {
    let v = |i: usize| {
        s.get(i..i + 2)
            .and_then(|d| u8::from_str_radix(d, 16).ok())
            .unwrap_or(255)
    };
    [v(1), v(3), v(5)]
}

/// Black or white, whichever reads on `bg` (WCAG relative luminance above 0.4 is light).
pub fn contrast(bg: [u8; 3]) -> [u8; 3] {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(bg[0]) + 0.7152 * lin(bg[1]) + 0.0722 * lin(bg[2]);
    if l > 0.4 {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    }
}

impl CaptionStyleDef {
    /// The colour `paint` draws in; `on_box`: the word sits on this style's box (so `contrast` is against it).
    pub fn rgb(&self, paint: &Paint, on_box: bool) -> [u8; 3] {
        match paint.0.as_str() {
            "accent" => parse_hex(&self.accent),
            "contrast" => {
                let bg = self
                    .boxes
                    .as_ref()
                    .filter(|_| on_box)
                    .map_or([0, 0, 0], |b| self.rgb(&b.color, false));
                contrast(bg)
            }
            hex => parse_hex(hex),
        }
    }

    /// This style with `variant`'s fields over it, then `accent` (the person's) over that. Unknown variants are
    /// ignored here: validation refuses them before (`chat.captions.noLook`, `model.captions.look`).
    pub fn with(&self, variant: Option<&str>, accent: Option<&str>) -> CaptionStyleDef {
        let mut s = self.clone();
        if let Some(v) = variant.and_then(|k| self.variants.iter().find(|v| v.key == k)) {
            if let Some(a) = &v.accent {
                s.accent = a.clone();
            }
            if let Some(f) = &v.fill {
                s.fill = f.clone();
            }
            if let Some(o) = &v.outline {
                s.outline = Some(o.clone());
            }
            if let Some(b) = &v.boxes {
                s.boxes = Some(b.clone());
            }
            if let Some(sh) = &v.shadow {
                s.shadow = Some(sh.clone());
            }
        }
        if let Some(a) = accent.filter(|a| hex_ok(a)) {
            s.accent = a.to_owned();
        }
        s
    }
}
