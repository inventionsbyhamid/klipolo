//! A caption word measured: its advance and its face's height, shaped on one line in the face the page
//! draws in (its CJK runs in their own families), and kept in a small cache keyed by the fonts' generation, the font
//! system, the face, the text, the size and the runs, so laying a page out again shapes only the words it changed.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight, Wrap};

use crate::render::fonts::{self, FaceChoice, width_of};

/// Shaped widths kept (≈ 30 KB), cleared when full.
const MEASURED_CAP: usize = 1024;

static MEASURED: LazyLock<Mutex<HashMap<u64, (f32, f32)>>> = LazyLock::new(Mutex::default);

/// `text`'s advance and its face's ascent plus descent at `px`, from the cache (keyed by the fonts' generation, the
/// face, the text, the size and the runs) or shaped.
pub(super) fn measure(
    fs: &mut FontSystem,
    face: &FaceChoice,
    text: &str,
    px: f32,
    spacing_em: f32,
    runs: &[(Range<usize>, Arc<str>)],
) -> (f32, f32) {
    let key = {
        let mut h = DefaultHasher::new();
        // The font system's own faces too: the managed-only system and the scanned one measure an
        // installed family differently at one generation, for the moment the scan publishes between the two.
        let generation = (fonts::generation(), fs.db().len());
        (
            generation,
            &face.family,
            face.weight,
            face.italic,
            face.stretch,
            face.no_file,
        )
            .hash(&mut h);
        (text, px.to_bits(), spacing_em.to_bits()).hash(&mut h);
        for (r, f) in runs {
            (r.start, r.end, &**f).hash(&mut h);
        }
        h.finish()
    };
    if let Some(&m) = MEASURED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return m;
    }
    let buffer = shaped(fs, face, text, px, spacing_em, runs);
    let w = buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
    let h = buffer
        .lines
        .first()
        .and_then(|l| l.layout_opt())
        .and_then(|l| l.first())
        .map_or(px, |l| l.max_ascent + l.max_descent);
    let m = (w, h.max(1.0));
    let mut measured = MEASURED.lock().unwrap_or_else(PoisonError::into_inner);
    if measured.len() >= MEASURED_CAP {
        measured.clear();
    }
    measured.insert(key, m);
    m
}

/// The width of a space between two words at `px` in `face`.
pub(super) fn space_width(fs: &mut FontSystem, face: &FaceChoice, px: f32, spacing_em: f32) -> f32 {
    let buffer = shaped(fs, face, " ", px, spacing_em, &[]);
    buffer
        .layout_runs()
        .flat_map(|r| r.glyphs.iter())
        .map(|g| g.w)
        .sum()
}

/// `text` shaped on one line in `face`, its CJK runs in their own families.
fn shaped(
    fs: &mut FontSystem,
    face: &FaceChoice,
    text: &str,
    px: f32,
    spacing_em: f32,
    runs: &[(Range<usize>, Arc<str>)],
) -> Buffer {
    let family = if face.no_file {
        Family::SansSerif
    } else {
        Family::Name(&face.family)
    };
    let mut attrs = Attrs::new()
        .family(family)
        .weight(Weight(face.weight))
        .stretch(width_of(face.stretch))
        .style(if face.italic {
            Style::Italic
        } else {
            Style::Normal
        });
    if spacing_em != 0.0 {
        attrs = attrs.letter_spacing(spacing_em);
    }
    let px = px.max(1.0);
    let mut buffer = Buffer::new(fs, Metrics::new(px, px));
    buffer.set_size(fs, None, None);
    buffer.set_wrap(fs, Wrap::None);
    if runs.is_empty() {
        buffer.set_text(fs, text, &attrs, Shaping::Advanced);
    } else {
        let spans = runs.iter().filter_map(|(r, f)| {
            Some((text.get(r.clone())?, attrs.clone().family(Family::Name(f))))
        });
        buffer.set_rich_text(fs, spans, &attrs, Shaping::Advanced, None);
    }
    buffer.shape_until_scroll(fs, false);
    buffer
}
