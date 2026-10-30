//! A scene text shaped: one cosmic-text buffer for the whole text, so kerning, ligatures, joining scripts
//! and right-to-left runs are shaped in context, each span in its own face and size, and the Chinese, Japanese or
//! Korean stretches in their language's face (the text's runs). Then read back as lines and glyphs in the node's box.
//! Only ever when a text is laid out (a build, a resize, a font change), never per frame.

use std::ops::Range;

use glyphon::cosmic_text::{FeatureTag, FontFeatures, LayoutGlyph};
use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Style, Weight, Wrap};

use klipolo_model::scene::Align;

use crate::render::fonts::{FaceChoice, width_of};

/// What a text is shaped with: its looks' faces and sizes (target pixels at the fit's scale 1), which look each byte
/// range takes, the CJK runs' families, its letter spacing (a share of each look's size) and line height.
pub(super) struct Shaping<'a> {
    pub text: &'a str,
    /// Byte ranges and the look each takes (0 the node's own); bytes outside any take look 0.
    pub spans: &'a [(Range<usize>, u8)],
    pub faces: &'a [FaceChoice],
    pub sizes: &'a [f32],
    /// The runs over the text (empty: every byte asks for its look's face).
    pub runs: &'a [(Range<usize>, std::sync::Arc<str>)],
    /// The family the runs name for bytes that aren't CJK: such a byte asks for its own look's face instead.
    pub primary: &'a str,
    pub letter_spacing_em: &'a [f32],
    pub line_height: f32,
    /// Units drawn one grapheme at a time: no ligature joins two of them.
    pub no_ligatures: bool,
    /// A counter's digits on tabular figures (`tnum`, when the face has them).
    pub tabular: bool,
}

/// One laid-out line: its top, height and baseline down the block, its glyphs (`Lines::glyphs`), whether its paragraph
/// runs right to left, and its glyphs' left and right edges (whitespace at either end left out).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Line {
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
    pub glyphs: [usize; 2],
    pub rtl: bool,
    pub ink: [f32; 2],
}

/// A shaped text's lines and glyphs, each glyph's bytes made the whole text's.
pub(super) struct Lines {
    pub lines: Vec<Line>,
    pub glyphs: Vec<LayoutGlyph>,
}

impl Lines {
    /// The widest line's width and the block's height.
    pub fn extent(&self) -> (f32, f32) {
        let w = self
            .lines
            .iter()
            .map(|l| l.ink[1] - l.ink[0])
            .fold(0.0, f32::max);
        let h = self.lines.last().map_or(0.0, |l| l.top + l.height);
        (w, h)
    }
}

