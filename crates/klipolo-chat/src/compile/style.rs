//! `setLayer`'s style keys on words of the additions' working copy: a text layer's style, or animated words' look
//! settings (`media::text_motion`), changed as the text card changes them, in the same step as the layout. A look goes
//! on first (its colour, box, outline at its step for the size, shadow), then each key given on top: a new size
//! scales an outline with it, as the card does; bold and italic pick the font's nearest face. A font is named as
//! `addText` names one, from the app's fonts; one still to download is said in the reply (`font_note`). A title
//! template keeps its own look.

use klipolo_model::media::text_motion;
use klipolo_model::timeline::{Item, ItemContent, Outline, TextStyle};

use super::{Compiler, Refused, to_model};
use crate::tools::{OutlineStep, Paint, StyleChange};

/// The weight bold is, and the one not bold is.
const BOLD: u16 = 700;
const REGULAR: u16 = 400;

/// `style` with `change`, named for the model as `said`; and the font's family when it is still to download.
fn restyled(
    c: &Compiler,
    style: &TextStyle,
    change: &StyleChange,
    said: &str,
) -> Result<(TextStyle, Option<String>), Refused> {
    let mut s = style.clone();
    if let Some(look) = change.look {
        s.color = look.color.to_owned();
        s.background = look.highlight.map(str::to_owned);
        s.outline = (look.outline).map(|(color, step)| Outline {
            color: color.to_owned(),
            width: step.width(s.size),
        });
        s.shadow = look.shadow;
    }
    if let Some(size) = change.size {
        if let Some(o) = &mut s.outline {
            o.width = (o.width * size / s.size * 100.0).round() / 100.0;
        }
        s.size = size;
    }
    if let Some(color) = &change.color {
        s.color = color.clone();
    }
    match &change.highlight {
        Some(Paint::Color(c)) => s.background = Some(c.clone()),
        Some(Paint::None) => s.background = None,
        None => {}
    }
    match &change.outline {
        Some(Paint::Color(color)) => {
            let width = (change.outline_width)
                .map(|step| step.width(s.size))
                .or(s.outline.as_ref().map(|o| o.width))
                .unwrap_or_else(|| OutlineStep::Medium.width(s.size));
            s.outline = Some(Outline {
                color: color.clone(),
                width,
            });
        }
        Some(Paint::None) => s.outline = None,
        None => {
            if let Some(step) = change.outline_width {
                let Some(o) = &mut s.outline else {
                    return Err(to_model(format!(
                        "setLayer: {said} has no outline for outlineWidth to change. Give outline a colour too."
                    )));
                };
                o.width = step.width(s.size);
            }
        }
    }
    if let Some(shadow) = change.shadow {
        s.shadow = shadow;
    }
    if let Some(align) = change.align {
        s.align = align;
    }
    let weight = change
        .bold
        .map_or(s.weight, |b| if b { BOLD } else { REGULAR });
    let italic = change.italic.unwrap_or(s.italic);
    let mut download = None;
    if change.font.is_some() || weight != s.weight || italic != s.italic {
        let family = change.font.clone().unwrap_or_else(|| s.font.family.clone());
        match c.inputs.fonts.named(&family, weight, italic) {
            Some(named) => {
                if named.download_bytes.is_some() {
                    download = Some(named.font.family.clone());
                }
                s.font = named.font;
                s.weight = named.weight;
                s.italic = named.italic;
            }
            None if change.font.is_some() => {
                return Err(to_model(format!(
                    "setLayer: no font is named {family:?}, here or at Google. Leave font out to keep {}.",
                    s.font.family
                )));
            }
            // The font's own faces aren't known here: the weight and slant asked for, as the card sets them.
            None => {
                s.weight = weight;
                s.italic = italic;
            }
        }
    }
    Ok((s, download))
}

/// `item`'s content with its words restyled by `change` (unchanged when `change` sets nothing), named `said`; and the
/// font's family when it is still to download. A layer that isn't words, or is a title template, is refused.
pub(super) fn restyled_content(
    c: &Compiler,
    item: &Item,
    change: &StyleChange,
    said: &str,
) -> Result<(ItemContent, Option<String>), Refused> {
    if *change == StyleChange::default() {
        return Ok((item.content.clone(), None));
    }
    match &item.content {
        ItemContent::Text { text, style } => {
            let (style, download) = restyled(c, style, change, said)?;
            let text = text.clone();
            Ok((ItemContent::Text { text, style }, download))
        }
        ItemContent::Generator { source, params, .. } if source.as_str() == text_motion::SOURCE => {
            let (text, style, motion) = text_motion::read(params);
            let (style, download) = restyled(c, &style, change, said)?;
            let mut content = item.content.clone();
            if let ItemContent::Generator { params, .. } = &mut content {
                *params = text_motion::params(&text, &style, &motion);
            }
            Ok((content, download))
        }
        ItemContent::Generator { .. } => Err(to_model(format!(
            "setLayer: {said} is a title template with its own look, so it takes no style keys. Style words added with \
             addText instead."
        ))),
        _ => Err(to_model(format!(
            "setLayer: {said} is not words, so it takes no style keys (font, size, color and the rest)."
        ))),
    }
}

/// What the reply says of a font still to download, as `addText`'s note says it.
pub(super) fn font_note(family: &str) -> String {
    format!(
        "{family} isn't on this Mac yet, so the words show in Inter until you download it over the preview."
    )
}
