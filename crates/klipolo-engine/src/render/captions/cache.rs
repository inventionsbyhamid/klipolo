//! The caption drawer's kept atlases: each page's layout and atlas under what it is drawn from
//! (`Key`), at most `PER_FORMAT` and `FORMAT_BYTES` per format, least recently used first out, never the page drawn
//! nor the next. Keys are content hashes, never pointers, so a new revision whose page didn't change finds its atlas.

use std::hash::{DefaultHasher, Hash, Hasher};

use klipolo_model::captions::{Keyword, Pages};
use klipolo_model::project::FormatKind;
use klipolo_model::text_language::CjkScript;

use super::atlas::PageAtlas;
use super::layout::PageLayout;
use crate::render::fonts;

/// At most this many atlases per format…
pub const PER_FORMAT: usize = 4;
/// …and this many bytes (the page drawn and the next are kept even past it).
pub const FORMAT_BYTES: u64 = 16 << 20;

/// What an atlas is drawn from. Every field is `Copy`, so making it each frame allocates nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Key {
    pub(super) style: u64,
    pub(super) face: u64,
    pub(super) format: FormatKind,
    pub(super) out: (u32, u32),
    pub(super) max_width_px: u32,
    pub(super) words: u64,
    pub(super) language: u64,
    pub(super) generation: u64,
    /// Whether the system font scan was done: until the template is set, a font system made at the scan's own
    /// generation holds the managed faces only, so a page built then mustn't be taken for one built after.
    pub(super) scanned: bool,
}

pub(super) struct Entry {
    pub(super) key: Key,
    pub(super) layout: PageLayout,
    pub(super) atlas: PageAtlas,
    pub(super) bind: wgpu::BindGroup,
    /// Its CJK asks for a Noto Sans that isn't downloaded (`frame.cjkFontMissing`).
    pub(super) cjk_missing: bool,
    pub(super) used: u64,
}

/// The atlases kept, every format's.
#[derive(Default)]
pub(super) struct Atlases(pub(super) Vec<Entry>);

impl Atlases {
    pub(super) fn find(&self, key: &Key) -> Option<usize> {
        self.0.iter().position(|e| e.key == *key)
    }

    /// The key of the most recently drawn atlas of the same page as `key` (its format and words) under another key, if
    /// one is kept. O(atlases).
    pub(super) fn same_page(&self, key: &Key) -> Option<Key> {
        self.0
            .iter()
            .filter(|e| e.key.format == key.format && e.key.words == key.words)
            .max_by_key(|e| e.used)
            .map(|e| e.key)
    }

    /// Evicts `format`'s least recently used atlases past `PER_FORMAT` or `FORMAT_BYTES`, never `keep`'s.
    pub(super) fn evict(&mut self, format: FormatKind, keep: [Option<Key>; 2]) {
        loop {
            let (count, bytes) = self
                .0
                .iter()
                .filter(|e| e.key.format == format)
                .fold((0, 0), |(n, b), e| (n + 1, b + e.atlas.bytes));
            if count <= PER_FORMAT && bytes <= FORMAT_BYTES {
                return;
            }
            let victim = self
                .0
                .iter()
                .enumerate()
                .filter(|(_, e)| e.key.format == format && !keep.contains(&Some(e.key)))
                .min_by_key(|(_, e)| e.used)
                .map(|(i, _)| i);
            match victim {
                Some(i) => drop(self.0.swap_remove(i)),
                None => return,
            }
        }
    }

    /// The bytes of every atlas kept.
    pub(super) fn bytes(&self) -> u64 {
        self.0.iter().map(|e| e.atlas.bytes).sum()
    }
}

pub(super) fn hash_of(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// What page `page`'s atlas is drawn from, in a fixed field order: each word's shown text (its dropped final period
/// included), the space before it, its hiding and new line, its size step and emphasis, and whether it is a keyword.
/// Its colours aren't: an atlas is white masks. O(words), no allocation.
pub(super) fn words_hash(pages: &Pages, page: usize) -> u64 {
    let mut h = DefaultHasher::new();
    if let Some(pg) = pages.pages.get(page) {
        for i in pg.words.0..pg.words.1 {
            let Some(w) = pages.words.get(i as usize) else {
                break;
            };
            let look = pages.look(w);
            let spaced = pages
                .text
                .as_bytes()
                .get((w.text.0 as usize).wrapping_sub(1))
                == Some(&b' ');
            let keyword = pages.keyword(i).map(|k| match k {
                Keyword::Plain => 0u8,
                Keyword::Gain => 1,
                Keyword::Loss => 2,
            });
            (
                pages.word_text(pg, i),
                spaced,
                w.hidden,
                w.line_break,
                look.size,
                look.emphasis,
                keyword,
            )
                .hash(&mut h);
        }
    }
    h.finish()
}

/// Whether `layout`'s CJK runs ask for a Noto Sans that isn't downloaded.
pub(super) fn cjk_missing(layout: &PageLayout) -> bool {
    layout.runs.iter().any(|(_, _, family)| {
        CjkScript::ALL.iter().any(|&script| {
            &**family == klipolo_fonts::fallback::cjk_alias(script)
                && !fonts::cjk_registered(script)
        })
    })
}