/// `s` shaped at `scale` of its sizes, wrapped at `wrap` pixels (breaking inside a word wider than that, as titles
/// do) or not at all, every line left-aligned (the caller aligns them in the box).
pub(super) fn shape(fs: &mut FontSystem, s: &Shaping, scale: f32, wrap: Option<f32>) -> Lines {
    let node_px = look_px(s, scale, 0);
    let mut buffer = Buffer::new(fs, Metrics::new(node_px, node_px * s.line_height));
    buffer.set_size(fs, wrap.map(|w| w.max(1.0)), None);
    buffer.set_wrap(
        fs,
        if wrap.is_some() {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        },
    );
    let mut features = FontFeatures::new();
    if s.no_ligatures {
        for tag in [b"liga", b"clig", b"dlig"] {
            features.disable(FeatureTag::new(tag));
        }
    }
    if s.tabular {
        features.enable(FeatureTag::new(b"tnum"));
    }
    // Cut where a span or a run starts or ends: each piece asks for one look and one family.
    let mut cuts: Vec<usize> = vec![0, s.text.len()];
    cuts.extend(s.spans.iter().flat_map(|(r, _)| [r.start, r.end]));
    cuts.extend(s.runs.iter().flat_map(|(r, _)| [r.start, r.end]));
    cuts.retain(|&c| c <= s.text.len() && s.text.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let pieces = cuts.windows(2).filter_map(|w| {
        let (a, b) = (w[0], w[1]);
        let look = s
            .spans
            .iter()
            .find(|(r, _)| r.start <= a && a < r.end)
            .map_or(0, |(_, l)| usize::from(*l));
        let family = s
            .runs
            .iter()
            .find(|(r, _)| r.start <= a && a < r.end)
            .map(|(_, f)| &**f)
            .filter(|f| *f != s.primary);
        Some((s.text.get(a..b)?, attrs(s, &features, scale, look, family)))
    });
    buffer.set_rich_text(
        fs,
        pieces,
        &attrs(s, &features, scale, 0, None),
        glyphon::Shaping::Advanced,
        Some(glyphon::cosmic_text::Align::Left),
    );
    buffer.shape_until_scroll(fs, false);
    read(&buffer, s.text)
}

/// Look `look`'s size in target pixels at `scale`.
fn look_px(s: &Shaping, scale: f32, look: usize) -> f32 {
    (s.sizes.get(look).copied().unwrap_or(s.sizes[0]) * scale).max(1.0)
}

/// What look `look` asks for (its face, or `family`, a CJK run's), at its size at `scale`.
fn attrs<'a>(
    s: &'a Shaping,
    features: &FontFeatures,
    scale: f32,
    look: usize,
    family: Option<&'a str>,
) -> Attrs<'a> {
    let face = s.faces.get(look).unwrap_or(&s.faces[0]);
    let px = look_px(s, scale, look);
    let family = match family {
        Some(f) => Family::Name(f),
        None if face.no_file => Family::SansSerif,
        None => Family::Name(&face.family),
    };
    let mut a = Attrs::new()
        .family(family)
        .weight(Weight(face.weight))
        .stretch(width_of(face.stretch))
        .style(if face.italic {
            Style::Italic
        } else {
            Style::Normal
        })
        .metrics(Metrics::new(px, px * s.line_height))
        .font_features(features.clone());
    let em = s.letter_spacing_em.get(look).copied().unwrap_or(0.0);
    if em != 0.0 {
        a = a.letter_spacing(em);
    }
    a
}

/// The buffer's lines and glyphs, each glyph's bytes made the text's (cosmic-text counts them per paragraph).
fn read(buffer: &Buffer, text: &str) -> Lines {
    let mut starts = Vec::with_capacity(buffer.lines.len());
    let mut at = 0usize;
    for line in &buffer.lines {
        starts.push(at);
        at += line.text().len() + line.ending().as_str().len();
    }
    let mut out = Lines {
        lines: Vec::new(),
        glyphs: Vec::new(),
    };
    for run in buffer.layout_runs() {
        let base = starts.get(run.line_i).copied().unwrap_or(0);
        let from = out.glyphs.len();
        let mut ink = [f32::MAX, f32::MIN];
        for g in run.glyphs {
            let mut g = g.clone();
            g.start += base;
            g.end += base;
            let blank = text
                .get(g.start..g.end)
                .is_none_or(|t| t.chars().all(char::is_whitespace));
            if !blank {
                ink = [ink[0].min(g.x), ink[1].max(g.x + g.w)];
            }
            out.glyphs.push(g);
        }
        if ink[0] > ink[1] {
            ink = [0.0, 0.0];
        }
        out.lines.push(Line {
            top: run.line_top,
            height: run.line_height,
            baseline: run.line_y,
            glyphs: [from, out.glyphs.len()],
            rtl: run.rtl,
            ink,
        });
    }
    out
}

/// Each line moved to its place in a `bw` × `bh` box: by `align` (by its paragraph's direction without one: left
/// for left-to-right, right for right-to-left), and the block centred down the box, or from its top when taller.
pub(super) fn place(lines: &mut Lines, align: Option<Align>, (bw, bh): (f32, f32)) {
    let (_, h) = lines.extent();
    let dy = if h <= bh { (bh - h) / 2.0 } else { 0.0 };
    for line in &mut lines.lines {
        let [x0, x1] = line.ink;
        let dx = match align.unwrap_or(if line.rtl { Align::Right } else { Align::Left }) {
            Align::Left => -x0,
            Align::Center => (bw - (x1 - x0)) / 2.0 - x0,
            Align::Right => bw - x1,
        };
        for g in &mut lines.glyphs[line.glyphs[0]..line.glyphs[1]] {
            g.x += dx;
        }
        line.ink = [x0 + dx, x1 + dx];
        line.top += dy;
        line.baseline += dy;
    }
}
